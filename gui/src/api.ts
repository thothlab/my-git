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
  hunks: Hunk[];
}

/// `whitespace` defaults to "none" — showing every difference is the historical
/// behaviour and the safe one. An unknown mode is rejected by the backend.
export const diffFile = (
  path: string,
  against: DiffBase,
  whitespace: WhitespaceMode = "none",
) => invoke<FileDiff>("diff_file", { path, against, whitespace });

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
) => invoke<FileDiff>("commit_file_diff", { hash, path, whitespace });
export const commitsCompare = (from: string, to: string) =>
  invoke<CommitFileEntry[]>("commits_compare", { from, to });
export const commitsCompareDiff = (
  from: string,
  to: string,
  path: string,
  whitespace: WhitespaceMode = "none",
) => invoke<FileDiff>("commits_compare_diff", { from, to, path, whitespace });

// branch tree (task 05)
export const branchTree = () => invoke<BranchNode[]>("branch_tree");
export const branchRename = (from: string, to: string) =>
  invoke<RepoState>("branch_rename", { from, to });
export const branchDelete = (name: string, remote: boolean, force: boolean) =>
  invoke<RepoState>("branch_delete", { name, remote, force });
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
export const tagCreate = (hash: string, name: string, message?: string) =>
  invoke<RepoState>("tag_create", { hash, name, message: message ?? null });

export const opContinue = () => invoke<RepoState>("op_continue");
export const opAbort = () => invoke<RepoState>("op_abort");
export const opSkip = () => invoke<RepoState>("op_skip");

export const stashListApp = () => invoke<string[]>("stash_list_app");
export const stashRestore = (name: string) =>
  invoke<RepoState>("stash_restore", { name });

// panel UI state (task 01)
export const uiStateGet = () => invoke<UiState>("ui_state_get");
/** Rust parameter is named `ui`: `state` is taken by Tauri's managed state. */
export const uiStateSet = (ui: UiState) => invoke<UiState>("ui_state_set", { ui });
