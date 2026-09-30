/**
 * `Co-authored-by:` trailers for the commit panel — who can be picked, and where
 * the lines go in the message.
 *
 * The rule of *where* is git's own (`interpret-trailers`, the one GitHub and
 * GitLab read the credit back with): the last paragraph of the message is a
 * trailer block when every line of it is `Token: value` — or when git itself
 * wrote one of its lines (`Signed-off-by: `, `(cherry picked from commit `) and
 * at least a quarter of it is trailers. The first paragraph is the subject and
 * never a block, which is what keeps `feat: x` from being read as one. A new
 * trailer joins an existing block; otherwise it opens its own after a blank line.
 *
 * Two deliberate differences from `interpret-trailers` on raw input, both about
 * what `git commit -m` stores: a `#` line is text (the whitespace clean-up of
 * `-m` keeps it), and leading/trailing blank lines are not part of the message.
 * And one stricter rule: a person already credited *by address* is not added
 * again even under another spelling of the name — git's `addIfDifferent` would
 * add them, and the forge would then list the same account twice.
 *
 * No imports: `scripts/check-log-filters.mjs` transpiles this file on its own.
 */

export interface Person {
  name: string;
  email: string;
}

const TRAILER_KEY = "Co-authored-by";
/** What git's `find_separator` accepts as a trailer line: a token of letters,
 * digits and `-`, optional blanks, then the separator. */
const TRAILER_LINE = /^[A-Za-z0-9-]+[ \t]*:/;
const GIT_PREFIXES = ["Signed-off-by: ", "(cherry picked from commit "];
const CO_AUTHOR_LINE = /^co-authored-by[ \t]*:(.*)$/i;

const NAME_OK = /^[^<>\r\n\0]+$/;
const EMAIL_OK = /^[^\s<>@\0]+@[^\s<>@\0]+$/;

/** A person whose trailer would come out well-formed: a name without angle
 * brackets or line breaks, one address with an `@` and no blanks. History can
 * hold worse (an empty or local-only address); such a person is not offered. */
export const isCreditable = (p: Person): boolean =>
  NAME_OK.test(p.name.trim()) && EMAIL_OK.test(p.email.trim());

export const trailerLine = (p: Person): string =>
  `${TRAILER_KEY}: ${p.name.trim()} <${p.email.trim()}>`;

const addressKey = (email: string) => email.trim().toLowerCase();
const isBlank = (line: string) => line.trim() === "";

/**
 * Whom the picker offers for `query`: creditable, not the reader (`me`, their
 * `user.email`), not chosen already, name or address containing the query —
 * addresses compared without case throughout. Order is the caller's (the most
 * prolific first).
 */
export function pickable<T extends Person>(
  candidates: readonly T[],
  query: string,
  chosen: readonly Person[],
  me: string | null | undefined,
  limit = 8,
): T[] {
  const taken = new Set(chosen.map((p) => addressKey(p.email)));
  if (me) taken.add(addressKey(me));
  const needle = query.trim().toLowerCase();
  const out: T[] = [];
  for (const p of candidates) {
    if (out.length >= limit) break;
    if (!isCreditable(p) || taken.has(addressKey(p.email))) continue;
    if (needle && !p.name.toLowerCase().includes(needle) && !p.email.toLowerCase().includes(needle)) continue;
    out.push(p);
  }
  return out;
}

/**
 * Where the last paragraph starts if it is a trailer block, else `null`.
 * `lines` has no trailing blank lines; `bodyStart` is the first line after the
 * subject paragraph. Mirrors the backward scan of git's trailer.c.
 */
function trailerBlockStart(lines: string[], bodyStart: number): number | null {
  let trailers = 0;
  let others = 0;
  let continuation = 0;
  let recognized = false;
  for (let i = lines.length - 1; i >= bodyStart; i--) {
    const line = lines[i];
    if (isBlank(line)) {
      others += continuation;
      if (recognized && trailers * 3 >= others) return i + 1;
      if (trailers > 0 && others === 0) return i + 1;
      return null;
    }
    if (GIT_PREFIXES.some((p) => line.startsWith(p))) {
      trailers++;
      continuation = 0;
      recognized = true;
    } else if (TRAILER_LINE.test(line)) {
      trailers++;
      continuation = 0;
    } else if (/^[ \t]/.test(line)) {
      continuation++;
    } else {
      others++;
      others += continuation;
      continuation = 0;
    }
  }
  // Reached the subject without a blank line: the paragraph is the subject's own.
  return null;
}

/** Addresses already credited by `Co-authored-by:` lines of the block. */
function creditedIn(lines: string[]): Set<string> {
  const out = new Set<string>();
  for (const line of lines) {
    const m = CO_AUTHOR_LINE.exec(line);
    if (!m) continue;
    const addr = /<([^<>]*)>/.exec(m[1]);
    if (addr) out.add(addressKey(addr[1]));
  }
  return out;
}

/** The message as `git commit -m` stores it — line breaks unified, outer blank
 * lines and trailing blanks gone — and where its trailer block starts. */
function readMessage(message: string): { text: string[]; block: number | null } {
  const lines = message.replace(/\r\n?/g, "\n").split("\n");
  let first = 0;
  while (first < lines.length && isBlank(lines[first])) first++;
  let end = lines.length;
  while (end > first && isBlank(lines[end - 1])) end--;
  const text = lines.slice(first, end).map((l) => l.replace(/[ \t]+$/, ""));

  let bodyStart = 0;
  while (bodyStart < text.length && !isBlank(text[bodyStart])) bodyStart++;
  return { text, block: trailerBlockStart(text, bodyStart) };
}

/**
 * The `Co-authored-by:` lines committing `message` would add for `people` — the
 * creditable ones not credited in its trailer block yet, each address once. What
 * the panel previews, so the preview never promises a line the commit leaves out.
 */
export function trailersToAdd(message: string, people: readonly Person[]): string[] {
  const { text, block } = readMessage(message);
  const credited = block === null ? new Set<string>() : creditedIn(text.slice(block));
  const added: string[] = [];
  for (const p of people) {
    const key = addressKey(p.email);
    if (!isCreditable(p) || credited.has(key)) continue;
    credited.add(key);
    added.push(trailerLine(p));
  }
  return added;
}

/**
 * `message` with a `Co-authored-by:` trailer for each of `people` not credited
 * yet (`trailersToAdd`). Unchanged when there is nobody to add. Line breaks may
 * be `\n` or `\r\n`; the result uses `\n`, which is what the commit field produces.
 */
export function withCoAuthors(message: string, people: readonly Person[]): string {
  const added = trailersToAdd(message, people);
  if (added.length === 0) return message;
  const { text, block } = readMessage(message);
  if (text.length === 0) return added.join("\n");
  const sep = block === null ? "\n\n" : "\n";
  return text.join("\n") + sep + added.join("\n");
}
