import { invoke } from "@tauri-apps/api/core";

// ── Types (mirror src-tauri/src/model.rs) ────────────────────────────────────

export type FileState =
  | "modified"
  | "added"
  | "deleted"
  | "renamed"
  | "untracked"
  | "conflicted"
  | "ignored";

export interface FileStatus {
  path: string;
  status: FileState;
  oldPath?: string | null;
  staged: boolean;
  unstaged: boolean;
}

export interface ChangelistView {
  id: string;
  name: string;
  comment: string;
  isDefault: boolean;
  isUnversioned: boolean;
  isIgnored?: boolean;
  files: FileStatus[];
}

export interface RepoState {
  repoPath: string;
  branch: string;
  upstream?: string | null;
  ahead: number;
  behind: number;
  detached: boolean;
  activeChangelistId: string;
  changelists: ChangelistView[];
  operation: OperationState;
  /** `user.email` of this repository, or null when git has none configured.
   * The log tells the reader's own commits apart by it (R45i). */
  userEmail?: string | null;
}

// Error shape returned by Rust commands (see error.rs). Always carries a message;
// git failures also carry the underlying stderr.
export interface BackendError {
  kind: "git" | "io" | "parse" | "rule";
  message: string;
  stderr?: string | null;
}

export function errText(e: unknown): string {
  const be = e as Partial<BackendError> | undefined;
  if (be && typeof be === "object" && "message" in be) {
    return be.stderr ? `${be.message}\n${be.stderr}` : (be.message ?? String(e));
  }
  return String(e);
}

// ── Commands ─────────────────────────────────────────────────────────────────

export const openRepo = (path?: string) =>
  invoke<RepoState>("repo_open", { path: path ?? null });

export const repoState = () => invoke<RepoState>("repo_state");

export const setShowIgnored = (value: boolean) =>
  invoke<RepoState>("set_show_ignored", { value });

// changelists (task_02)
export const changelistCreate = (name: string) =>
  invoke<RepoState>("changelist_create", { name });

export const changelistRename = (id: string, name: string) =>
  invoke<RepoState>("changelist_rename", { id, name });

export const changelistSetComment = (id: string, comment: string) =>
  invoke<RepoState>("changelist_set_comment", { id, comment });

export const changelistDelete = (id: string) =>
  invoke<RepoState>("changelist_delete", { id });

export const changelistSetActive = (id: string) =>
  invoke<RepoState>("changelist_set_active", { id });

export const filesMove = (paths: string[], toListId: string) =>
  invoke<RepoState>("files_move", { paths, toListId });

// rollback (task_03)
export const fileRollback = (paths: string[]) =>
  invoke<RepoState>("file_rollback", { paths });

export const listRollback = (id: string) =>
  invoke<RepoState>("list_rollback", { id });

// diff & hunk staging (task_04)
export type DiffBase = "worktree" | "index" | "head";

export interface DiffLine {
  origin: " " | "+" | "-";
  content: string;
  oldNo: number | null;
  newNo: number | null;
}
export interface Hunk {
  header: string;
  lines: DiffLine[];
  patch: string;
}
export interface FileDiff {
  path: string;
  binary: boolean;
  /** Bytes on each side of a **binary** file — the only honest thing to show when
   * there is no text (prd_02 История 68). Absent on the side where the file does
   * not exist (added / deleted) and absent entirely for a text diff. */
  oldSize?: number;
  newSize?: number;
  /** The diff is a merge commit's comparison against its *first* parent — the panel
   * has to say so, and the fact travels with the diff itself. */
  mergeFirstParent: boolean;
  hunks: Hunk[];
}

/// `whitespace` defaults to "none" — showing every difference is the historical
/// behaviour and the safe one. An unknown mode is rejected by the backend.
/**
 * `context` is how many unchanged lines to keep around each change. Omitted
 * means "as before" — the backend then passes no `-U` at all and the patch is
 * byte-for-byte the historical one. Raising it is how the panel reveals the
 * lines git left out *between* hunks (R46i, D04).
 */
export const diffFile = (
  path: string,
  against: DiffBase,
  whitespace: WhitespaceMode = "none",
  context?: number,
) => invoke<FileDiff>("diff_file", { path, against, whitespace, context });

export const hunkStage = (patch: string) =>
  invoke<RepoState>("hunk_stage", { patch });
export const hunkUnstage = (patch: string) =>
  invoke<RepoState>("hunk_unstage", { patch });
export const hunkRevert = (patch: string) =>
  invoke<RepoState>("hunk_revert", { patch });

// commit (task_05)
export const commitList = (a: {
  id?: string;
  paths?: string[];
  message: string;
  amend: boolean;
}) =>
  invoke<RepoState>("commit_list", {
    id: a.id ?? null,
    paths: a.paths ?? null,
    message: a.message,
    amend: a.amend,
  });

// branches & remotes (task_06)
export interface BranchInfo {
  name: string;
  isRemote: boolean;
  isCurrent: boolean;
  upstream?: string | null;
}

