import { createSignal } from "solid-js";
import {
  errText,
  openRepo as apiOpenRepo,
  repoState as apiRepoState,
  setShowIgnored as apiSetShowIgnored,
  type FileState,
  type RepoState,
} from "./api";

// Single-window app → module-level signals are the store. Every command that
// mutates the repo returns a fresh RepoState; run() funnels them through one place
// so busy/error handling is uniform and the UI never blocks.

export const [state, setState] = createSignal<RepoState | null>(null);
export const [error, setError] = createSignal("");
export const [busy, setBusy] = createSignal(false);
/** Name of the operation currently running, shown next to the busy bar. */
export const [busyLabel, setBusyLabel] = createSignal("");

// ── Window mode (Changes | Log) ──────────────────────────────────────────────
// The Log mode is a second main area, not a second window. Switching does not
// touch the Changes state: checked files and the selected changelist live in
// module signals here, so they survive the panels being unmounted. History is
// not read while the mode is "changes" — nothing in this store fetches it.

export type ViewMode = "changes" | "log";
const VIEW_MODE_KEY = "viewMode";
const [viewMode, setViewModeSignal] = createSignal<ViewMode>(
  localStorage.getItem(VIEW_MODE_KEY) === "log" ? "log" : "changes",
);
export { viewMode };
export function setViewMode(m: ViewMode) {
  setViewModeSignal(m);
  localStorage.setItem(VIEW_MODE_KEY, m);
}
export const toggleViewMode = () => setViewMode(viewMode() === "log" ? "changes" : "log");

/** path of the file whose diff is shown on the right */
export const [selectedPath, setSelectedPath] = createSignal<string | null>(null);
/** list highlighted for list-level actions / commit (defaults to the active list) */
export const [selectedListId, setSelectedListId] = createSignal<string>("default");
/** multi-select for move/commit-subset */
export const [checked, setChecked] = createSignal<Set<string>>(new Set());

// ── Theme (auto / light / dark) ──────────────────────────────────────────────
// Lifted out of Toolbar so the Settings dialog and the toolbar toggle share it.

export type Theme = "auto" | "light" | "dark";
const [theme, setThemeSignal] = createSignal<Theme>(
  (localStorage.getItem("theme") as Theme) || "auto",
);
export { theme };

