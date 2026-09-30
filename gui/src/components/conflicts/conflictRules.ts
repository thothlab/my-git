/**
 * The pure rules of the conflict editor (R05e): reading git's conflict markers,
 * assembling a result from per-block decisions, the editor's own undo history and
 * the navigation between blocks.
 *
 * **This module imports nothing and must not start to.** The harness
 * (`scripts/check-log-filters.mjs`) transpiles it rather than bundling it, so one
 * `import` from `../../api` would fail as a module-resolution error that reads like
 * an unrelated breakage — the same rule `diff/editRules.ts` lives by.
 *
 * The model the editor keeps, and why:
 *
 *  - **`base` — a text with markers — plus one decision per marked block.** The
 *    result on screen is `buildResult(parse(base), decisions)`. A block with no
 *    decision assembles back to its markers exactly as written, so
 *    `buildResult(parse(t), [])` is `t` byte for byte. Keeping the markers in
 *    `base` is what lets a click on a line *add that line to the result at once*
 *    and a second click add the next one: the block is still addressable, because
 *    nothing has been cut out of `base` yet.
 *  - **Typing in the result is absorbed into the block it touched** when the edit
 *    lies wholly inside one decided block (`absorbEdit` → a `manual` decision), and
 *    otherwise **baked**: the typed text becomes the new `base` and the decisions
 *    start over. Blocks still marked in it are the ones left to resolve.
 *  - **Strict parsing.** A nested, unterminated or half-written block is an error
 *    with a line number, never a guess: a region misread as common text would be
 *    written to disk with its markers still in it and called resolved.
 *  - **A bare `=======` outside a block is content**, not a marker — it is a
 *    Markdown/reST underline, and treating it as leftover would warn on every
 *    README. Stray `<<<<<<<`, `|||||||` and `>>>>>>>` are errors.
 */

export type Side = "ours" | "base" | "theirs";

export interface CommonRegion {
  kind: "common";
  lines: string[];
  /** 0-based line of the first line in the parsed text. */
  start: number;
}

export interface ConflictRegion {
  kind: "conflict";
  /** 0-based position among the blocks of this text. */
  index: number;
  ours: string[];
  /** Only when git wrote it (`merge.conflictStyle` `diff3` or `zdiff3`). */
  base: string[] | null;
  theirs: string[];
  oursLabel: string;
  baseLabel: string | null;
  theirsLabel: string;
  /** 0-based lines of the opening and the closing marker. */
  start: number;
  end: number;
  /** The block as written, markers included — what an undecided block assembles to. */
  raw: string[];
}

export type Region = CommonRegion | ConflictRegion;

export type ParseFailure =
  | "nested"
  | "unterminated"
  | "no-separator"
  | "stray-base"
  | "stray-separator"
  | "stray-closing"
  | "marker-size";

export interface ParseError {
  reason: ParseFailure;
  /** 1-based line the reader should look at. */
  line: number;
  /** `marker-size`: the length of the markers that were found instead. */
  size?: number;
}

export interface ParsedOk {
  ok: true;
  regions: Region[];
  conflicts: ConflictRegion[];
  finalNewline: boolean;
}

export type Parsed = ParsedOk | { ok: false; error: ParseError };

/** git's marker length when `conflict-marker-size` is not set. */
export const DEFAULT_MARKER_SIZE = 7;

/** Lines of `text` (CRLF read as LF) and whether it ended with a line break. */
export function splitLines(text: string): { lines: string[]; finalNewline: boolean } {
  const t = text.replace(/\r\n/g, "\n");
  if (t === "") return { lines: [], finalNewline: false };
  const finalNewline = t.endsWith("\n");
  const lines = t.split("\n");
  if (finalNewline) lines.pop();
  return { lines, finalNewline };
}

/** The inverse of `splitLines` (in `\n`). */
export function joinLines(lines: readonly string[], finalNewline: boolean): string {
  return lines.length === 0 ? "" : lines.join("\n") + (finalNewline ? "\n" : "");
}

/**
 * Is `line` a marker made of `size` × `ch`? Returns its label (`""` for a bare
 * one) or `null`. Exactly `size` characters, then the end of the line or a space
 * and the label — a longer run is content, as it is for git.
 */