export const branchList = () => invoke<BranchInfo[]>("branch_list");
export const branchCreate = (name: string, from?: string) =>
  invoke<RepoState>("branch_create", { name, from: from ?? null });
export const branchCheckout = (name: string, stash: boolean) =>
  invoke<RepoState>("branch_checkout", { name, stash });

export type PushMode = "normal" | "upstream" | "force";
export const push = (mode: PushMode) => invoke<RepoState>("push", { mode });
export const fetchRemote = () => invoke<RepoState>("fetch");
export const pull = () => invoke<RepoState>("pull");

// ── Git panel: history (prd_02, task_01) ─────────────────────────────────────

export type WhitespaceMode = "none" | "trailing" | "all";

export type RefKind = "head" | "local" | "remote" | "tag";
export interface RefLabel {
  name: string;
  kind: RefKind;
}

export type LaneEdgeKind = "straight" | "branch" | "merge";
export interface LaneEdge {
  fromLane: number;
  toLane: number;
  kind: LaneEdgeKind;
  color: number;
}

export interface LogCommit {
  hash: string;
  shortHash: string;
  parents: string[];
  author: string;
  authorEmail: string;
  authorAt: number;
  subject: string;
  refs: RefLabel[];
  lane: number;
  edges: LaneEdge[];
}

export interface LogCursor {
  skip: number;
  /** hashes of parents whose graph lines cross the page boundary */
  openLanes: string[];
}

export interface LogPage {
  commits: LogCommit[];
  nextCursor: LogCursor | null;
  laneOverflow: boolean;
}

export type LogOrder = "date" | "topo";

export interface LogFilter {
  branch?: string | null;
  text?: string | null;
  regex: boolean;
  matchCase: boolean;
  author?: string | null;
  since?: number | null;
  until?: number | null;
  paths: string[];
  order: LogOrder;
}

export const emptyLogFilter = (): LogFilter => ({
  branch: null,
  text: null,
  regex: false,
  matchCase: false,
  author: null,
  since: null,
  until: null,
  paths: [],
  order: "date",
});

export interface CommitDetails {
  hash: string;
  parents: string[];
  author: string;
  authorEmail: string;
  authorAt: number;
  committer: string;
  committerEmail: string;
  committerAt: number;
  subject: string;
  body: string;
  refs: RefLabel[];
  branches: string[];
  branchesTruncated: boolean;
}

export interface CommitFileEntry {
  status: FileState;
  path: string;
  oldPath: string | null;
}

export interface BranchNode {
  name: string;
  fullRef: string;
  isRemote: boolean;
  isCurrent: boolean;
  upstream: string | null;
  ahead: number | null;
  behind: number | null;
  isFavorite: boolean;
  lastCommitAt: number;
}

export type OperationKind = "none" | "merge" | "rebase" | "cherryPick" | "revert";

export interface OperationState {
  kind: OperationKind;
  current: number | null;
  total: number | null;
  conflicted: string[];
}

/** Panel UI state, persisted in `.git/graft-ui.json` (never in changelists.json). */
export interface UiState {
  favorites: string[];
  collapsedFolders: string[];
  columnWidths: Record<string, number>;
  logHighlight: boolean;
  version: number;
}

export const emptyUiState = (): UiState => ({
  favorites: [],
  collapsedFolders: [],
  columnWidths: {},
  logHighlight: false,
  version: 1,
});

// log (task 03)
export const logPage = (filter: LogFilter, cursor: LogCursor | null, limit: number) =>
  invoke<LogPage>("log_page", { filter, cursor, limit });
export const logAuthors = () => invoke<string[]>("log_authors");

// one commit (task 04)
export const commitDetails = (hash: string) =>
  invoke<CommitDetails>("commit_details", { hash });
export const commitFiles = (hash: string) =>
  invoke<CommitFileEntry[]>("commit_files", { hash });
export const commitFileDiff = (
  hash: string,
  path: string,
  whitespace: WhitespaceMode = "none",
  context?: number,
) => invoke<FileDiff>("commit_file_diff", { hash, path, whitespace, context });
/** The revision that means "the working tree" in `commitsCompare` /
 * `commitsCompareDiff` (prd_02 История 77). A comparison against a real revision
 * always names it, so passing this constant is a deliberate choice rather than an
 * empty string that slipped through. */
export const WORKING_TREE = "";

/** Of the given commits, the ones the current revision cannot reach — the input
 * behind the log's row emphasis (R45i, D05). */
export const commitsUnreachable = (hashes: string[]) =>
  invoke<string[]>("commits_unreachable", { hashes });

export const commitsCompare = (from: string, to: string) =>
  invoke<CommitFileEntry[]>("commits_compare", { from, to });
export const commitsCompareDiff = (
  from: string,
  to: string,
  path: string,
  whitespace: WhitespaceMode = "none",
  context?: number,
) => invoke<FileDiff>("commits_compare_diff", { from, to, path, whitespace, context });

// branch tree (task 05)
export const branchTree = () => invoke<BranchNode[]>("branch_tree");
export const branchRename = (from: string, to: string) =>
  invoke<RepoState>("branch_rename", { from, to });
