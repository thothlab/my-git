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

/**
 * Where the "current difference" pointer lands once the file has been
 * republished with a different number of differences — staging, reverting or an
 * edit all replace the payload underneath it.
 *
 * The pointer is clamped against the list that now exists, not the one it was
 * chosen in: a file whose last difference was just staged has fewer, and "that
 * was the last one" would otherwise be answered about differences still above.
 * A file with none left has no current difference. Nothing chosen stays nothing
 * chosen — `-1` is not pulled up to the first difference, because arriving at a
 * new payload is not the user asking to go anywhere.
 */
export function clampCurrent(count: number, current: number): number {
  if (current < 0) return -1;
  return Math.min(current, count - 1);
}

/**
 * May the typed text be written over what the reread found? Only when the file
 * is editable, or when it is simply not there.
 *
 * `missing` is the one blockage an overwrite answers: the digest of an absent
 * file is `""`, and `""` is exactly what tells the backend to create it. Every
 * other blockage reports the same empty digest without meaning the same thing,
 * so writing with it would be refused as "changed on disk" and put the very
 * same question back on screen under a reason that is not the true one (rule 3
 * of `prd_03_interfaces.md`). Not the rule the *reread* branch asks: that one
 * needs text, and `missing` has none.
 */
export function mayOverwrite(blocked: EditBlock | null): boolean {
  return blocked === null || blocked === "missing";
}

/**
 * The parts of a hunk that decide whether two answers describe the same patch.
 * `patch` is the hunk's own text, so the lines are compared through it.
 *
 * Declared here rather than imported: `FileDiff` from `../../api` satisfies it
 * structurally, and this module imports nothing (see the header).
 */
export interface PatchHunk {
  header: string;
  patch: string;
}

/** Everything a `FileDiff` says about the file, as this rule reads it. */
export interface PatchPayload {
  path: string;
  binary: boolean;
  oldSize?: number;
  newSize?: number;
  mergeFirstParent: boolean;
  hunks: PatchHunk[];
}

/**
 * Do two answers describe the very same patch?
 *
 * The panel re-reads the file whenever a fresh `RepoState` says the world may
 * have moved - a mutation, the refresh button, the window regaining focus - and
 * most of those answers come back identical. Publishing one anyway is not wrong,
 * it is expensive: the row list is keyed by reference, so every row is rebuilt
 * and the reader's scroll position goes with it. Alt-tabbing to a terminal and
 * back would jump the diff to the top of the file.
 *
 * Two absent payloads are *not* the same: "nothing shown" is the state the first
 * answer has to replace.
 */
export function samePayload(a: PatchPayload | null, b: PatchPayload | null): boolean {
  if (!a || !b) return false;
  if (
    a.path !== b.path ||
    a.binary !== b.binary ||
    a.oldSize !== b.oldSize ||
    a.newSize !== b.newSize ||
    a.mergeFirstParent !== b.mergeFirstParent ||
    a.hunks.length !== b.hunks.length
  )
    return false;
  return a.hunks.every((h, i) => h.header === b.hunks[i].header && h.patch === b.hunks[i].patch);
}

/**
 * What the panel knows when it has to decide whether the rows already on screen
 * may stay there while a new answer is on its way.
 *
 * The two keys name a *request* — the source and the whitespace mode — and
 * deliberately not the context width: widening the context is the reader
 * staying in this file, so the payload drawn from the narrower answer is still
 * an answer about the same thing.
 */
export interface DrawGate {
  /** A request is in flight. */
  loading: boolean;
  /** The last request failed. */
  error: boolean;
  /** Request the drawn payload answers; `null` when nothing is drawn. */
  drawnKey: string | null;
  /** Request the panel would ask for as it stands. */
  requestKey: string | null;
}

/**
 * May the diff rows be drawn right now?
 *
 * The question exists because a re-read is not a departure. The panel re-reads
 * the file on every fresh `RepoState` - a staged hunk, the refresh button, the
 * window regaining focus - and the answer is usually the one already drawn.
 * Tearing the rows down for the wait empties the scrolling container, so the
 * browser clamps its `scrollTop` to zero and the reader is thrown back to the
 * first hunk; the payload comparison that follows cannot undo that, because the
 * damage is done before the answer arrives. So a reload of the *same* request
 * keeps drawing what it already drew, and only the wait for a request the drawn
 * payload does not answer - another file, another base, another whitespace mode
 * - shows the placeholder.
 *
 * A failure hides the rows whatever is drawn: a patch left standing under
 * "diff unavailable" would be read as current.
 */
export function drawRows(g: DrawGate): boolean {
  if (g.error) return false;
  if (!g.loading) return true;
  return g.drawnKey !== null && g.drawnKey === g.requestKey;
}

/**
 * How an editing session can be left.
 *
 * `blur` is the caret going somewhere else inside the window - another panel, a
 * toolbar button, a bare patch of background; `window` is the whole application
 * losing the keyboard.
 */
