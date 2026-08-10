use serde::{Deserialize, Serialize};

/// git status of a changed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileState {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

/// A changed file with its status and index/worktree staging flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileStatus {
    pub path: String,
    pub status: FileState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    /// has staged (index) changes relative to HEAD
    pub staged: bool,
    /// has unstaged (worktree) changes relative to index
    pub unstaged: bool,
}

/// Result of a `git status` snapshot (internal; not sent to the frontend as-is).
#[derive(Debug, Clone)]
pub struct RepoSnapshot {
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub detached: bool,
    pub files: Vec<FileStatus>,
}

/// A changelist as presented to the UI: the list plus its resolved file statuses.
/// `is_unversioned` marks the synthetic "Unversioned Files" list (untracked, never
/// persisted).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangelistView {
    pub id: String,
    pub name: String,
    pub comment: String,
    pub is_default: bool,
    pub is_unversioned: bool,
    pub files: Vec<FileStatus>,
}

/// One line of a diff hunk. `origin` is " ", "+" or "-".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub origin: String,
    pub content: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

/// A diff hunk. `patch` is the **exact, self-contained** patch text (file header +
/// this hunk) so the frontend can hand it straight back to `git apply` for
/// hunk-level stage/revert without any lossy reconstruction.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
    pub patch: String,
}

/// A file's diff against a chosen base.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

/// Full repository state pushed to the UI on every mutation / refresh.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoState {
    pub repo_path: String,
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub detached: bool,
    pub active_changelist_id: String,
    pub changelists: Vec<ChangelistView>,
}
