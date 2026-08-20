/**
 * The search pattern of the log, compiled once and asked by everyone.
 *
 * Pure on purpose: nothing here reads a signal, so the rule the application runs
 * is the rule a harness can run, and the same functions serve highlighting
 * (`spansIn`), the dim predicate and the jump between matches (`matchesCommit`).
 * `searchMatch.ts` is the reactive skin over this file and adds no rule of its
 * own — a second rule is exactly what makes a jump land on a row that the
 * highlighting does not consider matched.
 *
 * **The rule, in one place** (`matchesCommit`): a subject matches by the
 * pattern; a hash matches by the same pattern in regular-expression mode and by
 * prefix in plain mode, which is how a reader pastes one.
 *
 * `logStore.matcher()` still restates this rule — the store belongs to another
 * task's zone and cannot be edited from here. The two are kept equal on purpose;
 * when the store is in scope it must call `matchesCommit` instead of repeating
 * it (TODO(prd): один компилятор шаблона, задача вне зоны 11).
 */

/** Half-open `[start, end)` range inside a subject. */
export type Span = [number, number];

export interface SearchQuery {
  text: string;
  regex: boolean;
  matchCase: boolean;
}

export type Pattern =
  /** Nothing typed: nothing matches, nothing is highlighted. */
  | { kind: "none" }
  | {
      kind: "ok";
      /** Does this text contain the pattern? */
      test: (text: string) => boolean;
      /** Where it occurs, for highlighting. */
      ranges: (text: string) => Span[];
      /** The plain-mode needle, or null in regex mode. Prefix-matching a hash is
       * a plain-mode rule; in regex mode the hash is matched by the pattern. */
      plainNeedle: string | null;
    }
  /** A regular expression the engine refused, with its reason. */
  | { kind: "error"; message: string };

/**
 * Last compilation, keyed by the query it came from. The pattern is asked once
 * per visible row per render, and recompiling a regular expression that often is
 * a cost paid for nothing.
 */
let cacheKey: string | null = null;
let cached: Pattern = { kind: "none" };

const keyOf = (q: SearchQuery) => `${q.regex ? "r" : "-"}${q.matchCase ? "c" : "-"} ${q.text}`;

/** Compile a query. Memoised by text and flags, not by call site. */
export function compilePattern(q: SearchQuery): Pattern {
  const key = keyOf(q);
  if (key === cacheKey) return cached;
  cacheKey = key;
  cached = build(q);
  return cached;
}

function build(q: SearchQuery): Pattern {
  if (!q.text) return { kind: "none" };

  if (q.regex) {
    let re: RegExp;
    try {
      re = new RegExp(q.text, q.matchCase ? "g" : "gi");
    } catch (e) {
      // The store's matcher swallows this and simply refuses to jump; the field
      // is the only place the reader is told what is wrong with the pattern.
      return { kind: "error", message: e instanceof Error ? e.message : String(e) };
    }
    return {
      kind: "ok",
      // `test` on a /g/ regex is stateful, so the position is reset every time.
      test: (text) => {
        re.lastIndex = 0;
        return re.test(text);
      },
      ranges: (text) => regexSpans(re, text),
      plainNeedle: null,
    };
  }

  const needle = q.matchCase ? q.text : q.text.toLowerCase();
  const hay = (text: string) => (q.matchCase ? text : text.toLowerCase());
  return {
    kind: "ok",
    test: (text) => hay(text).includes(needle),
    ranges: (text) => plainSpans(hay(text), needle),
    plainNeedle: needle,
  };
}

/**
 * Spans of the pattern inside a text. Empty for "nothing typed" and for a
 * pattern that did not compile — a broken pattern leaves the rows as they were
 * rather than blanking their highlighting.
 */
export function spansIn(p: Pattern, text: string): Span[] {
  return p.kind === "ok" ? p.ranges(text) : [];
}

/**
 * Whether a commit matches: the single rule behind both the dim predicate and
 * the jump between matches. Not a narrowing predicate — no caller of this
 * function removes rows from the log.
 */
export function matchesCommit(p: Pattern, subject: string, hash: string): boolean {
  if (p.kind !== "ok") return false;
  if (p.test(subject)) return true;
  return p.plainNeedle === null ? p.test(hash) : hash.startsWith(p.plainNeedle.toLowerCase());
}

function plainSpans(hay: string, needle: string): Span[] {
  const out: Span[] = [];
  let i = hay.indexOf(needle);
  while (i >= 0) {
    out.push([i, i + needle.length]);
    i = hay.indexOf(needle, i + needle.length);
  }
  return out;
}

function regexSpans(re: RegExp, text: string): Span[] {
  const out: Span[] = [];
  re.lastIndex = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    // A pattern that can match nothing (`a*`) never advances `lastIndex` on its
    // own: without this the loop hangs the window on the first empty match.
    if (m[0].length === 0) {
      re.lastIndex++;
      continue;
    }
    out.push([m.index, m.index + m[0].length]);
  }
  return out;
}