export type EditExit = "escape" | "toggle" | "unified" | "source" | "blur" | "window";

/**
 * Does this departure end the editing session?
 *
 * Only the ones that say so. `escape` and `toggle` are the reader asking;
 * `unified` and `source` are the editable side ceasing to exist under them.
 * All six reach here from one funnel (`exitEdit` in `DiffView`), so this is the
 * whole policy and not a second opinion beside it.
 *
 * **Losing the caret is not one of them.** Every button in the window takes the
 * focus off the textarea when it is pressed - which is why the edit toggle has
 * to defend itself with `preventDefault` - so treating a blur as a departure
 * means the refresh button, pressed constantly, drops the reader out of edit
 * mode. It is also a third detector of something already detected twice:
 * leaving for another file is `source`, switching the window mode unmounts the
 * panel and the draft survives on purpose, and anything typed is on disk within
 * `AUTOSAVE_MS` regardless. `window` is kept apart from `blur` even though both
 * answer the same today, because they are different events and a future policy
 * would need to tell them apart.
 */
export function endsEditSession(exit: EditExit): boolean {
  return exit === "escape" || exit === "toggle" || exit === "unified" || exit === "source";
}

/**
 * One drawn diff row that carries a line number of the new version, measured
 * against the top of the scrolling viewport: `top` is the row's upper edge in
 * viewport coordinates (negative once it has been scrolled past) and `height`
 * its drawn height.
 *
 * Viewport-relative rather than `offsetTop` plus `scrollTop`: `offsetTop` is
 * only the right number while the scrolling box is also the row's
 * `offsetParent`, which is true today by accident of one `absolute` and would
 * go silently wrong the day a wrapper between them becomes positioned.
 */
export interface DrawnRow {
  top: number;
  height: number;
  /** `newNo` of the row — rows without one are not offered here. */
  line: number;
}

/** The place in the file the reader is looking at: a line of the new version
 *  and how far below the top of the viewport that line was drawn. */
export interface ReadingSpot {
  line: number;
  /** Viewport-relative top of the row; negative when it is partly scrolled off. */
  offset: number;
}

/**
 * Where the reader is in the file, from the rows on screen: the topmost one
 * still visible.
 *
 * The topmost visible row and not the current difference: `current` is `-1`
 * until someone uses previous/next, so a rule that preferred it would answer
 * "start of file" in the very case this exists for — a reader who scrolled by
 * hand — while the topmost row lands next to the current difference anyway
 * whenever there is one, because jumping to it scrolled it into view.
 *
 * `rows` are the rows that *have* a `newNo`, in document order; the deletion
 * rows in between are simply not passed. Filtering before the scan gives the
 * same answer as scanning and then skipping, because "the bottom edge is below
 * the top of the viewport" is monotone in document order — once true for a row
 * it is true for every row after it.
 *
 * **An `Iterable`, not an array, and the scan stops at the answer.** That same
 * monotonicity makes every row after the first visible one irrelevant, and
 * measuring a row is not free: the caller's source measures the DOM lazily, so
 * a diff of thousands of rows costs the handful above the fold rather than a
 * `getBoundingClientRect` per row inside a click handler.
 *
 * `null` when nothing qualifies — an empty diff, a viewport filled entirely by
 * deleted lines, the summary of a big diff. The caller opens the file at its
 * beginning then; guessing the last line above the viewport would move the
 * reader in the one direction they can already see is wrong.
 */
export function readingSpot(rows: Iterable<DrawnRow>): ReadingSpot | null {
  for (const r of rows) if (r.top + r.height > 0) return { line: r.line, offset: r.top };
  return null;
}

/**
 * Scroll offset that brings **the anchor line** back to the height it was read
 * at — `spot.offset` below the top of the box, where the diff had drawn it.
 *
 * That one line, and not the picture around it. Two things differ on the way in
 * and neither is corrected here: the diff rows are drawn `leading-tight` (about
 * 15 px) while the editing surface is `LINE_PX`, so lines drift roughly a pixel
 * apart per line away from the anchor; and above the anchor the two surfaces
 * hold different content, because the deleted lines the comparison shows are
 * not in the file. Keeping one named line where the reader left it is the whole
 * promise — holding the two columns level while someone types is deliberately
 * not offered here.
 *
 * `(line - 1) * lineHeight` is the exact top of a line in the textarea only
 * because that surface is drawn for it — `wrap="off"`, so one line is one drawn
 * row, an explicit `line-height`, and no vertical padding.
 *
 * Clamped at zero: a line near the top of the file has less text above it than
 * the offset asks for, and there is nothing to scroll. The caret is on the first
 * screen either way, so it stays visible.
 */
export function editorScrollTop(spot: ReadingSpot, lineHeight: number): number {
  return Math.max(0, (spot.line - 1) * lineHeight - spot.offset);
}
