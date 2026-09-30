import { For, Show, createEffect, createMemo, createSignal, on } from "solid-js";
import {
  conflictRead,
  conflictResolve,
  conflictTake,
  errText,
  fileRead,
  fileWrite,
  isStaleError,
  type ConflictFile,
  type ConflictSide,
  type Eol,
  type RepoState,
} from "../../api";
import { d } from "../../i18n";
import { busy, chooseOption, confirmAction, refresh, registerModalSource, setError, state } from "../../store";
import { blockText } from "../diff/editState";
import { runResult } from "../log/actions/repoRefresh";
import { continueOperation, operationWord } from "../log/actions/operation";
import {
  CONFLICT_CODES,
  DEFAULT_MARKER_SIZE,
  absorbEdit,
  asPicks,
  buildResult,
  coalesces,
  emptyHistory,
  historyRecord,
  historyRedo,
  historyUndo,
  leftoverMarkers,
  lineOffset,
  parseConflicts,
  sideLines,
  stepConflict,
  togglePick,
  unresolved,
  type Built,
  type ConflictRegion,
  type Decision,
  type History,
  type Pick,
  type Side,
} from "./conflictRules";

/**
 * The conflict editor (R05e): one conflicted file, its three sides and an
 * editable result.
 *
 * An overlay in the `BlamePanel` manner: it opens from the operation strip and
 * from the Changes panel, registers as a modal source, and answers its own keys
 * in `onKeyDown` — `hotkeys.ts` stands down while any modal is up.
 *
 * Seams worth naming:
 *
 *  - **The model is `conflictRules.ts`'s**: a text with markers (`base`) plus a
 *    decision per block; the result on screen is assembled from both. Typing is
 *    absorbed into the decided block it touched, or baked into `base`.
 *  - **Undo is this editor's own**, over `{base, decisions}` snapshots — never
 *    the textarea's: the textarea's value is replaced on every block action, and
 *    its native history would undo into texts the model never had. `Cmd+Z` is
 *    taken in `onKeyDown`, and a native undo arriving some other way (the Edit
 *    menu) is caught as `beforeinput` `historyUndo` and turned into ours.
 *  - **Save is `file_write`**, the second caller of that bypass of `run()` next
 *    to `diff/editState.ts`: a save changes no index and no ref. "Mark resolved"
 *    and the whole-side takes are mutations and go through `run()`
 *    (`runResult`, so the overlay can show the refusal it caused).
 *  - **A file changed underneath** is `kind: "stale"` from either road, and the
 *    same choice as in the in-place editor follows: reread, or overwrite.
 */

type Target = { path: string; repo: string };
type Snap = { base: string; decisions: (Decision | null)[] };

const [target, setTarget] = createSignal<Target | null>(null);
registerModalSource(() => target() !== null);

let returnFocus: HTMLElement | null = null;

/** Open the conflict editor for `path` (a path git reports as unmerged). */
export function openConflict(path: string): void {
  const repo = state()?.repoPath;
  if (!repo || !path) return;
  if (!target()) {
    const el = document.activeElement;
    returnFocus = el instanceof HTMLElement ? el : null;
  }
  setTarget({ path, repo });
}

function closeConflict(restore = true): void {
  setTarget(null);
  const el = returnFocus;
  returnFocus = null;
  if (restore && el?.isConnected) queueMicrotask(() => el.focus());
}

/** Every unmerged path right now: the operation's list, or — with no operation
 * (a `stash pop` that collided) — the conflicted rows of the Changes panel. */
function conflictedNow(): string[] {
  const s = state();
  if (!s) return [];
  // A bisect has no conflicts of its own: a `stash pop` that collided during one
  // is read like one with no operation at all.
  if (s.operation.kind !== "none" && s.operation.kind !== "bisect")
    return s.operation.conflicted.map((c) => c.path);
  return s.changelists.flatMap((c) => c.files).filter((f) => f.status === "conflicted").map((f) => f.path);
}

export default function ConflictPanel() {
  return (
    <Show when={target()} keyed>
      {(t) => <ConflictView target={t} />}
    </Show>
  );
}

