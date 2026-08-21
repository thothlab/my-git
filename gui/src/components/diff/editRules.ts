/**
 * The pure rules behind editing the working-tree side of a comparison (prd_03):
 * when editing is offered at all, how a draft moves between "typed", "being
 * written" and "on disk", and the two text measurements the editor's gutter and
 * its caret placement need.
 *
 * **This module imports nothing, and must keep importing nothing.**
 * `scripts/check-log-filters.mjs` *transpiles* its entry points, it does not
 * bundle them, so a single `import` from `../../api` turns the harness into a
 * module-resolution failure that reads like an unrelated breakage. The reactive
 * wrapper and every call to the backend live in `editState.ts`.
 */

/** Why a working-tree file cannot be edited in place (mirrors `EditBlock` in model.rs). */
export type EditBlock = "binary" | "too-large" | "mixed-eol" | "missing";

/**
 * Why the editing control cannot be used. `null` from `editAvailability` means
 * it can. Never a boolean: a disabled control in this project has to name its
 * reason, and a key is the only shape that survives the trip through `i18n`.
 */
export type EditUnavailable = "unified" | "read-only" | "loading" | EditBlock;

/** Pause after the last keystroke before the draft goes to disk on its own. */
export const AUTOSAVE_MS = 800;

export interface EditConditions {
  /** The panel is in the side-by-side view. */
  split: boolean;
  /** `sideLabels(...).right.readOnly` — the existing answer to "is this side
   * the working tree", and deliberately not a fourth predicate. */
  readOnly: boolean;
  /** The file is still being read, so `blocked` is not known yet. */
  loading: boolean;
  blocked: EditBlock | null;
}

/**
 * The three conditions of PRD §"Когда правка предлагается", in that order.
 * Ordered rather than combined so the reason shown is the one nearest to what
 * the reader is looking at: in the unified view of a commit, "editing is offered
 * side by side" is the actionable half.
 */
export function editAvailability(c: EditConditions): EditUnavailable | null {
  if (!c.split) return "unified";
  if (c.readOnly) return "read-only";
  if (c.loading) return "loading";
  return c.blocked;
}

/**
 * The draft of one file: what is on screen, what is known to be on disk, and
 * the text of the write currently in flight (`null` when none is).
 *
 * `writing` is the text that was *sent*, not the text at the moment it lands:
 * the user keeps typing while the write travels, and marking the draft clean
 * against the later text would drop everything typed in between.
 */
export interface Draft {
  text: string;
  saved: string;
  writing: string | null;
}

export type DraftEvent =
  /** A keystroke. */
  | { kind: "type"; text: string }
  /** A write of the current text has just been started. */
  | { kind: "sent" }
  /** The write in flight succeeded. */
  | { kind: "ok" }
  /** The write in flight failed; the typed text stays, disk is unchanged. */
  | { kind: "fail" }
  /** The file was read from disk (opened, or reread after an outside change). */
  | { kind: "synced"; text: string };

export const emptyDraft = (): Draft => ({ text: "", saved: "", writing: null });

/** Is there anything typed that the file on disk does not have? */
export const draftDirty = (d: Draft) => d.text !== d.saved;

/**
 * May a write start right now? Only when something is unsaved *and* nothing is
 * in flight — two writes of the same file at once would race, and the second
 * would carry a digest the first is about to make stale.
 */
export const draftShouldWrite = (d: Draft) => d.writing === null && d.text !== d.saved;

export function draftReduce(d: Draft, e: DraftEvent): Draft {
  switch (e.kind) {
    case "type":
      return { ...d, text: e.text };
    case "sent":
      return { ...d, writing: d.text };
    case "ok":
      return { text: d.text, saved: d.writing ?? d.saved, writing: null };
    case "fail":
      return { ...d, writing: null };
    case "synced":
      return { text: e.text, saved: e.text, writing: null };
  }
}

/**
 * Lines a textarea shows for this text, which is the number the gutter counts
 * out. A trailing newline opens a further, empty line — the caret can sit on it
 * — so "a\n" is two lines and "" is one.
 */
export function countLines(text: string): number {
  let n = 1;
  for (let i = 0; i < text.length; i++) if (text[i] === "\n") n++;
  return n;
}

/**
 * Offset of the first character of `line` (1-based), for placing the caret from
 * a double-click on a drawn row. A line past the end clamps to the end of the
 * text rather than returning -1: the row numbering comes from a diff that may
 * describe a file the draft has since shortened.
 */
export function lineStartOffset(text: string, line: number): number {
  if (line <= 1) return 0;
  let at = 0;
  for (let n = 1; n < line; n++) {
    const nl = text.indexOf("\n", at);
    if (nl < 0) return text.length;
    at = nl + 1;
  }
  return at;
}
