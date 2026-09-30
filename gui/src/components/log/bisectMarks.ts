/**
 * Bisect's answers drawn on the commits themselves: which commit is the bad end
 * of the range, which ones were marked good or skipped, which one is checked out
 * for the current test and — once the search is over — which one brought the bug
 * in (or, when only skipped commits were left, which ones may have).
 *
 * One mark per commit, by precedence: the answer of the search beats everything,
 * an answer the user gave beats "under test" (a commit already answered is not
 * waiting for one), and a bisect whose state did not parse draws nothing — its
 * lists are empty by contract (`BisectState.problem`), and a guess would be worse.
 *
 * No imports: `scripts/check-log-filters.mjs` transpiles this file on its own.
 */

export type BisectMarkKind = "culprit" | "candidate" | "bad" | "good" | "skip" | "testing";

/** The fields of `BisectState` (api.ts) this reads. */
export interface BisectMarkSource {
  bad: string | null;
  good: string[];
  skip: string[];
  current: string | null;
  firstBad: string | null;
  candidates: string[];
  problem?: string | null;
}

/** Mark by full hash (lower case, as git prints it). Empty when no bisect runs. */
export function bisectMarks(b: BisectMarkSource | null | undefined): Map<string, BisectMarkKind> {
  const out = new Map<string, BisectMarkKind>();
  if (!b || b.problem) return out;
  const put = (hash: string | null, kind: BisectMarkKind) => {
    if (hash && !out.has(hash.toLowerCase())) out.set(hash.toLowerCase(), kind);
  };
  const over = b.firstBad !== null || b.candidates.length > 0;
  put(b.firstBad, "culprit");
  for (const h of b.candidates) put(h, "candidate");
  put(b.bad, "bad");
  for (const h of b.good) put(h, "good");
  for (const h of b.skip) put(h, "skip");
  if (!over) put(b.current, "testing");
  return out;
}

/** What the search is waiting for, for the strip's text. */
export type BisectPhase = "broken" | "found" | "ambiguous" | "waitBoth" | "waitBad" | "waitGood" | "testing";

export function bisectPhase(b: BisectMarkSource): BisectPhase {
  if (b.problem) return "broken";
  if (b.firstBad !== null) return "found";
  if (b.candidates.length > 0) return "ambiguous";
  if (b.bad === null && b.good.length === 0) return "waitBoth";
  if (b.bad === null) return "waitBad";
  if (b.good.length === 0) return "waitGood";
  return "testing";
}

/** The repository's word for a role, or null when it is git's default — the UI
 *  then says it in its own words ("bug present") instead of git's. */
export function customTerm(term: string, fallback: "bad" | "good"): string | null {
  return term === fallback ? null : term;
}