function ConflictView(props: { target: Target }) {
  const path = props.target.path;
  let box: HTMLDivElement | undefined;
  let ta: HTMLTextAreaElement | undefined;
  const blockEls: HTMLElement[] = [];

  const [file, setFile] = createSignal<ConflictFile | null>(null);
  const [loadError, setLoadError] = createSignal("");
  const [actionError, setActionError] = createSignal("");
  const [base, setBase] = createSignal("");
  const [decisions, setDecisions] = createSignal<(Decision | null)[]>([]);
  /** What the file holds on disk, as far as this editor knows. */
  const [saved, setSaved] = createSignal("");
  const [history, setHistory] = createSignal<History<Snap>>(emptyHistory<Snap>());
  const [current, setCurrent] = createSignal(0);
  const [view, setView] = createSignal<"blocks" | "files">("blocks");
  const [done, setDone] = createSignal(false);
  const [working, setWorking] = createSignal(false);
  let digest = "";
  let eol: Eol = "lf";
  let lastChange: { kind: string; at: number } | null = null;
  /** A save reached the disk that no `RepoState` has followed yet. */
  let wrote = false;
  let seq = 0;

  const size = () => file()?.markerSize ?? DEFAULT_MARKER_SIZE;
  const parsed = createMemo(() => parseConflicts(base(), size()));
  const built = createMemo<Built>(() => {
    const p = parsed();
    return p.ok ? buildResult(p, decisions()) : { text: base(), spans: [] };
  });
  const result = () => built().text;
  const dirty = () => result() !== saved();
  const conflicts = (): ConflictRegion[] => {
    const p = parsed();
    return p.ok ? p.conflicts : [];
  };
  const left = createMemo(() => {
    const p = parsed();
    return p.ok ? unresolved(p, decisions()) : [];
  });
  const lineLevel = () => {
    const f = file();
    return !!f && !f.wholeOnly;
  };
  const noBaseInMarkers = () => conflicts().length > 0 && conflicts().every((c) => c.base === null);
  const opKind = () => state()?.operation.kind ?? "none";

  const load = async () => {
    const my = ++seq;
    setLoadError("");
    setActionError("");
    try {
      const f = await conflictRead(path);
      if (my !== seq) return;
      const text = f.worktree.text ?? "";
      digest = f.worktree.digest;
      eol = f.worktree.eol;
      lastChange = null;
      setFile(f);
      setBase(text);
      setDecisions([]);
      setSaved(text);
      setHistory(emptyHistory<Snap>());
      setCurrent(0);
      setDone(false);
    } catch (e) {
      if (my === seq) setLoadError(errText(e));
    }
  };
  void load();
  queueMicrotask(() => box?.focus());

  // The repository switched underneath: the conflict is of the old one.
  createEffect(() => {
    if (state()?.repoPath !== props.target.repo) closeConflict(false);
  });

  // The textarea follows the model, but is never rewritten with the text it
  // already holds — that would move the caret to the end on every keystroke.
  createEffect(() => {
    const v = result();
    if (!ta || ta.value === v) return;
    // Undo and redo land here while the caret is in the text: keep it — and the
    // scroll — where it was rather than let the assignment throw it to the end.
    const { selectionStart, selectionEnd, scrollTop } = ta;
    ta.value = v;
    // Only when it has the focus: older WebKit focuses a field on setSelectionRange.
    if (document.activeElement === ta)
      ta.setSelectionRange(Math.min(selectionStart, v.length), Math.min(selectionEnd, v.length));
    ta.scrollTop = scrollTop;
  });

  // After a bake renumbers the blocks, the current one may be gone.
  createEffect(
    on(conflicts, (cs) => {
      if (current() >= cs.length) setCurrent(Math.max(0, cs.length - 1));
    }),
  );

  // ── changes and the editor's own undo ──────────────────────────────────────

  const snap = (): Snap => ({ base: base(), decisions: decisions() });
  const restore = (s: Snap) => {
    setBase(s.base);
    setDecisions(s.decisions);
  };
  const change = (kind: "block" | "pick" | "type", next: Snap) => {
    const now = Date.now();
    if (!coalesces(lastChange, kind, now)) setHistory((h) => historyRecord(h, snap()));
    lastChange = { kind, at: now };
    restore(next);
  };
  const undo = () => {
    const r = historyUndo(history(), snap());
    if (!r) return;
    lastChange = null;
    setHistory(r.history);
    restore(r.value);
  };
  const redo = () => {
    const r = historyRedo(history(), snap());
    if (!r) return;
    lastChange = null;
    setHistory(r.history);
    restore(r.value);
  };

  const withDecision = (index: number, dec: Decision | null) => {
    const next = conflicts().map((c) => decisions()[c.index] ?? null);
    next[index] = dec;
    return next;
  };
  const decide = (index: number, dec: Decision | null) => {
    setCurrent(index);
    change("block", { base: base(), decisions: withDecision(index, dec) });
  };
  const pick = (region: ConflictRegion, p: Pick) => {
    setCurrent(region.index);
    change("pick", { base: base(), decisions: withDecision(region.index, togglePick(region, decisions()[region.index], p)) });
  };
  const typed = (value: string) => {
    const p = parsed();
    if (p.ok) {
      const absorbed = absorbEdit(p, decisions(), built(), value);
      if (absorbed) {
        change("type", { base: base(), decisions: absorbed });
        return;
      }
    }
    change("type", { base: value, decisions: [] });
  };

  // ── navigation ─────────────────────────────────────────────────────────────

  const showBlock = (index: number) => {
    setCurrent(index);
    blockEls[index]?.scrollIntoView({ block: "nearest" });
    const span = built().spans.find((s) => s.index === index);
    if (ta && span) {
      const lh = parseFloat(getComputedStyle(ta).lineHeight) || 16;
      ta.scrollTop = Math.max(0, span.from * lh - ta.clientHeight / 3);
      if (document.activeElement === ta) {
        const text = result();
        ta.setSelectionRange(lineOffset(text, span.from), lineOffset(text, span.to));
      }
    }
  };
  const go = (dir: 1 | -1) => {
    const list = left().length > 0 ? left() : conflicts().map((c) => c.index);
    const next = stepConflict(list, current(), dir);
    if (next !== null) showBlock(next);
  };

  // ── disk ───────────────────────────────────────────────────────────────────

  /**
   * Someone changed the file underneath. The same two readings as in the in-place
   * editor: reread (the result here is lost) or overwrite (a reread for the
   * digest, then `retry` with it). `true` when `retry` ran and succeeded.
   */
  const onStale = async (retry: () => Promise<boolean>): Promise<boolean> => {
    const choice = await chooseOption(d().editStaleAsk(), [
      { key: "reread", label: d().editStaleReread(), danger: true },
      { key: "overwrite", label: d().editStaleOverwrite() },
    ]);
    if (choice === "reread") {
      setError("");
      await load();
      return false;
    }
    if (choice !== "overwrite") return false;
    try {
      digest = (await fileRead(path)).digest;
    } catch (e) {
      setActionError(errText(e));
      return false;
    }
    return retry();
  };

  const save = async (): Promise<boolean> => {
    const text = result();
    setActionError("");
    try {
      digest = (await fileWrite(path, text, eol, digest)).digest;
      setSaved(text);
      wrote = true;
      return true;
    } catch (e) {
      if (isStaleError(e)) return onStale(save);
      setActionError(errText(e));
      return false;
    }
  };

  /** A mutation through `run()`, its refusal shown here as well as in the banner. */
  const mutate = async (p: Promise<RepoState>, label: string): Promise<"ok" | "stale" | "failed"> => {
    let stale = false;
    p.catch((e) => {
      stale = isStaleError(e);
    });
    setWorking(true);
    try {
      const err = await runResult(p, label);
      if (err === null) return "ok";
      if (!stale) setActionError(err);
      return stale ? "stale" : "failed";
    } finally {
      setWorking(false);
    }
  };

  const resolveText = async (): Promise<boolean> => {
    const text = result();
    const r = await mutate(conflictResolve(path, text, eol, digest), d().phaseConflictResolve());
    if (r === "ok") {
      setSaved(text);
      setDone(true);
      return true;
    }
    return r === "stale" ? onStale(resolveText) : false;
  };

  const markResolved = async () => {
    const f = file();
    if (!f || working()) return;
    setActionError("");
    if (lineLevel()) {
      const marks = leftoverMarkers(result(), size());
      if (marks.length > 0 && !(await confirmAction(d().conflictMarkersLeft(marks.slice(0, 5).join(", ")), true)))
        return;
      await resolveText();
      return;
    }
    // As it lies. A digest guards only a file this editor could fingerprint: a
    // text one, or an absent one (`""` means "must be absent").
    const b = f.worktree.blocked;
    const expect = b === null || b === "missing" ? digest : null;
    const r = await mutate(conflictResolve(path, null, eol, expect), d().phaseConflictResolve());
    if (r === "ok") setDone(true);
    else if (r === "stale") await load();
  };

  const takeWhole = async (side: "ours" | "theirs") => {
    if (working()) return;
    if (dirty() && !(await confirmAction(d().conflictWholeDiscards(), true))) return;
    setActionError("");
    if ((await mutate(conflictTake(path, side), d().phaseConflictTake())) === "ok") setDone(true);
  };

  const tryClose = async () => {
    if (!done() && dirty() && !(await confirmAction(d().conflictDiscardEdits(), true))) return;
    closeConflict();
    // Saved but not resolved: no mutation installed a state since, and the diff
    // of this file in the Changes panel behind is the one from before the save.
    // A fresh `RepoState` is what makes `DiffView` re-read (its `state` effect).
    if (!done() && wrote) void refresh();
  };

  const openNext = (next: string) => {
    closeConflict(false);
    queueMicrotask(() => openConflict(next));
  };
  const doContinue = async () => {
    closeConflict();
    await continueOperation();
  };

  // ── keys ───────────────────────────────────────────────────────────────────

  const onKeyDown = (e: KeyboardEvent) => {
    e.stopPropagation();
    const mod = e.metaKey || e.ctrlKey;
    if (e.code === "Escape") {
      e.preventDefault();
      void tryClose();
      return;
    }
    if (done()) return;
    // Before any "is the user typing" check: the textarea is where undo is wanted.
    if (mod && e.code === "KeyZ") {
      e.preventDefault();
      if (e.shiftKey) redo();
      else undo();
      return;
    }
    if (mod && e.code === "KeyY") {
      e.preventDefault();
      redo();
      return;
    }
    if (mod && e.code === "KeyS") {
      e.preventDefault();
      if (lineLevel() && dirty()) void save();
      return;
    }
    if (e.code === "F7") {
      e.preventDefault();
      go(e.shiftKey ? -1 : 1);
    }
  };

  const onBeforeInput = (e: InputEvent) => {
    if (e.inputType === "historyUndo" || e.inputType === "historyRedo") {
      e.preventDefault();
      if (e.inputType === "historyUndo") undo();
      else redo();
    }
  };

  // ── labels ─────────────────────────────────────────────────────────────────

  const how = (dec: Decision | null | undefined): string => {
    if (!dec) return d().conflictBlockOpen();
    const w =
      dec.kind === "ours"
        ? d().conflictHowOurs()
        : dec.kind === "theirs"
          ? d().conflictHowTheirs()
          : dec.kind === "both"
            ? dec.first === "ours"
              ? d().conflictHowBothOT()
              : d().conflictHowBothTO()
            : dec.kind === "lines"
              ? d().conflictHowLines(dec.picks.length)
              : d().conflictHowManual();
    return d().conflictBlockTaken(w);
  };

  const wholeWhy = (f: ConflictFile): string => {
    if (!f.ours || !f.theirs) return d().conflictWhyDeleted();
    const sides = [f.ours, f.base, f.theirs];
    if (sides.some((s) => s?.mode === "160000")) return d().conflictSubmodule();
    if (sides.some((s) => s?.mode === "120000")) return d().conflictSymlink();
    const b = f.worktree.blocked ?? sides.find((s) => s?.blocked)?.blocked ?? null;
    return b ? blockText(b) : "";
  };

  const parseProblem = () => {
    const p = parsed();
    return p.ok ? null : d().conflictParse(p.error.reason, p.error.line, p.error.size, size());
  };

  const nextFile = () => conflictedNow().find((p) => p !== path) ?? null;

  const wholeButtons = (f: ConflictFile) => (
    <>
      <WholeBtn
        side="ours"
        present={!!f.ours}
        disabled={working() || busy()}
        onClick={() => void takeWhole("ours")}
      />
      <WholeBtn
        side="theirs"
        present={!!f.theirs}
        disabled={working() || busy()}
        onClick={() => void takeWhole("theirs")}
      />
    </>
  );

  return (
    <div class="fixed inset-0 z-40 flex items-center justify-center bg-black/40">
      <div
        ref={box}
        tabindex={-1}
        class="flex h-[92vh] w-[96vw] flex-col rounded-lg border border-border bg-bg text-fg shadow-xl outline-none"
        onKeyDown={onKeyDown}
      >
        {/* Header */}
        <div class="flex flex-wrap items-center gap-2 border-b border-border px-4 py-2 text-xs">
          <span class="truncate font-mono text-sm font-semibold" title={path}>
            {d().conflictTitle(path)}
          </span>
          <Show when={file()}>
            {(f) => (
              <span class="shrink-0 text-fg-muted" title={d().conflictKind(f().kind)}>
                <span class="font-mono font-bold text-danger">{CONFLICT_CODES[f().kind]}</span>{" "}
                {d().conflictKind(f().kind)}
              </span>
            )}
          </Show>
          <Show when={lineLevel() && !done()}>
            <span class="shrink-0 rounded bg-bg-muted px-1.5 py-0.5 text-fg-muted">
              {d().conflictLeft(left().length)}
            </span>
          </Show>
          <div class="ml-auto flex flex-wrap items-center gap-1">
            <Show when={lineLevel() && !done()}>
              <Btn label={d().conflictPrev()} title={d().conflictNavTip()} disabled={conflicts().length === 0} onClick={() => go(-1)} />
              <Btn label={d().conflictNext()} title={d().conflictNavTip()} disabled={conflicts().length === 0} onClick={() => go(1)} />
              <span class="mx-1 h-4 w-px bg-border" />
              <Btn
                label={d().conflictUndo()}
                title={history().past.length === 0 ? d().conflictNothingToUndo() : `${d().conflictUndo()} (⌘/Ctrl+Z)`}
                disabled={history().past.length === 0}
                onClick={undo}
              />
              <Btn
                label={d().conflictRedo()}
                title={history().future.length === 0 ? d().conflictNothingToRedo() : `${d().conflictRedo()} (⌘/Ctrl+⇧Z)`}
                disabled={history().future.length === 0}
                onClick={redo}
              />
              <span class="mx-1 h-4 w-px bg-border" />
              <Btn
                label={dirty() ? d().conflictSave() : d().conflictSaved()}
                title={dirty() ? `${d().conflictSave()} (⌘/Ctrl+S)` : d().conflictSaved()}
                disabled={!dirty() || working()}
                onClick={() => void save()}
              />
              <Btn
                label={d().conflictMarkResolved()}
                title={d().conflictMarkResolvedTip()}
                accent
                disabled={working() || busy()}
                onClick={() => void markResolved()}
              />
              <span class="mx-1 h-4 w-px bg-border" />
            </Show>
            <Show when={file() && !done() && lineLevel()}>{wholeButtons(file()!)}</Show>
            <Btn label={d().close()} onClick={() => void tryClose()} />
          </div>
        </div>

        {/* Notes and errors */}
        <Show when={opKind() === "rebase" && !done()}>
          <div class="border-b border-border bg-warn/10 px-4 py-1 text-[0.6875rem] text-fg-muted">
            {d().conflictRebaseNote()}
          </div>
        </Show>
        <Show when={loadError() || actionError()}>
          <pre class="max-h-24 overflow-auto whitespace-pre-wrap border-b border-border bg-danger/10 px-3 py-2 font-mono text-[0.6875rem] text-danger">
            {loadError() || actionError()}
          </pre>
        </Show>

        <Show
          when={file()}
          fallback={
            <div class="flex flex-1 items-center justify-center text-xs text-fg-muted">
              {loadError() ? "" : d().conflictLoading()}
            </div>
          }
        >
          {(f) => (
            <Show when={!done()} fallback={<Finished next={nextFile()} onNext={openNext} onContinue={() => void doContinue()} onClose={() => closeConflict()} />}>
              <Show
                when={lineLevel()}
                fallback={
                  <div class="flex min-h-0 flex-1 flex-col">
                    <div class="space-y-2 border-b border-border px-4 py-3 text-xs">
                      <div class="text-fg-muted">{d().conflictWhyWhole(wholeWhy(f()))}</div>
                      <div class="flex flex-wrap gap-2">
                        {wholeButtons(f())}
                        <Btn
                          label={d().conflictMarkAsIs()}
                          title={d().conflictMarkAsIsTip()}
                          disabled={working() || busy()}
                          onClick={() => void markResolved()}
                        />
                      </div>
                    </div>
                    <WholeFiles file={f()} />
                  </div>
                }
              >
                <div class="flex min-h-0 flex-1 flex-col">
                  {/* Top: the blocks, or the whole files */}
                  <div class="flex items-center gap-2 border-b border-border px-4 py-1 text-[0.6875rem]">
                    <For each={["blocks", "files"] as const}>
                      {(v) => (
                        <button
                          class="rounded px-2 py-0.5"
                          classList={{
                            "bg-accent/15 text-fg": view() === v,
                            "text-fg-muted hover:bg-bg-muted": view() !== v,
                          }}
                          onClick={() => setView(v)}
                        >
                          {v === "blocks" ? d().conflictViewBlocks() : d().conflictViewFiles()}
                        </button>
                      )}
                    </For>
                    <Show when={noBaseInMarkers() && view() === "blocks"}>
                      <span class="truncate text-fg-subtle" title={d().conflictNoBase()}>
                        {d().conflictNoBase()}
                      </span>
                    </Show>
                  </div>
                  <div class="min-h-0 flex-1 overflow-auto">
                    <Show when={view() === "blocks"} fallback={<WholeFiles file={f()} />}>
                      <Show
                        when={!parseProblem()}
                        fallback={<div class="px-4 py-3 text-xs text-warn">{parseProblem()}</div>}
                      >
                        <div class="space-y-3 p-3">
                          <For each={conflicts()}>
                            {(region) => (
                              <Block
                                region={region}
                                total={conflicts().length}
                                decision={decisions()[region.index] ?? null}
                                current={current() === region.index}
                                how={how(decisions()[region.index])}
                                ref={(el) => (blockEls[region.index] = el)}
                                onDecide={(dec) => decide(region.index, dec)}
                                onPick={(p) => pick(region, p)}
                                onFocus={() => setCurrent(region.index)}
                              />
                            )}
                          </For>
                        </div>
                      </Show>
                    </Show>
                  </div>

                  {/* Bottom: the result */}
                  <div class="flex h-[42%] min-h-0 shrink-0 flex-col border-t border-border">
                    <div class="flex items-center gap-2 px-4 py-1 text-[0.6875rem] text-fg-muted">
                      <span>{d().conflictResult()}</span>
                      <Show when={dirty()}>
                        <span class="text-warn">· {d().conflictUnsaved()}</span>
                      </Show>
                    </div>
                    <textarea
                      ref={(el) => {
                        ta = el;
                        el.value = result();
                      }}
                      class="min-h-0 flex-1 resize-none border-0 bg-bg-subtle px-3 py-1 font-mono text-xs leading-normal text-fg outline-none"
                      wrap="off"
                      spellcheck={false}
                      style={{ "tab-size": "4" }}
                      onBeforeInput={onBeforeInput}
                      onInput={(e) => typed(e.currentTarget.value)}
                    />
                  </div>
                </div>
              </Show>
            </Show>
          )}
        </Show>

        <div class="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-border px-3 py-1 text-[0.6875rem] text-fg-subtle">
          <span class="ml-auto">{d().conflictKeys()}</span>
        </div>
      </div>
    </div>
  );
}

