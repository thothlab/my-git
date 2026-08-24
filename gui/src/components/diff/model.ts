import type { DiffBase, DiffLine, FileDiff, Hunk, WhitespaceMode } from "../../api";
import { WORKING_TREE } from "../../api";
import { newSideAnchors } from "./editRules";

/**
 * Pure model behind the diff panel: where a comparison comes from, how its
 * lines pair up side by side, which of them form one "difference", where an
 * unchanged region is long enough to fold, and what changed *inside* a pair of
 * lines. Nothing here touches the DOM or a signal, so the shapes below are the
 * seam the panel is asserted against.
 */

/** Where the shown comparison comes from. One panel serves all three. */
export type DiffSource =
  /** working tree / index, the historical behaviour of the Changes mode */
  | { kind: "worktree"; path: string; base: DiffBase }
  /**
   * one file inside a commit, compared against its first parent. `parent` is
   * that parent's hash and travels with the source: the diff itself does not
   * carry it, and a side header must name a revision rather than derive an
   * expression from the commit's own hash. `null` means a root commit.
   */
  | { kind: "commit"; path: string; hash: string; parent: string | null }
  /** two revisions; `to === WORKING_TREE` means the working tree */
  | { kind: "compare"; path: string; from: string; to: string };

/** Intra-line emphasis (R35i). */
export type HighlightMode = "words" | "lines" | "none";

export const WHITESPACE_MODES: WhitespaceMode[] = ["none", "trailing", "all"];
export const HIGHLIGHT_MODES: HighlightMode[] = ["words", "lines", "none"];

/** Unchanged runs longer than this fold away (spec: "more than six"). */
export const FOLD_THRESHOLD = 6;
/** Kept visible on each side of a fold, so the fold has context around it. */
export const FOLD_CONTEXT = 3;
/** Above this many diff lines the file is summarised instead of rendered (R32i.3). */
export const BIG_DIFF_LINES = 5000;

export type Row = {
  left?: DiffLine;
  right?: DiffLine;
  /** index of the difference this row belongs to, or -1 for an unchanged row */
  diff: number;
  /** first row of that difference — the anchor navigation scrolls to */
  first: boolean;
  /**
   * The line of the new version this row **stands on** — `right.newNo` when the
   * row draws one, and for a row drawing only a deletion the line the text was
   * removed from. Every row has one, which is what lets the right-hand column
   * answer a double-click anywhere and `spotOnScreen` read a place off any row.
   * See `editRules.newSideAnchors` for the two ends of the run.
   */
  newAnchor: number;
};

export type Item =
  | { kind: "row"; row: Row }
  /** a folded unchanged run that is present in the payload and can be revealed */
  | { kind: "fold"; hidden: number; rows: Row[]; id: string };

export type HunkView = {
  hunk: Hunk;
  items: Item[];
};

export type DiffView = {
  hunks: HunkView[];
  /** unchanged lines git left out between hunk i-1 and hunk i, by hunk index */
  gaps: number[];
  /** number of differences in the whole file (R36i) */
  count: number;
  /** total diff lines, the budget behind the "big diff" summary */
  lines: number;
};

const isChange = (l: DiffLine | undefined) => !!l && l.origin !== " ";

/**
 * Pair deletions with additions. A run of `-` followed by a run of `+` is **one**
 * difference, not two: that is what makes a file with two edited regions report
 * "2 differences" and not four.
 */
export function toRows(lines: DiffLine[], firstDiff: number): { rows: Row[]; count: number } {
  const rows: Omit<Row, "newAnchor">[] = [];
  let dels: DiffLine[] = [];
  let adds: DiffLine[] = [];
  let count = 0;
  const flush = () => {
    const n = Math.max(dels.length, adds.length);
    if (n === 0) return;
    const idx = firstDiff + count;
    for (let i = 0; i < n; i++)
      rows.push({ left: dels[i], right: adds[i], diff: idx, first: i === 0 });
    count += 1;
    dels = [];
    adds = [];
  };
  for (const l of lines) {
    if (l.origin === "-") dels.push(l);
    else if (l.origin === "+") adds.push(l);
    else {
      flush();
      rows.push({ left: l, right: l, diff: -1, first: false });
    }
  }
  flush();
  const anchors = newSideAnchors(rows.map((r) => r.right?.newNo));
  return { rows: rows.map((r, i) => ({ ...r, newAnchor: anchors[i] })), count };
}

/** Fold unchanged runs longer than `FOLD_THRESHOLD`, keeping context around them. */
export function foldRows(rows: Row[], hunkKey: string): Item[] {
  const items: Item[] = [];
  let run: Row[] = [];
  const flush = () => {
    if (run.length === 0) return;
    if (run.length > FOLD_THRESHOLD) {
      const head = run.slice(0, FOLD_CONTEXT);
      const tail = run.slice(run.length - FOLD_CONTEXT);
      const mid = run.slice(FOLD_CONTEXT, run.length - FOLD_CONTEXT);
      for (const r of head) items.push({ kind: "row", row: r });
      items.push({
        kind: "fold",
        hidden: mid.length,
        rows: mid,
        id: `${hunkKey}:${mid[0].left?.oldNo ?? mid[0].right?.newNo ?? items.length}`,
      });
      for (const r of tail) items.push({ kind: "row", row: r });
    } else {
      for (const r of run) items.push({ kind: "row", row: r });
    }
    run = [];
  };
  for (const r of rows) {
    if (r.diff < 0) run.push(r);
    else {
      flush();
      items.push({ kind: "row", row: r });
    }
  }
  flush();
  return items;
}

