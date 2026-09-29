/**
 * Which lines of a diff the reader has chosen for staging, unstaging or reverting —
 * the rules only; `DiffView` holds the value and draws it.
 *
 * **No imports, and it must stay that way**: `scripts/check-log-filters.mjs`
 * transpiles this file on its own, and an import would fail module resolution in a
 * way that reads like an unrelated breakage. The line type below is structural for
 * the same reason — `api.DiffLine` fits it.
 *
 * **A selection belongs to one diff.** It carries the fingerprint (`digest`) of the
 * payload it was made in and the context that payload was asked for with; the
 * backend rebuilds the patch from the diff it reads again and refuses a different
 * one. The indexes mean nothing against any other payload, so a selection whose
 * digest is not the one on screen is empty (`forPayload`) — kept, it would stage
 * whatever lines now sit at those positions.
 *
 * Only added and removed lines can be chosen: a context line is in both versions
 * and there is nothing to move.
 */

export type SelLine = { origin: string };
export type SelHunk = { lines: SelLine[] };
/** A line by position: index into the payload's hunks, then into its lines. */
export type LineRef = { hunk: number; line: number };

export type LineSelection = {
  /** Fingerprint of the diff the lines were chosen in; `""` when nothing is. */
  digest: string;
  /** The context that diff was asked for with (`undefined`: git's default). */
  context: number | undefined;
  /** Chosen lines as `lineKey`s, unique, in document order. */
  keys: string[];
  /** Where a range starts: the last line clicked or toggled. */
  anchor: LineRef | null;
  /** Where the range currently ends — what the keyboard moves. */
  cursor: LineRef | null;
};

export const NO_SELECTION: LineSelection = {
  digest: "",
  context: undefined,
  keys: [],
  anchor: null,
  cursor: null,
};

export const lineKey = (r: LineRef): string => `${r.hunk}:${r.line}`;

const parseKey = (k: string): LineRef => {
  const [h, l] = k.split(":");
  return { hunk: Number(h), line: Number(l) };
};

const before = (a: LineRef, b: LineRef) => a.hunk - b.hunk || a.line - b.line;

const sameRef = (a: LineRef | null, b: LineRef | null) =>
  !!a && !!b && a.hunk === b.hunk && a.line === b.line;

/** Can this line be chosen? Additions and removals only. */
export const selectable = (l: SelLine | undefined): boolean =>
  !!l && (l.origin === "+" || l.origin === "-");

/** Every line that can be chosen, in document order. */
export function selectableRefs(hunks: SelHunk[]): LineRef[] {
  const out: LineRef[] = [];
  hunks.forEach((h, hunk) =>
    h.lines.forEach((l, line) => {
      if (selectable(l)) out.push({ hunk, line });
    }),
  );
  return out;
}

const valid = (hunks: SelHunk[], r: LineRef) => selectable(hunks[r.hunk]?.lines[r.line]);

/** The selection as it applies to the diff now on screen: itself, or nothing. */
export function forPayload(sel: LineSelection, digest: string): LineSelection {
  return digest !== "" && sel.digest === digest ? sel : NO_SELECTION;
}

function withKeys(
  base: LineSelection,
  keys: Iterable<string>,
  digest: string,
  context: number | undefined,
  anchor: LineRef | null,
  cursor: LineRef | null,
): LineSelection {
  const sorted = [...new Set(keys)].map(parseKey).sort(before).map(lineKey);
  if (sorted.length === 0) return NO_SELECTION;
  // The context is the one of the payload the first line was chosen in; a later
  // choice on the same digest describes the same diff.
  const ctx = base.keys.length > 0 ? base.context : context;
  return { digest, context: ctx, keys: sorted, anchor, cursor };
}

/** Lines that can be chosen from `a` to `b`, both included, in either order. */
function range(hunks: SelHunk[], a: LineRef, b: LineRef): string[] {
  const [lo, hi] = before(a, b) <= 0 ? [a, b] : [b, a];
  return selectableRefs(hunks)
    .filter((r) => before(r, lo) >= 0 && before(r, hi) <= 0)
    .map(lineKey);
}

/** A click: choose the line or give it back; it becomes the anchor of a range. */
export function toggle(
  sel: LineSelection,
  hunks: SelHunk[],
  ref: LineRef,
  digest: string,
  context: number | undefined,
): LineSelection {
  if (!valid(hunks, ref)) return sel;
  const base = forPayload(sel, digest);
  const k = lineKey(ref);
  const keys = new Set(base.keys);
  if (keys.has(k)) keys.delete(k);
  else keys.add(k);
  return withKeys(base, keys, digest, context, ref, ref);
}

/**
 * A shift-click, or a keyboard step: the range from the anchor now ends at `ref`.
 *
 * The range it replaces is taken back first, the way a text editor's shift
 * selection works — stepping back over a line un-chooses it. Lines chosen outside
 * that range stay chosen. With no anchor it is a plain choice of the line.
 */
export function extendTo(
  sel: LineSelection,
  hunks: SelHunk[],
  ref: LineRef,
  digest: string,
  context: number | undefined,
): LineSelection {
  if (!valid(hunks, ref)) return sel;
  const base = forPayload(sel, digest);
  const anchor = base.anchor && valid(hunks, base.anchor) ? base.anchor : null;
  if (!anchor) return withKeys(base, [...base.keys, lineKey(ref)], digest, context, ref, ref);
  const keys = new Set(base.keys);
  if (base.cursor && !sameRef(base.cursor, anchor))
    for (const k of range(hunks, anchor, base.cursor)) keys.delete(k);
  for (const k of range(hunks, anchor, ref)) keys.add(k);
  return withKeys(base, keys, digest, context, anchor, ref);
}

/**
 * The keyboard: move the end of the range to the next (`+1`) or previous (`-1`)
 * line that can be chosen. With nothing chosen yet, the first line in that
 * direction from `from` (the difference on screen) is chosen, or the first / last
 * of the file. It stops at the ends rather than wrapping.
 */
export function step(
  sel: LineSelection,
  hunks: SelHunk[],
  dir: 1 | -1,
  digest: string,
  context: number | undefined,
  from?: LineRef | null,
): LineSelection {
  const refs = selectableRefs(hunks);
  if (refs.length === 0) return forPayload(sel, digest);
  const base = forPayload(sel, digest);
  const cursor = base.cursor && valid(hunks, base.cursor) ? base.cursor : null;
  if (!cursor) {
    const start =
      (from && (dir > 0 ? refs.find((r) => before(r, from) >= 0) : [...refs].reverse().find((r) => before(r, from) <= 0))) ??
      (dir > 0 ? refs[0] : refs[refs.length - 1]);
    return withKeys(base, [...base.keys, lineKey(start)], digest, context, start, start);
  }
  const i = refs.findIndex((r) => sameRef(r, cursor));
  const next = refs[Math.min(refs.length - 1, Math.max(0, i + dir))];
  return extendTo(base, hunks, next, digest, context);
}

/** What the backend is sent: chosen lines grouped by hunk, both in order. */
export function picks(sel: LineSelection): { hunk: number; lines: number[] }[] {
  const out: { hunk: number; lines: number[] }[] = [];
  for (const r of sel.keys.map(parseKey)) {
    const last = out[out.length - 1];
    if (last && last.hunk === r.hunk) last.lines.push(r.line);
    else out.push({ hunk: r.hunk, lines: [r.line] });
  }
  return out;
}
