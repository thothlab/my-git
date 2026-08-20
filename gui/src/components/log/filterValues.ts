/**
 * Values the filter bar converts between what a control shows and what
 * `LogFilter` carries: day boundaries for the date filter, repository-relative
 * paths for the path filter.
 *
 * Pure and exported for the same reason as `searchPattern.ts`: these are the
 * functions the bar itself calls, so checking them checks the panel and not a
 * copy of it. Nothing here reads a signal or touches the DOM.
 */

export const DAY = 86_400;

/**
 * Local midnight of today, in seconds. Local, not UTC: "today" is the reader's
 * day, and git is given an absolute instant (`--since=@<unix>`) either way.
 */
export const startOfToday = (now: Date = new Date()): number =>
  Math.floor(new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime() / 1000);

/** Start of the day named by a `YYYY-MM-DD` value, in seconds, local. */
export const dayStart = (v: string): number => Math.floor(new Date(`${v}T00:00:00`).getTime() / 1000);

/** End of that day: the day a reader names is meant whole, so an upper bound is
 * its last second and not its midnight — otherwise "to the 20th" drops the 20th. */
export const dayEnd = (v: string): number => Math.floor(new Date(`${v}T23:59:59`).getTime() / 1000);

/**
 * The inverse of {@link dayStart}, for `<input type="date">`. Built from local
 * parts, not from `toISOString()`: formatting through UTC shifts the day for
 * every reader east or west of Greenwich, so reopening the menu would show the
 * day before the one applied and every re-apply would walk the window back.
 */
export function asInputDate(t: number | null): string {
  if (t === null) return "";
  const dt = new Date(t * 1000);
  const p2 = (n: number) => String(n).padStart(2, "0");
  return `${dt.getFullYear()}-${p2(dt.getMonth() + 1)}-${p2(dt.getDate())}`;
}

/** Path separators as git wants them. The file dialog answers in the platform's
 * form — backslashes on Windows, where the project also ships a release — and a
 * comparison against a forward-slash root would then declare every picked path
 * to be outside the repository. */
export const toSlash = (p: string): string => p.replace(/\\/g, "/");

/** A path in Windows form, recognised by its drive letter. Used only to decide
 * how to compare, never to rewrite the path itself. */
const isWindowsPath = (p: string): boolean => /^[a-zA-Z]:/.test(p);

/**
 * Repository-relative form of a path the file dialog returned, or null when it
 * lies outside the repository: git would reject it, and a filter that selects
 * nothing reads as a broken filter rather than as a rejected path.
 *
 * The root is compared case-insensitively where the file system is, and only
 * there. Windows hands back the drive letter in either case (`C:\p` and `c:\p`
 * name one directory), so an exact comparison declares every picked path to be
 * outside the repository. On a case-sensitive file system the opposite is true:
 * folding case would accept `/home/u/Repo` for `/home/u/repo` and hand git a
 * path that does not exist. `ignoreCase` overrides the guess for a caller that
 * knows better — a case-insensitive macOS volume, say.
 *
 * The relative part keeps the case it arrived with: it is a path git has to
 * match, not a label.
 */
export function relativeToRepo(
  repo: string,
  abs: string,
  opts: { ignoreCase?: boolean } = {},
): string | null {
  const root = toSlash(repo).replace(/\/+$/, "");
  const p = toSlash(abs).replace(/\/+$/, "");
  if (!root) return null;
  const fold = opts.ignoreCase ?? (isWindowsPath(root) || isWindowsPath(p));
  // Folding case changes no lengths, so `root.length` still cuts in the right place.
  const cmpRoot = fold ? root.toLowerCase() : root;
  const cmpPath = fold ? p.toLowerCase() : p;
  if (cmpPath === cmpRoot) return ".";
  if (!cmpPath.startsWith(`${cmpRoot}/`)) return null;
  return p.slice(root.length + 1);
}