const firstNo = (h: Hunk, side: "old" | "new") => {
  for (const l of h.lines) {
    const n = side === "old" ? l.oldNo : l.newNo;
    if (n != null) return n;
  }
  return null;
};
const lastNo = (h: Hunk, side: "old" | "new") => {
  for (let i = h.lines.length - 1; i >= 0; i--) {
    const n = side === "old" ? h.lines[i].oldNo : h.lines[i].newNo;
    if (n != null) return n;
  }
  return null;
};

/**
 * Unchanged lines git omitted between two hunks. Counted on both sides and the
 * larger taken: an added or deleted file numbers only one of them.
 */
export function gapBetween(prev: Hunk, next: Hunk): number {
  let gap = 0;
  for (const side of ["old", "new"] as const) {
    const a = lastNo(prev, side);
    const b = firstNo(next, side);
    if (a != null && b != null) gap = Math.max(gap, b - a - 1);
  }
  return Math.max(0, gap);
}

/** Whole-file view: rows, folds, gaps between hunks and the difference count. */
export function buildView(diff: FileDiff): DiffView {
  const hunks: HunkView[] = [];
  const gaps: number[] = [];
  let count = 0;
  let lines = 0;
  diff.hunks.forEach((h, i) => {
    const { rows, count: n } = toRows(h.lines, count);
    count += n;
    lines += h.lines.length;
    hunks.push({ hunk: h, items: foldRows(rows, String(i)) });
    gaps.push(i === 0 ? 0 : gapBetween(diff.hunks[i - 1], h));
  });
  return { hunks, gaps, count, lines };
}

export type Seg = { text: string; changed: boolean };

/** Split into words, keeping separators as their own tokens. */
function tokenize(s: string): string[] {
  return s.match(/[A-Za-z0-9_Ѐ-ӿ]+|\s+|[^A-Za-z0-9_Ѐ-ӿ\s]/g) ?? [];
}

/**
 * What changed inside a pair of lines: common prefix and suffix stay plain, the
 * middle is emphasised. One changed identifier therefore lights up alone
 * instead of the whole line.
 */
export function wordSegments(oldText: string, newText: string): { left: Seg[]; right: Seg[] } {
  const a = tokenize(oldText);
  const b = tokenize(newText);
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head++;
  let tail = 0;
  while (
    tail < a.length - head &&
    tail < b.length - head &&
    a[a.length - 1 - tail] === b[b.length - 1 - tail]
  )
    tail++;
  const build = (t: string[]): Seg[] => {
    const mid = t.slice(head, t.length - tail).join("");
    const segs: Seg[] = [];
    const pre = t.slice(0, head).join("");
    const post = t.slice(t.length - tail).join("");
    if (pre) segs.push({ text: pre, changed: false });
    if (mid) segs.push({ text: mid, changed: true });
    if (post) segs.push({ text: post, changed: false });
    return segs.length ? segs : [{ text: "", changed: false }];
  };
  return { left: build(a), right: build(b) };
}

/** A pair of changed lines is the only case word-level emphasis applies to. */
export const pairChanged = (r: Row) => isChange(r.left) && isChange(r.right);

const short = (rev: string) => (/^[0-9a-f]{7,40}$/i.test(rev) ? rev.slice(0, 8) : rev);

export type SideLabel = { text: string; readOnly: boolean };

/**
 * What each side shows (R37i, R47i). `working` is the caller's word for the
 * working tree — the only side that is not read-only; `noParent` names the
 * empty left side of a root commit.
 */
export function sideLabels(
  src: DiffSource,
  words: { working: string; index: string; head: string; noParent: string },
): { left: SideLabel; right: SideLabel } {
  if (src.kind === "commit") {
    return {
      left: { text: src.parent ? short(src.parent) : words.noParent, readOnly: true },
      right: { text: short(src.hash), readOnly: true },
    };
  }
  if (src.kind === "compare") {
    const side = (rev: string): SideLabel =>
      rev === WORKING_TREE
        ? { text: words.working, readOnly: false }
        : { text: short(rev), readOnly: true };
    return { left: side(src.from), right: side(src.to) };
  }
  const base = src.base;
  const left: SideLabel =
    base === "worktree"
      ? { text: words.index, readOnly: true }
      : { text: words.head, readOnly: true };
  const right: SideLabel =
    base === "index"
      ? { text: words.index, readOnly: true }
      : { text: words.working, readOnly: false };
  return { left, right };
}

/**
 * Do two sources name the same comparison?
 *
 * The panel's request is keyed on the source *value*, but a source is built
 * fresh by whoever selects a file — in the Log mode from the selected file plus
 * the loaded rows — so its identity changes whenever those change, and a page
 * appended at the foot of the log would otherwise re-ask git for the file the
 * reader is already looking at and blank the panel while the answer travels.
 * Compared field by field, "the same file of the same commit" stays the same
 * source no matter how many times it is rebuilt.
 */
export function sameDiffSource(a: DiffSource | null, b: DiffSource | null): boolean {
  if (a === b) return true;
  if (!a || !b || a.kind !== b.kind || a.path !== b.path) return false;
  if (a.kind === "worktree") return a.base === (b as typeof a).base;
  if (a.kind === "commit")
    return a.hash === (b as typeof a).hash && a.parent === (b as typeof a).parent;
  return a.from === (b as typeof a).from && a.to === (b as typeof a).to;
}