export function markerOf(line: string, ch: string, size: number): string | null {
  if (line.length < size) return null;
  for (let i = 0; i < size; i++) if (line[i] !== ch) return null;
  if (line.length === size) return "";
  if (line[size] !== " ") return null;
  return line.slice(size + 1);
}

/**
 * Markers of another length: an opening run, a bare separator and a closing run,
 * all `n` long, `n !== size`. What an attribute changed after the merge (or never
 * reported) looks like — refused with that length rather than read as common text.
 */
function otherMarkerSize(lines: readonly string[], size: number): { size: number; line: number } | null {
  for (let i = 0; i < lines.length; i++) {
    const m = /^(<{3,})(?: |$)/.exec(lines[i]);
    if (!m || m[1].length === size) continue;
    const n = m[1].length;
    let sep = false;
    for (let j = i + 1; j < lines.length; j++) {
      if (!sep && lines[j] === "=".repeat(n)) sep = true;
      else if (sep && markerOf(lines[j], ">", n) !== null) return { size: n, line: i + 1 };
    }
  }
  return null;
}

/**
 * Split a conflicted text into common and conflict regions.
 *
 * Accepts `\n` and `\r\n` alike (the lines come back without `\r`). `size` is the
 * `conflict-marker-size` of the path.
 */
export function parseConflicts(text: string, size: number = DEFAULT_MARKER_SIZE): Parsed {
  const { lines, finalNewline } = splitLines(text);
  const regions: Region[] = [];
  const conflicts: ConflictRegion[] = [];
  let common: string[] = [];
  let commonStart = 0;
  const fail = (reason: ParseFailure, line: number): Parsed => ({ ok: false, error: { reason, line } });
  const flush = () => {
    if (common.length > 0) regions.push({ kind: "common", lines: common, start: commonStart });
    common = [];
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const oursLabel = markerOf(line, "<", size);
    if (oursLabel === null) {
      if (markerOf(line, ">", size) !== null) return fail("stray-closing", i + 1);
      if (markerOf(line, "|", size) !== null) return fail("stray-base", i + 1);
      if (common.length === 0) commonStart = i;
      common.push(line);
      continue;
    }

    const start = i;
    const ours: string[] = [];
    let base: string[] | null = null;
    let baseLabel: string | null = null;
    const theirs: string[] = [];
    let side: Side = "ours";
    let closed: string | null = null;
    for (i++; i < lines.length; i++) {
      const l = lines[i];
      if (markerOf(l, "<", size) !== null) return fail("nested", i + 1);
      const b = markerOf(l, "|", size);
      if (b !== null) {
        if (side !== "ours") return fail("stray-base", i + 1);
        base = [];
        baseLabel = b;
        side = "base";
        continue;
      }
      if (markerOf(l, "=", size) === "") {
        if (side === "theirs") return fail("stray-separator", i + 1);
        side = "theirs";
        continue;
      }
      const c = markerOf(l, ">", size);
      if (c !== null) {
        if (side !== "theirs") return fail("no-separator", i + 1);
        closed = c;
        break;
      }
      (side === "ours" ? ours : side === "base" ? base! : theirs).push(l);
    }
    if (closed === null) return fail("unterminated", start + 1);

    flush();
    const region: ConflictRegion = {
      kind: "conflict",
      index: conflicts.length,
      ours,
      base,
      theirs,
      oursLabel,
      baseLabel,
      theirsLabel: closed,
      start,
      end: i,
      raw: lines.slice(start, i + 1),
    };
    regions.push(region);
    conflicts.push(region);
  }
  flush();

  if (conflicts.length === 0) {
    const other = otherMarkerSize(lines, size);
    if (other) return { ok: false, error: { reason: "marker-size", line: other.line, size: other.size } };
  }
  return { ok: true, regions, conflicts, finalNewline };
}

// ── decisions and assembly ───────────────────────────────────────────────────

/** One line of one side, as clicked. */
export interface Pick {
  side: Side;
  index: number;
}

export type Decision =
  | { kind: "ours" }
  | { kind: "theirs" }
  | { kind: "both"; first: "ours" | "theirs" }
  /** Lines in the order they were clicked. */
  | { kind: "lines"; picks: Pick[] }
  /** Typed by hand in the result, inside this block. */
  | { kind: "manual"; lines: string[] };

