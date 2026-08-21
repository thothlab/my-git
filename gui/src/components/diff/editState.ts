import { createSignal } from "solid-js";
import {
  errText,
  fileRead,
  fileWrite,
  isStaleError,
  type Eol,
} from "../../api";
import { chooseOption, setError } from "../../store";
import { d } from "../../i18n";
import {
  AUTOSAVE_MS,
  draftReduce,
  draftShouldWrite,
  type Draft,
  type DraftEvent,
  type EditBlock,
} from "./editRules";

/**
 * The draft of the file being edited in the right column, and everything that
 * carries it to disk: the deferred write, the digest of the last thing the
 * application read or wrote there, and the choice offered when someone else
 * changed the file underneath.
 *
 * **The signals are module-level, not component-level, on purpose.** `Cmd+1` /
 * `Cmd+2` are matched above the "the user is typing" cut-off in `hotkeys.ts`,
 * so switching the window mode unmounts the panel from under an open editor.
 * A draft owned by the component would go with it, and the promise this feature
 * makes is that what was typed does not disappear.
 *
 * The text lives in a signal of its own rather than inside a state object: a
 * prop derived from an object is reactive on the whole object, and every
 * expensive consumer would recompute on each character.
 *
 * Writing does **not** go through `store.run()` — the one mutation of the
 * project that does not. A write every 800 ms through the mutation funnel would
 * flash the window's toolbar disabled, republish the global state and take a
 * full repository snapshot on every pause in typing. A failure still reaches the
 * user by the usual road: the same banner, through `setError`. The rule that a
 * component writes no `try/catch` of its own also holds — this module is the
 * action layer, and it is the one holding them.
 */

const [path, setPath] = createSignal<string | null>(null);
const [text, setText] = createSignal("");
const [saved, setSaved] = createSignal("");
const [blocked, setBlocked] = createSignal<EditBlock | null>(null);

/** Text of the write in flight, `null` when none is. Not a signal: nothing draws it. */
let writing: string | null = null;
/** Fingerprint of what the application last read or wrote at `path`. Every
 *  successful write replaces it, or the next automatic save would compare
 *  against a digest its own predecessor made stale. */
let digest = "";
let eol: Eol = "lf";
let timer: ReturnType<typeof setTimeout> | undefined;
/** One stale-file question at a time, however many writes fail behind it. */
let asking = false;

/** The file being edited, or `null` when the editor is closed. */
export const editorPath = path;
/** The draft as it stands on screen. */
export const editorText = text;
/** Why the open file could not be edited, when a read said so. */
export const editorBlocked = blocked;
export const editorOpen = () => path() !== null;
/** Is there anything typed that the file on disk does not have? */
export const editorDirty = () => text() !== saved();

/**
 * Why a file cannot be edited, in words. Lives here rather than in the panel
 * because both readers need it: the panel names the reason on its disabled
 * control, and this module names it when a reread turns a file that was
 * editable a moment ago into one that is not.
 */
export function blockText(b: EditBlock | null): string {
  return b === "binary"
    ? d().editOffBinary()
    : b === "too-large"
      ? d().editOffTooLarge()
      : b === "mixed-eol"
        ? d().editOffMixedEol()
        : d().editOffMissing();
}

const snapshot = (): Draft => ({ text: text(), saved: saved(), writing });
const apply = (e: DraftEvent) => {
  const next = draftReduce(snapshot(), e);
  setText(next.text);
  setSaved(next.saved);
  writing = next.writing;
};

const disarm = () => {
  if (timer !== undefined) clearTimeout(timer);
  timer = undefined;
};

/** A keystroke: the draft moves and the deferred write is pushed back. */
export function setEditorText(t: string): void {
  if (!editorOpen()) return;
  apply({ kind: "type", text: t });
  disarm();
  timer = setTimeout(() => {
    timer = undefined;
    void write();
  }, AUTOSAVE_MS);
}

/**
 * Open `path` for editing, reading it fresh from disk.
 *
 * Fresh rather than from whatever the panel read for the availability check:
 * that copy's digest is the one from before the session's own writes, and the
 * first save after reopening the file would be refused as stale against it.
 *
 * Returns whether the editor is now open on this file.
 */
