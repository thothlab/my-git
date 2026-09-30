/**
 * Web links for a commit on its hosting forge — GitHub, GitLab or Bitbucket —
 * built from the address of a git remote.
 *
 * Only the three public hosts are known. A self-hosted GitLab (or Gitea, or a
 * Bitbucket Server) cannot be recognised from its host name, and a guessed URL
 * shape would open a wrong page with confidence; an unknown host gets no link.
 * The aliases these hosts use for SSH (`ssh.github.com`, `altssh.gitlab.com`,
 * `altssh.bitbucket.org`) and `www.` map to the web host.
 *
 * The link is assembled from the host and the repository path **only**: user
 * names, passwords, tokens, ports, queries and fragments of the address never
 * reach it — a remote with credentials (masked by `remote_list` or not) yields
 * the same clean link as one without.
 *
 * No imports: `scripts/check-log-filters.mjs` transpiles this file on its own.
 */

export type Forge = "github" | "gitlab" | "bitbucket";

export interface ForgeRepo {
  forge: Forge;
  /** The web host, `github.com` and the like. */
  host: string;
  /** Path segments of the repository, `.git` removed: `["owner", "repo"]`. */
  path: string[];
}

export interface RemoteLike {
  name: string;
  fetchUrls: string[];
}

const HOSTS: Record<string, Forge> = {
  "github.com": "github",
  "www.github.com": "github",
  "ssh.github.com": "github",
  "gitlab.com": "gitlab",
  "www.gitlab.com": "gitlab",
  "altssh.gitlab.com": "gitlab",
  "bitbucket.org": "bitbucket",
  "www.bitbucket.org": "bitbucket",
  "altssh.bitbucket.org": "bitbucket",
};
const WEB_HOST: Record<Forge, string> = {
  github: "github.com",
  gitlab: "gitlab.com",
  bitbucket: "bitbucket.org",
};
export const FORGE_NAME: Record<Forge, string> = {
  github: "GitHub",
  gitlab: "GitLab",
  bitbucket: "Bitbucket",
};

const SCHEMES = new Set(["https:", "http:", "ssh:", "git+ssh:", "ssh+git:", "git:"]);
/** `[user@]host:path` — git's scp-like syntax (no scheme, no slash before the colon). */
const SCP = /^(?:[^@/]*@)?([^@/:]+):(.+)$/;
const HASH = /^[0-9a-f]{7,64}$/i;
const EMAIL = /^[^\s@<>]+@[^\s@<>]+$/;

/** The forge repository a remote address points at, or null. */
export function parseRemote(url: string): ForgeRepo | null {
  const value = url.trim();
  if (!value) return null;
  let host: string;
  let rawPath: string;
  if (value.includes("://")) {
    let u: URL;
    try {
      u = new URL(value);
    } catch {
      return null;
    }
    if (!SCHEMES.has(u.protocol)) return null;
    host = u.hostname;
    const decoded = decodeSafe(u.pathname);
    if (decoded === null) return null;
    rawPath = decoded;
  } else {
    const m = SCP.exec(value);
    if (!m) return null;
    host = m[1];
    rawPath = m[2];
  }
  const forge = HOSTS[host.toLowerCase()];
  if (!forge) return null;

  const trimmed = rawPath.replace(/^\/+/, "").replace(/\/+$/, "").replace(/\.git$/i, "");
  const path = trimmed.split("/");
  if (path.some((s) => s === "" || s === "." || s === ".." || /[\s?#\\]/.test(s))) return null;
  // GitHub and Bitbucket: owner/repo exactly; GitLab: groups may nest.
  if (forge === "gitlab" ? path.length < 2 : path.length !== 2) return null;
  return { forge, host: WEB_HOST[forge], path };
}

function decodeSafe(s: string): string | null {
  try {
    return decodeURIComponent(s);
  } catch {
    return null;
  }
}

/**
 * The remote the links are about: the one the current branch's upstream is on
 * (`origin/main` → `origin`; the longest remote name that prefixes it, since a
 * remote name may hold a slash), else `origin`, else the first. Its first fetch
 * address decides — a less preferred remote on a known forge is not used
 * instead: it may be another repository (a fork) that lacks the commit.
 */
export function pickRemote(
  remotes: readonly RemoteLike[],
  upstream: string | null | undefined,
): { name: string; url: string } | null {
  const byUpstream = upstream
    ? remotes
        .filter((r) => upstream.startsWith(`${r.name}/`))
        .sort((a, b) => b.name.length - a.name.length)[0]
    : undefined;
  const chosen = byUpstream ?? remotes.find((r) => r.name === "origin") ?? remotes[0];
  const url = chosen?.fetchUrls[0];
  return chosen && url ? { name: chosen.name, url } : null;
}

const base = (r: ForgeRepo) => `https://${r.host}/${r.path.map(encodeURIComponent).join("/")}`;

export interface ForgeLinks {
  forge: Forge;
  /** The commit's page, or null for a hash that is not one. */
  commit: string | null;
  /** The author's commits, or null — Bitbucket has no such filter. */
  authorCommits: string | null;
}

/**
 * Links for commit `hash` by `author`, from the remote address `url`; null when
 * the address is not on a known forge.
 *
 * The author filter differs per forge. GitHub's `commits?author=` takes a login
 * or an e-mail — the address is the one the commit carries. GitLab documents
 * `?author=` with the author's **name** (`?author=Elliot%20Stevens`, "Commits"
 * help page), and it needs a ref in the path: the branchless `/-/commits`
 * redirects to the default branch and drops the query (`commits_root`). `HEAD`
 * is that ref — GitLab reads an unknown first segment as the ref itself, and a
 * repository's `HEAD` is its default branch.
 */
export function forgeLinks(
  url: string,
  hash: string,
  author: { name: string; email: string },
): ForgeLinks | null {
  const r = parseRemote(url);
  if (!r) return null;
  const okHash = HASH.test(hash);
  const email = author.email.trim();
  const name = author.name.trim();
  const b = base(r);
  switch (r.forge) {
    case "github":
      return {
        forge: r.forge,
        commit: okHash ? `${b}/commit/${hash}` : null,
        authorCommits: EMAIL.test(email) ? `${b}/commits?author=${encodeURIComponent(email)}` : null,
      };
    case "gitlab":
      return {
        forge: r.forge,
        commit: okHash ? `${b}/-/commit/${hash}` : null,
        authorCommits: name ? `${b}/-/commits/HEAD?author=${encodeURIComponent(name)}` : null,
      };
    case "bitbucket":
      return { forge: r.forge, commit: okHash ? `${b}/commits/${hash}` : null, authorCommits: null };
  }
}