export type Decisions = readonly (Decision | null | undefined)[];

export function sideLines(region: ConflictRegion, side: Side): string[] {
  return side === "base" ? (region.base ?? []) : region[side];
}

/** The lines a decision puts in place of its block. */
export function decisionLines(region: ConflictRegion, d: Decision): string[] {
  switch (d.kind) {
    case "ours":
      return [...region.ours];
    case "theirs":
      return [...region.theirs];
    case "both":
      return d.first === "ours"
        ? [...region.ours, ...region.theirs]
        : [...region.theirs, ...region.ours];
    case "lines":
      return d.picks
        .map((p) => sideLines(region, p.side)[p.index])
        .filter((l): l is string => l !== undefined);
    case "manual":
      return [...d.lines];
  }
}

const all = (side: Side, n: number): Pick[] => Array.from({ length: n }, (_, index) => ({ side, index }));

/** A decision as the ordered list of lines it takes (none for `manual`). */
export function asPicks(region: ConflictRegion, d: Decision | null | undefined): Pick[] {
  if (!d) return [];
  switch (d.kind) {
    case "ours":
      return all("ours", region.ours.length);
    case "theirs":
      return all("theirs", region.theirs.length);
    case "both":
      return d.first === "ours"
        ? [...all("ours", region.ours.length), ...all("theirs", region.theirs.length)]
        : [...all("theirs", region.theirs.length), ...all("ours", region.ours.length)];
    case "lines":
      return d.picks;
    case "manual":
      return [];
  }
}

/**
 * A click on one line of one side. A line already taken is dropped; any other is
 * appended — the result grows in click order. A whole-side decision turns into the
 * lines it took, so a click after "take ours" adds to ours rather than starting
 * over. On a hand-edited block the line is appended to the typed text. The last
 * line dropped leaves the block undecided (its markers come back).
 */
export function togglePick(region: ConflictRegion, d: Decision | null | undefined, pick: Pick): Decision | null {
  if (sideLines(region, pick.side)[pick.index] === undefined) return d ?? null;
  if (d?.kind === "manual") {
    return { kind: "manual", lines: [...d.lines, sideLines(region, pick.side)[pick.index]] };
  }
  const picks = asPicks(region, d);
  const at = picks.findIndex((p) => p.side === pick.side && p.index === pick.index);
  const next = at >= 0 ? picks.filter((_, i) => i !== at) : [...picks, pick];
  return next.length === 0 ? null : { kind: "lines", picks: next };
}

/** Where a block landed in the assembled result: lines `[from, to)`, 0-based. */
export interface Span {
  index: number;
  from: number;
  to: number;
  decided: boolean;
}

export interface Built {
  text: string;
  spans: Span[];
}

/** The result: common text as is, each block by its decision or, undecided, as written. */
export function buildResult(parsed: ParsedOk, decisions: Decisions): Built {
  const out: string[] = [];
  const spans: Span[] = [];
  for (const r of parsed.regions) {
    if (r.kind === "common") {
      out.push(...r.lines);
      continue;
    }
    const d = decisions[r.index];
    const from = out.length;
    out.push(...(d ? decisionLines(r, d) : r.raw));
    spans.push({ index: r.index, from, to: out.length, decided: !!d });
  }
  return { text: joinLines(out, parsed.finalNewline), spans };
}

/** Blocks still waiting for a decision, by index. */
export function unresolved(parsed: ParsedOk, decisions: Decisions): number[] {
  return parsed.conflicts.filter((c) => !decisions[c.index]).map((c) => c.index);
}

/**
 * An edit typed into the result, read as a decision when it lies wholly inside
 * one decided block: that block becomes `manual` with its new lines, the others
 * keep theirs. `null` when it does not — the caller then bakes the text.
 * Undecided blocks never absorb: their lines are markers, and an edit there is an
 * edit of the markers themselves.
 */