export const branchDelete = (name: string, remote: boolean, force: boolean) =>
  invoke<RepoState>("branch_delete", { name, remote, force });
/// Commits that deleting the branch would lose — asked before the delete, so the
/// confirmation can name the number instead of parsing a failed attempt.
export const branchUnmergedCount = (name: string) =>
  invoke<number>("branch_unmerged_count", { name });
export const branchMerge = (name: string) =>
  invoke<RepoState>("branch_merge", { name });
export const branchRebaseOnto = (name: string) =>
  invoke<RepoState>("branch_rebase_onto", { name });

// operations on commits (task 06)
/// Manifest G02 / История 57: four reset modes. An unknown value is rejected by
/// the backend, not folded into a default.
export type ResetMode = "soft" | "mixed" | "hard" | "keep";
export const commitRevert = (hash: string) =>
  invoke<RepoState>("commit_revert", { hash });
export const commitReset = (hash: string, mode: ResetMode) =>
  invoke<RepoState>("commit_reset", { hash, mode });
export const commitCherryPick = (hash: string) =>
  invoke<RepoState>("commit_cherry_pick", { hash });
export const commitCheckout = (hash: string) =>
  invoke<RepoState>("commit_checkout", { hash });
/// Is the commit already on the current branch? Asked before the menu is drawn, so
/// cherry-pick can be disabled with a reason instead of failing when clicked.
export const commitContains = (hash: string) =>
  invoke<boolean>("commit_contains", { hash });
/// Commits a reset to this hash would discard — asked before the operation, so the
/// hard-reset confirmation can name the number.
export const commitResetLostCount = (hash: string) =>
  invoke<number>("commit_reset_lost_count", { hash });
/// Anything uncommitted in tree or index — the other half of the hard-reset warning.
export const repoLocalChanges = () => invoke<boolean>("repo_local_changes");
export const tagCreate = (hash: string, name: string, message?: string) =>
  invoke<RepoState>("tag_create", { hash, name, message: message ?? null });

export const opContinue = () => invoke<RepoState>("op_continue");
export const opAbort = () => invoke<RepoState>("op_abort");
export const opSkip = () => invoke<RepoState>("op_skip");

export const stashListApp = () => invoke<string[]>("stash_list_app");
/** One entry of {@link stashListApp}: NUL-separated ref, unix time, git's text. */
export type AppStash = { ref: string; at: number; label: string };
/**
 * Split a `stash_list_app` entry. The engine packs three fields into the string
 * because the command's contract is `string[]`; `at` is unix seconds and the
 * formatting stays on this side, in the panel's locale.
 */
export function parseAppStash(entry: string): AppStash {
  const [ref = entry, at = "0", label = ""] = entry.split("\u0000");
  return { ref, at: Number(at) || 0, label };
}
export const stashRestore = (name: string) =>
  invoke<RepoState>("stash_restore", { name });

// stash manager (История 21b)

/** One stash of the repository — every stash, not only the application's own. */
export interface StashEntry {
  /** git's own `stash@{N}`. It **renumbers** after any pop or drop. */
  ref: string;
  /** The stash commit, which does not move — pass it back so a destructive
   *  operation on a stale list is refused instead of hitting the neighbour. */
  hash: string;
  /** Unix seconds; formatting stays on this side, in the panel's locale. */
  at: number;
  /** `null` when the stash carries no recognisable branch (detached HEAD). */
  branch: string | null;
  message: string;
  /** Made by the application while switching branches — a mark, not a filter. */
  fromApp: boolean;
}

export const stashList = () => invoke<StashEntry[]>("stash_list");
/** Restore and keep the entry. */
export const stashApply = (name: string, hash?: string) =>
  invoke<RepoState>("stash_apply", { name, hash: hash ?? null });
/** Restore and drop the entry. */
export const stashPop = (name: string, hash?: string) =>
  invoke<RepoState>("stash_pop", { name, hash: hash ?? null });
/** Discard without applying. */
export const stashDrop = (name: string, hash?: string) =>
  invoke<RepoState>("stash_drop", { name, hash: hash ?? null });
/** What the stash changes — the file list of a commit, tracked files only. */
export const stashFiles = (name: string) =>
  invoke<CommitFileEntry[]>("stash_files", { name });
/** Stash the current changes, untracked included. A clean tree is refused. */
export const stashPush = (message?: string) =>
  invoke<RepoState>("stash_push", { message: message ?? null });

/** Bring a branch up to date with its upstream: `pull` for the current branch, a
 *  fast-forward in place for any other. A diverged branch and one with no upstream
 *  are refused with a reason instead of being touched. */
export const branchUpdate = (name: string) =>
  invoke<RepoState>("branch_update", { name });

// panel UI state (task 01)
export const uiStateGet = () => invoke<UiState>("ui_state_get");
/** Rust parameter is named `ui`: `state` is taken by Tauri's managed state. */
export const uiStateSet = (ui: UiState) => invoke<UiState>("ui_state_set", { ui });
