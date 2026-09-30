/**
 * Pure rules of the clone dialog. No imports — `scripts/check-log-filters.mjs`
 * transpiles this file on its own, and an import would break its module loading.
 */

/**
 * The folder a clone of `url` gets by default: the last component of the address,
 * without a trailing slash and without `.git` — close to what `git clone` itself
 * picks, so the dialog proposes the name the reader would get in a terminal.
 *
 * - `https://h/a/b.git` → `b`, `https://h/a/b/` → `b`
 * - scp syntax `git@host:org/repo.git` → `repo`, `host:repo` → `repo`
 * - a working tree's `.git` (`/x/y/.git`) names the folder holding it → `y`
 * - a query string or fragment is not part of the name
 *
 * An empty string when nothing usable is left (`""`, `https://`, `.`): the dialog
 * then asks for a name instead of inventing one.
 */
export function folderNameFromUrl(url: string): string {
  let s = url.trim();
  // `?…` / `#…` of a web address; a local path with `#` in it is rare enough.
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(s)) s = s.replace(/[?#].*$/, "");
  s = s.replace(/[\\/]+$/, "");
  s = s.replace(/[\\/]\.git$/i, "");
  s = s.replace(/[\\/]+$/, "");
  let last = s.split(/[\\/]/).pop() ?? "";
  // scp syntax without a slash in the path (`host:repo`), or a bare `https:`.
  last = last.slice(last.lastIndexOf(":") + 1);
  last = last.replace(/\.git$/i, "").replace(/\.bundle$/i, "");
  // eslint-disable-next-line no-control-regex
  last = last.replace(/[\u0000-\u001f\u007f]/g, "").trim();
  return last === "." || last === ".." ? "" : last;
}

/** A plain `http://` address: allowed, but the dialog says it is unencrypted. */
export function isPlainHttp(url: string): boolean {
  return /^http:\/\//i.test(url.trim());
}

/**
 * The login of an http(s) address with a user name and no password
 * (`https://me@bitbucket.org/…`, as Bitbucket and Azure DevOps hand them out), or
 * null. The backend accepts such an address — a name that looks like a token it
 * refuses — and the dialog says git will ask for the password through the
 * credential helper.
 */
export function httpLogin(url: string): string | null {
  const m = /^https?:\/\/([^/?#@\s]*)@/i.exec(url.trim());
  if (!m || m[1] === "" || m[1].includes(":")) return null;
  return m[1];
}

/** `<parent>/<name>` as the dialog shows the destination, one separator between. */
export function joinDest(parent: string, name: string): string {
  if (!parent) return name;
  const sep = parent.includes("\\") && !parent.includes("/") ? "\\" : "/";
  return parent.replace(/[\\/]+$/, "") + sep + name;
}
