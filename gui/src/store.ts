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

export async function run(p: Promise<RepoState>): Promise<void> {
  setBusy(true);
  try {
    const s = await p;
    setState(s);
    setError("");
    // keep selection valid after the tree changes
    const paths = new Set(s.changelists.flatMap((c) => c.files.map((f) => f.path)));
    if (selectedPath() && !paths.has(selectedPath()!)) setSelectedPath(null);
    setChecked((prev) => new Set([...prev].filter((x) => paths.has(x))));
    if (!s.changelists.some((c) => c.id === selectedListId())) {
      setSelectedListId(s.activeChangelistId);
    }
  } catch (e) {
    setError(errText(e));
  } finally {
    setBusy(false);
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

/** Open a repo by path (absolute or relative) and remember it for next launch. */
export async function openRepoAt(path: string): Promise<void> {
  await run(apiOpenRepo(path));
  const s = state();
  if (s && !error()) {
    rememberOpened(s.repoPath);
  } else if (error()) {
    // Opening failed (e.g. a recent whose folder was deleted): drop that path so
    // it doesn't linger in the menu and re-error on every click. `path` is the
    // stored resolved root for a recent-click; a dialog pick won't match (no-op).
    const pruned = recentRepos().filter((p) => p !== path);
    setRecentRepos(pruned);
    localStorage.setItem(RECENT_REPOS_KEY, JSON.stringify(pruned));
  }
}

/**
 * Startup open. Prefer the launch directory (terminal `cd repo && mygit-gui`);
 * if that isn't a git repo (e.g. launched from Finder with cwd "/"), fall back
 * to the last-used repo so double-click still lands somewhere useful.
 */
export async function openInitial(): Promise<void> {
  await run(apiOpenRepo("."));
  if (!error()) {
    const s = state();
    if (s) rememberOpened(s.repoPath);
  } else {
    const last = localStorage.getItem(LAST_REPO_KEY);
    if (last) {
      await openRepoAt(last);
      if (error()) localStorage.removeItem(LAST_REPO_KEY); // stale — forget it
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