/**
 * One conflict block: its state, the decision buttons, and the sides as
 * columns of clickable lines. A taken line carries its place in the result.
 */
function Block(props: {
  region: ConflictRegion;
  total: number;
  decision: Decision | null;
  current: boolean;
  how: string;
  ref: (el: HTMLElement) => void;
  onDecide: (d: Decision | null) => void;
  onPick: (p: Pick) => void;
  onFocus: () => void;
}) {
  const picks = createMemo(() => asPicks(props.region, props.decision));
  const orderOf = (side: Side, index: number): number | null => {
    const at = picks().findIndex((p) => p.side === side && p.index === index);
    return at >= 0 ? at + 1 : null;
  };
  const sides = (): Side[] => (props.region.base !== null ? ["ours", "base", "theirs"] : ["ours", "theirs"]);
  const title = (s: Side) =>
    s === "ours" ? d().conflictOurs() : s === "base" ? d().conflictBase() : d().conflictTheirs();
  const label = (s: Side) =>
    s === "ours" ? props.region.oursLabel : s === "base" ? props.region.baseLabel : props.region.theirsLabel;

  return (
    <div
      ref={props.ref}
      class="rounded border"
      classList={{
        "border-accent": props.current,
        "border-border": !props.current,
      }}
      onMouseDown={props.onFocus}
    >
      <div class="flex flex-wrap items-center gap-2 border-b border-border bg-bg-muted px-2 py-1 text-[0.6875rem]">
        <span class="font-semibold">{d().conflictBlock(props.region.index + 1, props.total)}</span>
        <span classList={{ "text-danger": !props.decision, "text-success": !!props.decision }}>{props.how}</span>
        <div class="ml-auto flex flex-wrap gap-1">
          <Btn small label={d().conflictTakeOurs()} onClick={() => props.onDecide({ kind: "ours" })} />
          <Btn small label={d().conflictTakeTheirs()} onClick={() => props.onDecide({ kind: "theirs" })} />
          <Btn small label={d().conflictBothOT()} onClick={() => props.onDecide({ kind: "both", first: "ours" })} />
          <Btn small label={d().conflictBothTO()} onClick={() => props.onDecide({ kind: "both", first: "theirs" })} />
          <Btn
            small
            label={d().conflictResetBlock()}
            title={d().conflictResetBlockTip()}
            disabled={!props.decision}
            onClick={() => props.onDecide(null)}
          />
        </div>
      </div>
      <div class="grid" style={{ "grid-template-columns": `repeat(${sides().length}, minmax(0, 1fr))` }}>
        <For each={sides()}>
          {(s) => (
            <div class="min-w-0 border-r border-border last:border-r-0">
              <div class="truncate border-b border-border px-2 py-0.5 text-[0.625rem] uppercase text-fg-muted" title={label(s) ?? ""}>
                {title(s)}
                <Show when={label(s)}>
                  <span class="ml-1 normal-case text-fg-subtle">{label(s)}</span>
                </Show>
              </div>
              <div class="overflow-x-auto font-mono text-xs">
                <Show
                  when={sideLines(props.region, s).length > 0}
                  fallback={<div class="px-2 py-0.5 text-fg-subtle">{d().conflictEmptySide()}</div>}
                >
                  <For each={sideLines(props.region, s)}>
                    {(line, i) => {
                      const order = () => orderOf(s, i());
                      return (
                        <div
                          class="flex cursor-pointer items-start gap-1 pr-2"
                          classList={{
                            "bg-accent/15": order() !== null,
                            "hover:bg-bg-muted": order() === null,
                          }}
                          title={d().conflictPickTip()}
                          onClick={() => props.onPick({ side: s, index: i() })}
                        >
                          <span class="w-6 shrink-0 select-none text-right text-[0.625rem] text-accent">
                            {order() ?? ""}
                          </span>
                          <span class="whitespace-pre">{line || " "}</span>
                        </div>
                      );
                    }}
                  </For>
                </Show>
              </div>
            </div>
          )}
        </For>
      </div>
    </div>
  );
}