export function absorbEdit(
  parsed: ParsedOk,
  decisions: Decisions,
  built: Built,
  next: string,
): (Decision | null)[] | null {
  const a = splitLines(built.text);
  const b = splitLines(next);
  if (a.finalNewline !== b.finalNewline) return null;
  const x = a.lines;
  const y = b.lines;
  let pre = 0;
  while (pre < x.length && pre < y.length && x[pre] === y[pre]) pre++;
  let suf = 0;
  while (suf < x.length - pre && suf < y.length - pre && x[x.length - 1 - suf] === y[y.length - 1 - suf]) suf++;
  const oldTo = x.length - suf;
  const span = built.spans.find((s) => s.decided && s.from <= pre && oldTo <= s.to);
  if (!span) return null;
  const lines = y.slice(span.from, span.to + (y.length - x.length));
  const out: (Decision | null)[] = parsed.conflicts.map((c) => decisions[c.index] ?? null);
  out[span.index] = { kind: "manual", lines };
  return buildResult(parsed, out).text === next ? out : null;
}

/**
 * Where markers are left in a text about to be marked resolved, as 1-based
 * lines: every block still marked, or the line a broken one fails at. A bare
 * `=======` in common text is not one (see the module docblock).
 */
export function leftoverMarkers(text: string, size: number = DEFAULT_MARKER_SIZE): number[] {
  const p = parseConflicts(text, size);
  if (!p.ok) return [p.error.line];
  return p.conflicts.map((c) => c.start + 1);
}

/**
 * The next block from `current` in direction `dir` among `indexes` (sorted),
 * wrapping around; `null` when there is none.
 */
export function stepConflict(indexes: readonly number[], current: number, dir: 1 | -1): number | null {
  if (indexes.length === 0) return null;
  if (dir === 1) return indexes.find((i) => i > current) ?? indexes[0];
  for (let k = indexes.length - 1; k >= 0; k--) if (indexes[k] < current) return indexes[k];
  return indexes[indexes.length - 1];
}

/** Offset of the start of 0-based `line` in `text` (`\n` lines); the end if past it. */
export function lineOffset(text: string, line: number): number {
  let at = 0;
  for (let n = 0; n < line; n++) {
    const nl = text.indexOf("\n", at);
    if (nl < 0) return text.length;
    at = nl + 1;
  }
  return at;
}

// ── the editor's own undo ────────────────────────────────────────────────────

export interface History<T> {
  past: T[];
  future: T[];
}

/** Steps kept; older ones fall off. */
export const HISTORY_LIMIT = 200;
/** Keystrokes closer together than this are one undo step. */
export const TYPING_COALESCE_MS = 1000;

export function emptyHistory<T>(): History<T> {
  return { past: [], future: [] };
}

/** Record `prev` — the state before a change — as an undo step; redo is forgotten. */
export function historyRecord<T>(h: History<T>, prev: T, limit: number = HISTORY_LIMIT): History<T> {
  const past = [...h.past, prev];
  return { past: past.length > limit ? past.slice(past.length - limit) : past, future: [] };
}

export function historyUndo<T>(h: History<T>, current: T): { history: History<T>; value: T } | null {
  if (h.past.length === 0) return null;
  const value = h.past[h.past.length - 1];
  return { history: { past: h.past.slice(0, -1), future: [current, ...h.future] }, value };
}

export function historyRedo<T>(h: History<T>, current: T): { history: History<T>; value: T } | null {
  if (h.future.length === 0) return null;
  const [value, ...rest] = h.future;
  return { history: { past: [...h.past, current], future: rest }, value };
}

/**
 * Does this change join the previous undo step? Only typing does, and only in a
 * burst: a click on a block is always its own step, and so is the first
 * keystroke after one.
 */
export function coalesces(
  last: { kind: string; at: number } | null,
  kind: string,
  at: number,
): boolean {
  return !!last && last.kind === "type" && kind === "type" && at - last.at < TYPING_COALESCE_MS;
}

// ── labels ───────────────────────────────────────────────────────────────────

/** The two letters `git status` prints for each kind of conflict. */
export const CONFLICT_CODES: Record<string, string> = {
  bothModified: "UU",
  bothAdded: "AA",
  deletedByUs: "DU",
  deletedByThem: "UD",
  addedByUs: "AU",
  addedByThem: "UA",
  bothDeleted: "DD",
};
