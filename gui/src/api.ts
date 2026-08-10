import { invoke } from "@tauri-apps/api/core";

// ── Types (mirror src-tauri/src/model.rs) ────────────────────────────────────

export type FileState =
  | "modified"
  | "added"
  | "deleted"
  | "renamed"
  | "untracked"
  | "conflicted";

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