/** The three sides as the index holds them, whole — read-only. */
function WholeFiles(props: { file: ConflictFile }) {
  const cols = () =>
    (
      [
        ["ours", props.file.ours],
        ["base", props.file.base],
        ["theirs", props.file.theirs],
      ] as const
    ).filter(([s, side]) => s !== "base" || side !== null);
  const title = (s: Side) =>
    s === "ours" ? d().conflictOurs() : s === "base" ? d().conflictBase() : d().conflictTheirs();
  const note = (side: ConflictSide | null): string | null => {
    if (!side) return d().conflictSideAbsent();
    if (side.mode === "160000") return d().conflictSubmodule();
    if (side.mode === "120000") return d().conflictSymlink();
    if (side.blocked) return blockText(side.blocked);
    return null;
  };
  return (
    <div class="grid min-h-0 flex-1" style={{ "grid-template-columns": `repeat(${cols().length}, minmax(0, 1fr))` }}>
      <For each={cols()}>
        {([s, side]) => (
          <div class="flex min-h-0 min-w-0 flex-col border-r border-border last:border-r-0">
            <div class="border-b border-border px-2 py-0.5 text-[0.625rem] uppercase text-fg-muted">{title(s)}</div>
            <Show
              when={note(side) === null}
              fallback={<div class="px-2 py-1 text-xs text-fg-subtle">{note(side)}</div>}
            >
              <pre class="min-h-0 flex-1 overflow-auto px-2 py-1 font-mono text-xs">{side!.text}</pre>
            </Show>
          </div>
        )}
      </For>
    </div>
  );
}