function applyTheme() {
  const t = theme();
  const dark =
    t === "dark" ||
    (t === "auto" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", dark);
}
export function setTheme(t: Theme) {
  setThemeSignal(t);
  localStorage.setItem("theme", t);
  applyTheme();
}
export function cycleTheme() {
  setTheme(theme() === "auto" ? "light" : theme() === "light" ? "dark" : "auto");
}
applyTheme();

/**
 * Install a fresh RepoState and re-validate everything that pointed into the old one.
 *
 * **`setState` here must stay a bare call — no `batch`, no transition.** It is
 * not only a store update: a new state object is how the window says "the world
 * may have moved", and `DiffView` hangs its whole invalidation on it. Its effect
 * on `state` drops the diff it has drawn and re-reads the file, which is what
 * makes a `git reset` run in a terminal show up when the panel is refreshed —
 * and what every staged hunk now relies on, since the diff panel no longer
 * re-reads on its own. Deferred or coalesced away, that failure is silent: the
 * build stays green and the panel keeps drawing the previous patch.
 *
 * The revision below runs *after* the effect has already fired, so nothing may
 * assume the effect sees a re-validated selection — `DiffView` defers its own
 * read by a microtask for exactly that reason.
 */
function applyState(s: RepoState): void {
  setState(s);
  // keep selection valid after the tree changes
  const paths = new Set(s.changelists.flatMap((c) => c.files.map((f) => f.path)));
  if (selectedPath() && !paths.has(selectedPath()!)) setSelectedPath(null);
  setChecked((prev) => new Set([...prev].filter((x) => paths.has(x))));
  if (!s.changelists.some((c) => c.id === selectedListId())) {
    setSelectedListId(s.activeChangelistId);
  }
}

/**
 * `label` names the operation for the busy indicator: an unlabelled bar during
 * a long fetch or rebase says only "something is happening".
 */
export async function run(p: Promise<RepoState>, label = ""): Promise<void> {
  setBusy(true);
  setBusyLabel(label);
  try {
    applyState(await p);
    setError("");
  } catch (e) {
    setError(errText(e));
    // A refused command is not the same as an unchanged repository. A revert,
    // merge, rebase or cherry-pick that ends in a conflict *fails* — git returns
    // non-zero and prints CONFLICT — and yet leaves the repository mid-operation,
    // with conflicted files in the tree. Keeping the pre-command state here would
    // leave the red banner as the only sign of it: the Continue / Skip / Abort
    // strip reads `RepoState.operation`, and the changes panel reads the same
    // state, so both would keep showing the world as it was before the failure
    // until some later command happened to refresh it. Re-read instead, so the
    // state is announced on entry, in whichever mode the window is in.
    // The error stays: this only replaces the state, never the message.
    try {
      applyState(await apiRepoState());
    } catch {
      // The repository itself is unreadable (none open, deleted). The original
      // error is the one worth showing — this second failure adds nothing.
    }
  } finally {
    setBusy(false);
    setBusyLabel("");
  }
}

export const refresh = () => run(apiRepoState());

const LAST_REPO_KEY = "lastRepo";
const RECENT_REPOS_KEY = "recentRepos";
const RECENT_MAX = 10;

const loadRecent = (): string[] => {
  try {
    const v = JSON.parse(localStorage.getItem(RECENT_REPOS_KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
};

/** Recently opened repos (resolved roots), most-recent first. Powers the repo menu. */
export const [recentRepos, setRecentRepos] = createSignal<string[]>(loadRecent());

/** Record a successfully-opened repo as last-used and at the front of recents. */
function rememberOpened(repoPath: string) {
  localStorage.setItem(LAST_REPO_KEY, repoPath);
  const next = [repoPath, ...recentRepos().filter((p) => p !== repoPath)].slice(0, RECENT_MAX);
  setRecentRepos(next);
  localStorage.setItem(RECENT_REPOS_KEY, JSON.stringify(next));
}

/** Does this failure mean the folder itself is gone? Only then is a remembered
 *  path worth forgetting — a repository the system merely refused to let us read
 *  (macOS access prompt declined) is still there, and dropping it would punish the
 *  user for one "Don't Allow". The literal comes from `CliEngine::resolve_root`. */
const isMissingFolder = (msg: string) => msg.includes("no such folder");

/** Open a repo by path (absolute or relative) and remember it for next launch. */
export async function openRepoAt(path: string): Promise<void> {
  await run(apiOpenRepo(path));
  const s = state();
  if (s && !error()) {
    rememberOpened(s.repoPath);
  } else if (error() && isMissingFolder(error())) {
    // The folder is gone (a recent that was deleted): drop that path so it doesn't
    // linger in the menu and re-error on every click. `path` is the stored resolved
    // root for a recent-click; a dialog pick won't match (no-op).
    const pruned = recentRepos().filter((p) => p !== path);
    setRecentRepos(pruned);
    localStorage.setItem(RECENT_REPOS_KEY, JSON.stringify(pruned));
  }
}

/**
 * Startup open. Prefer the launch directory (terminal `cd repo && graft`);
 * if that isn't a git repo (e.g. launched from Finder with cwd "/"), fall back
 * to the last-used repo so double-click still lands somewhere useful.
 *
 * The launch-directory probe deliberately bypasses `run()`: a bundle started from
 * Finder has cwd `/`, so the probe fails on **every** double-click, and its failure
 * says nothing about anything the user did. Through `run()` it painted the red
 * banner with `git rev-parse --show-toplevel failed` over a window that had not yet
 * opened a repository. A failed probe now simply means "not launched from inside a
 * repository", and the window shows its empty state.
 */
export async function openInitial(): Promise<void> {
  let opened = false;
  try {
    applyState(await apiOpenRepo("."));
    setError("");
    opened = true;
  } catch {
    // not started from inside a working tree — silence is the correct report
  }
  if (opened) {
    const s = state();
    if (s) rememberOpened(s.repoPath);
  } else {
    const last = localStorage.getItem(LAST_REPO_KEY);
    if (last) {
      await openRepoAt(last);
      if (error() && isMissingFolder(error())) localStorage.removeItem(LAST_REPO_KEY);
    }
  }
  // The backend's show-ignored flag is per app-session; re-apply the saved choice.
  if (showIgnored() && !error()) await run(apiSetShowIgnored(true));
}

// ── View toggles (persisted) ─────────────────────────────────────────────────

const SHOW_IGNORED_KEY = "showIgnored";
export const [showIgnored, setShowIgnoredSig] = createSignal(
  localStorage.getItem(SHOW_IGNORED_KEY) === "1",
);
export async function toggleShowIgnored(): Promise<void> {
  const next = !showIgnored();
  setShowIgnoredSig(next);
  localStorage.setItem(SHOW_IGNORED_KEY, next ? "1" : "0");
  await run(apiSetShowIgnored(next));
}

const GROUP_BY_DIR_KEY = "groupByDir";
export const [groupByDir, setGroupByDirSig] = createSignal(
  localStorage.getItem(GROUP_BY_DIR_KEY) === "1",
);
export function toggleGroupByDir(): void {
  const next = !groupByDir();
  setGroupByDirSig(next);
  localStorage.setItem(GROUP_BY_DIR_KEY, next ? "1" : "0");
}

export function toggleChecked(path: string) {
  setChecked((prev) => {
    const n = new Set(prev);
    n.has(path) ? n.delete(path) : n.add(path);
    return n;
  });
}
export const isChecked = (path: string) => checked().has(path);

// ── In-app modals (styled, not native alert/confirm) ─────────────────────────

export const [confirmState, setConfirmState] = createSignal<{
  message: string;
  danger: boolean;
  resolve: (ok: boolean) => void;
} | null>(null);

export function confirmAction(message: string, danger = true): Promise<boolean> {
  return new Promise((resolve) => setConfirmState({ message, danger, resolve }));
}

export const [promptState, setPromptState] = createSignal<{
  title: string;
  value: string;
  resolve: (v: string | null) => void;
} | null>(null);

export function promptText(title: string, value = ""): Promise<string | null> {
  return new Promise((resolve) => setPromptState({ title, value, resolve }));
}

export type ChoiceOption = { key: string; label: string; danger?: boolean };
export const [chooseState, setChooseState] = createSignal<{
  message: string;
  options: ChoiceOption[];
  resolve: (key: string | null) => void;
} | null>(null);

export function chooseOption(
  message: string,
  options: ChoiceOption[],
): Promise<string | null> {
  return new Promise((resolve) => setChooseState({ message, options, resolve }));
}

/**
 * Modals that live outside this store — the Log panel's own form dialogs.
 *
 * "Is a modal up" has to be one question with one answer: the keyboard layer
 * stands down on it, and a second, private flag somewhere else means arrows keep
 * moving a list behind a dialog nobody can see them move. Sources register
 * themselves here; the list is a signal so `modalOpen()` stays reactive.
 */
const [modalSources, setModalSources] = createSignal<Array<() => boolean>>([]);

export function registerModalSource(isOpen: () => boolean): () => void {
  setModalSources((l) => [...l, isOpen]);
  return () => setModalSources((l) => l.filter((f) => f !== isOpen));
}

/** True while any modal is up — the keyboard layer stands down meanwhile. */
export const modalOpen = () =>
  confirmState() !== null ||
  promptState() !== null ||
  chooseState() !== null ||
  modalSources().some((f) => f());

/** Status → { letter, colour class }. Mirrors the TUI palette mapping. */
export function statusMeta(s: FileState): { letter: string; cls: string } {
  switch (s) {
    case "modified":
      return { letter: "M", cls: "text-warn" };
    case "added":
      return { letter: "A", cls: "text-success" };
    case "deleted":
      return { letter: "D", cls: "text-danger" };
    case "renamed":
      return { letter: "R", cls: "text-accent" };
    case "conflicted":
      return { letter: "C", cls: "text-danger" };
    case "untracked":
      return { letter: "?", cls: "text-fg-muted" };
    case "ignored":
      return { letter: "!", cls: "text-fg-muted" };
  }
}
