import type { LogCommit } from "../../api";
import { search } from "../../logStore";
import { compilePattern, matchesCommit, spansIn, type Pattern, type Span } from "./searchPattern";

/**
 * Reactive skin over `searchPattern.ts`: reads the `search` signal and asks the
 * one compiler. **No rule lives here** — every function below is one line over
 * the pure module, so what a harness checks there is what the panel runs.
 *
 * Search and filters are different mechanisms and must not meet in one
 * predicate (PRD prd_02, `log/spec.md` «Text search in the log»):
 *
 *  1. **Highlight** — {@link matchRanges}: where inside a subject the search
 *     text sits. Changes how a row reads, never whether it is there.
 *  2. **Dim** — {@link commitMatches}: whether a row matched at all, for
 *     showing non-matching rows muted. Still narrows nothing, and answers with
 *     the same rule the jump between matches uses.
 *  3. **Narrow** — not here, and not from the search field. Narrowing is
 *     `LogFilter` (branch, author, date, paths) applied by the store; the
 *     search field never writes into it.
 *
 * The search toggles (`.*`, `Cc`) live in `search()` alone and must not be
 * copied into `LogFilter.regex` / `LogFilter.matchCase`: those flags also govern
 * how git matches `--author` (`engine/log.rs`), so writing them from the search
 * field would silently change the *row set* from a control that claims to change
 * only highlighting.
 */

export type { Pattern, Span };

/** The compiled current search. Reactive: reads the `search` signal, so a
 * component calling it inside JSX re-renders when the text or a toggle changes. */
export const searchPattern = (): Pattern => compilePattern(search());

/** Where the search text occurs inside `text`. */
export const matchRanges = (text: string): Span[] => spansIn(searchPattern(), text);

/** Whether a commit matched the search. */
export const commitMatches = (commit: LogCommit): boolean =>
  matchesCommit(searchPattern(), commit.subject, commit.hash);