/** After a resolution: the next file, or "Continue", or nothing left. */
function Finished(props: {
  next: string | null;
  onNext: (path: string) => void;
  onContinue: () => void;
  onClose: () => void;
}) {
  const op = () => state()?.operation.kind ?? "none";
  return (
    <div class="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center text-sm">
      <Show
        when={props.next}
        fallback={
          <Show
            when={op() !== "none" && op() !== "bisect"}
            fallback={<div class="text-fg-muted">{d().conflictNoneLeft()}</div>}
          >
            <div>{d().conflictAllResolved(operationWord(op()))}</div>
            <div class="flex gap-2">
              <Btn label={d().opContinue()} accent disabled={busy()} onClick={props.onContinue} />
              <Btn label={d().close()} onClick={props.onClose} />
            </div>
          </Show>
        }
      >
        {(n) => (
          <>
            <div class="font-mono">{d().conflictResolvedNext(n())}</div>
            <div class="flex gap-2">
              <Btn label={d().conflictOpenNext()} accent onClick={() => props.onNext(n())} />
              <Btn label={d().close()} onClick={props.onClose} />
            </div>
          </>
        )}
      </Show>
      <Show when={props.next === null && op() === "none"}>
        <Btn label={d().close()} onClick={props.onClose} />
      </Show>
    </div>
  );
}

