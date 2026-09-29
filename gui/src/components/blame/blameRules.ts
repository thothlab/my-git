/**
 * Pure rules behind the blame overlay (`BlamePanel.tsx`): which rows open a run
 * of one commit, how old a commit looks next to the others, whether a line can be
 * blamed further back and why not, and how wide the text column has to be.
 *
 * **No imports, and it is not to grow one**: `scripts/check-log-filters.mjs`
 * transpiles this file on its own, and an import would break that as a module
 * resolution failure, not as a failed assertion. The shapes below are structural
 * subsets of the `api.ts` types for the same reason.
 */

/** Anything with the index of its origin — a `BlameLine`. */
export type OriginRef = { origin: number };

/**
 * `true` for every row that starts a run of lines of one commit. Only such a row
 * shows the annotation; the rest of the run would repeat it line after line.
 */
export function runStarts(lines: readonly OriginRef[]): boolean[] {
  return lines.map((l, i) => i === 0 || lines[i - 1].origin !== l.origin);
}

/** How many shades the age bar has. */
export const AGE_LEVELS = 5;

/**
 * An age level per origin, `0` (oldest) to `AGE_LEVELS - 1` (newest), by the rank
 * of its date among the distinct dates of this blame — not by distance in time:
 * a file edited twice ten years ago and once yesterday would otherwise paint every
 * old line the same and never use the middle shades. An uncommitted line is the
 * newest there is.
 */
export function ageLevels(
  origins: readonly { authorAt: number; uncommitted: boolean }[],
): number[] {
  const dates = [...new Set(origins.filter((o) => !o.uncommitted).map((o) => o.authorAt))].sort(
    (a, b) => a - b,
  );
  const top = AGE_LEVELS - 1;
  return origins.map((o) => {
    if (o.uncommitted) return top;
    if (dates.length <= 1) return top;
    const rank = dates.indexOf(o.authorAt);
    return Math.round((rank / (dates.length - 1)) * top);
  });
}

/** Why "blame before this change" is not offered for a line. */
export type BeforeBlock = "boundary" | "created" | "new";

/**
 * Whether a line can be blamed in the version before its commit, and if not, the
 * reason to show next to the inactive control. `boundary` — the commit is the
 * earliest version reachable here (a root, or the edge of a shallow clone);
 * `created` — the commit created the file, so the line had no earlier version;
 * `new` — an uncommitted line of a file not committed at all yet (staged new).
 * An uncommitted line of a committed file steps back into HEAD like any other.
 */
export function beforeBlock(origin: {
  previous: { hash: string; path: string } | null;
  boundary: boolean;
  uncommitted: boolean;
}): BeforeBlock | null {
  if (origin.previous) return null;
  if (origin.uncommitted) return "new";
  return origin.boundary ? "boundary" : "created";
}

/** Columns a line takes in a monospace font, TABs expanded to `tab` stops. */
export function textColumns(text: string, tab = 8): number {
  let col = 0;
  for (const ch of text) col = ch === "\t" ? col + tab - (col % tab) : col + 1;
  return col;
}

/** The widest line, in columns — the text column's width, so the rows scroll
 * sideways together instead of each being clipped on its own. */
export function maxColumns(lines: readonly { text: string }[], tab = 8): number {
  let max = 0;
  for (const l of lines) max = Math.max(max, textColumns(l.text, tab));
  return max;
}

/**
 * Where "blame before" lands, clamped to the lines there are: the first line of
 * the range, or `null` for an empty file.
 */
export function landingLine(range: { from: number; to: number }, count: number): number | null {
  if (count <= 0) return null;
  return Math.min(Math.max(1, range.from), count);
}
