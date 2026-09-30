/**
 * The age of a commit as a step on a fixed scale — the "by age" colouring of the
 * log graph: the commit's node and its date take the step's colour, the lines
 * keep their lanes'.
 *
 * The scale is **absolute**: age against the clock, not a rank among the rows
 * loaded. A relative scale would repaint every row the moment an older page
 * arrives, and "old" would mean something else in each repository. That is also
 * why this is not `blameRules.ageLevels`: blame ranks the dates of one file on
 * purpose, where the set is closed and small; the log is paged and open-ended.
 *
 * The date is the **author** date — the one the row shows. Steps are colours by
 * CSS variables (`--age-0` … `--age-4` in `styles.css`, both themes); this
 * module only says which step.
 *
 * No imports: `scripts/check-log-filters.mjs` transpiles this file on its own.
 */

/** The steps, freshest first: `maxDays` is exclusive. The last one is open. */
export const AGE_STEPS = [
  { key: "day", maxDays: 1 },
  { key: "week", maxDays: 7 },
  { key: "month", maxDays: 31 },
  { key: "year", maxDays: 365 },
  { key: "older", maxDays: Infinity },
] as const;

export type AgeKey = (typeof AGE_STEPS)[number]["key"];

const DAY_S = 86_400;

/**
 * The step of a commit authored at `authorAt` (Unix seconds) as of `nowMs`
 * (milliseconds, `Date.now()`), or null when the date is not one — an unknown
 * age claims no colour. A date in the future is a skewed clock, not a fact: it
 * counts as the freshest.
 */
export function ageStep(authorAt: number, nowMs: number): number | null {
  if (!Number.isFinite(authorAt) || authorAt <= 0 || !Number.isFinite(nowMs)) return null;
  const days = (nowMs / 1000 - authorAt) / DAY_S;
  if (!(days > 0)) return 0;
  return AGE_STEPS.findIndex((s) => days < s.maxDays);
}

/** The colour of a step for an SVG attribute or a style: a theme variable. */
export const ageColor = (step: number) => `rgb(var(--age-${step}))`;