export async function openEditor(p: string): Promise<boolean> {
  if (path() === p) return true; // already open here — keep the draft
  closeEditor();
  try {
    const f = await fileRead(p);
    // Never open on a blocked file: `digest` is `""` for every blocked reason,
    // and a write with it against a file that exists comes back as "changed on
    // disk" — the user would be told the wrong thing about their own file.
    if (f.blocked !== null || f.text === null) {
      // The panel reads `editorBlocked()` to say why the press did nothing:
      // availability was judged on an earlier read, and a file can change
      // between that read and the click.
      setBlocked(f.blocked);
      return false;
    }
    digest = f.digest;
    eol = f.eol;
    setBlocked(null);
    setPath(p);
    apply({ kind: "synced", text: f.text });
    return true;
  } catch (e) {
    setError(errText(e));
    return false;
  }
}

/**
 * Close the editor, pushing a deferred write out on the way.
 *
 * The write is issued, not awaited: switching the window mode cannot wait for a
 * round trip, and the panel is already being torn down. A failure lands in the
 * shared banner the usual way.
 */
export function closeEditor(): void {
  flushPending();
  setPath(null);
  setText("");
  setSaved("");
  setBlocked(null);
  digest = "";
  eol = "lf";
}

/**
 * Issue whatever the timer was still holding, without waiting for it to land.
 * Called on every departure — a deferred write left in a timer is exactly the
 * "last few characters" that go missing when a panel is unmounted.
 */
export function flushPending(): void {
  disarm();
  void write();
}

/** Write now and wait for the answer — the explicit save, and every departure
 *  that can afford to wait for one. */
export async function saveNow(): Promise<void> {
  disarm();
  await write();
}

async function write(): Promise<void> {
  const p = path();
  if (p === null) return;
  if (!draftShouldWrite(snapshot())) return;
  const sent = text();
  const expect = digest;
  apply({ kind: "sent" });
  try {
    const w = await fileWrite(p, sent, eol, expect);
    // The editor moved on while the write travelled: the bytes are on disk, but
    // this draft is no longer the one on screen and must not be marked clean.
    if (path() !== p) {
      writing = null;
      return;
    }
    digest = w.digest;
    apply({ kind: "ok" });
  } catch (e) {
    if (path() !== p) {
      writing = null;
      return;
    }
    apply({ kind: "fail" });
    if (isStaleError(e)) await resolveStale(p, e);
    else setError(errText(e));
    return;
  }
  // Characters typed while the write was in flight: the draft is unsaved again
  // and nothing else will notice, because the timer already fired.
  if (draftShouldWrite(snapshot())) await write();
}

/**
 * Someone else changed the file. Nothing is overwritten by default: the user
 * picks between the two readings of what just happened, and the buttons say
 * which one costs what.
 *
 * **"Reread from disk" replaces the draft with the file** — the typed text is
 * gone, which is what the label promises and why it is the destructive one.
 * "Overwrite" keeps the draft and puts it on disk: a reread for the digest,
 * then a write with it — one road for a file that changed and for a file that
 * was deleted, because the digest of a file that is not there is `""`, and
 * `""` is exactly what tells the backend to create it.
 *
 * The reread can also come back *blocked* — someone replaced the text file with
 * a binary, or with something over the ceiling. Then there is nothing to
 * overwrite through: a blocked read has no digest either, and writing with the
 * empty one would be refused as "changed on disk" again, sending this same
 * question round a second time under a reason that is not the true one. The
 * blockage is named instead (rule 3 of `prd_03_interfaces.md`).
 */
async function resolveStale(p: string, cause: unknown): Promise<void> {
  // A question is already up. Only the duplicate question is suppressed — the
  // refusal itself is still the user's news, and swallowing it would leave a
  // write that never happened with nothing said about it.
  if (asking) {
    setError(errText(cause));
    return;
  }
  asking = true;
  let choice: string | null = null;
  try {
    choice = await chooseOption(d().editStaleAsk(), [
      { key: "reread", label: d().editStaleReread(), danger: true },
      { key: "overwrite", label: d().editStaleOverwrite() },
    ]);
  } finally {
    asking = false;
  }
  if (choice === null || path() !== p) return;
  try {
    const f = await fileRead(p);
    if (path() !== p) return;
    digest = f.digest;
    if (f.blocked === null) eol = f.eol;
    setBlocked(f.blocked);
    if (choice === "reread") {
      if (f.blocked !== null || f.text === null) {
        setError(blockText(f.blocked));
        return;
      }
      apply({ kind: "synced", text: f.text });
      return;
    }
    // `missing` is the one blockage an overwrite answers: its empty digest is
    // the documented way to create the file again with the typed text.
    if (f.blocked !== null && f.blocked !== "missing") {
      setError(blockText(f.blocked));
      return;
    }
    await write();
  } catch (e) {
    setError(errText(e));
  }
}