function WholeBtn(props: { side: "ours" | "theirs"; present: boolean; disabled: boolean; onClick: () => void }) {
  const name = () => (props.side === "ours" ? d().conflictHowOurs() : d().conflictHowTheirs());
  return (
    <Btn
      label={
        props.present
          ? props.side === "ours"
            ? d().conflictWholeOurs()
            : d().conflictWholeTheirs()
          : d().conflictWholeDeletes(name())
      }
      title={props.present ? d().conflictWholeTip(props.side) : d().conflictWholeDeletesTip(name())}
      danger={!props.present}
      disabled={props.disabled}
      onClick={props.onClick}
    />
  );
}

function Btn(props: {
  label: string;
  title?: string;
  disabled?: boolean;
  accent?: boolean;
  danger?: boolean;
  small?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      class="shrink-0 rounded border disabled:cursor-default disabled:opacity-50"
      classList={{
        "px-1.5 py-0 text-[0.6875rem]": !!props.small,
        "px-2 py-0.5 text-xs": !props.small,
        "border-accent bg-accent/15 text-fg hover:bg-accent/25": !!props.accent,
        "border-danger text-danger hover:bg-danger/10": !!props.danger && !props.accent,
        "border-border hover:bg-bg-muted": !props.accent && !props.danger,
      }}
      title={props.title ?? props.label}
      disabled={props.disabled}
      onClick={() => props.onClick()}
    >
      {props.label}
    </button>
  );
}
