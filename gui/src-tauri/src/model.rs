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
    Ignored,
}

/// A changed file with its status and index/worktree staging flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    #[serde(default)]
    pub is_ignored: bool,
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
///
/// `old_size` / `new_size` exist for a **binary** file, where there is no text to
/// show and the only honest statement is how many bytes the file had on each side
/// (prd_02 История 68). A side where the file does not exist — an addition or a
/// deletion — has `None`, and a text diff carries neither.
///
/// `merge_first_parent` marks a merge commit's diff: it is the comparison against
/// the commit's *first* parent, one of several possible readings, so the panel must
/// say so. The fact travels with the diff rather than being reassembled by the UI
/// from a second call.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub binary: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_size: Option<u64>,
    pub merge_first_parent: bool,
    pub hunks: Vec<Hunk>,
}

/// A branch (local or remote-tracking) for the branch picker.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub name: String,
    pub is_remote: bool,
    pub is_current: bool,
    pub upstream: Option<String>,
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
    /// `user.email` as this repository resolves it, or `None` when git has none
    /// configured. The log needs it to tell the reader's own commits apart
    /// (R45i, D05); it is not derived from the last commit, which would name
    /// whoever committed last rather than whoever is sitting here.
    pub user_email: Option<String>,
    /// Unfinished merge / rebase / cherry-pick / revert, if any. Travels with the
    /// state rather than a separate command — see prd_02 §Контракты и API.
    pub operation: OperationState,
}

// ── History panel (prd_02) ───────────────────────────────────────────────────

/// Kind of a ref label parsed out of `%D` on a log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RefKind {
    Head,
    Local,
    Remote,
    Tag,
}

/// A ref decorating a commit ("HEAD -> main", "origin/main", "tag: v1").
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
}

/// How a lane edge leaves a commit row towards the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LaneEdgeKind {
    Straight,
    Branch,
    Merge,
}

/// One segment of the commit graph between two adjacent rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneEdge {
    pub from_lane: u16,
    pub to_lane: u16,
    pub kind: LaneEdgeKind,
    pub color: u8,
}

/// A commit row of the log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogCommit {
    pub hash: String,
    pub short_hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub author_email: String,
    pub author_at: i64,
    pub subject: String,
    pub refs: Vec<RefLabel>,
    pub lane: u16,
    pub edges: Vec<LaneEdge>,
}

/// Where the next page starts. `open_lanes` are the parent hashes whose graph lines
/// cross the page boundary, so the next page can continue the same lanes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogCursor {
    pub skip: u32,
    pub open_lanes: Vec<String>,
}

/// One page of the log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    pub commits: Vec<LogCommit>,
    pub next_cursor: Option<LogCursor>,
    pub lane_overflow: bool,
}

/// Commit ordering requested by the filter bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogOrder {
    #[default]
    Date,
    Topo,
}

/// The filter bar as one value.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LogFilter {
    pub branch: Option<String>,
    pub text: Option<String>,
    pub regex: bool,
    pub match_case: bool,
    pub author: Option<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub paths: Vec<String>,
    pub order: LogOrder,
}

/// Everything shown in the commit card. `branches_truncated` marks that the
/// "contained in" list was cut for size.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetails {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub author_email: String,
    pub author_at: i64,
    pub committer: String,
    pub committer_email: String,
    pub committer_at: i64,
    pub subject: String,
    pub body: String,
    pub refs: Vec<RefLabel>,
    pub branches: Vec<String>,
    pub branches_truncated: bool,
}

/// A file touched by a commit (or by a comparison of two revisions).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFileEntry {
    pub status: FileState,
    pub path: String,
    pub old_path: Option<String>,
}

/// A node of the branch tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchNode {
    pub name: String,
    pub full_ref: String,
    pub is_remote: bool,
    pub is_current: bool,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub is_favorite: bool,
    pub last_commit_at: i64,
}

/// Which multi-step git operation is in progress, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationKind {
    #[default]
    None,
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

/// State of an unfinished operation. `kind: None` means the repository is calm.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationState {
    pub kind: OperationKind,
    pub current: Option<u32>,
    pub total: Option<u32>,
    pub conflicted: Vec<String>,
}

/// UI state of the Git panel, persisted in `.git/graft-ui.json`.
///
/// Deliberately a **separate** file from `.git/changelists.json`, which stays
/// byte-compatible with the TUI and is never touched by this panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiState {
    #[serde(default)]
    pub favorites: Vec<String>,
    #[serde(default)]
    pub collapsed_folders: Vec<String>,
    #[serde(default)]
    pub column_widths: std::collections::BTreeMap<String, u32>,
    #[serde(default)]
    pub log_highlight: bool,
    #[serde(default = "ui_state_version")]
    pub version: u32,
}

fn ui_state_version() -> u32 {
    1
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            favorites: Vec::new(),
            collapsed_folders: Vec::new(),
            column_widths: std::collections::BTreeMap::new(),
            log_highlight: false,
            version: ui_state_version(),
        }
    }
}

/// One entry of the repository's stash list — every stash, not only the ones the
/// application made for itself.
///
/// `reference` is git's own `stash@{N}`, which **renumbers** after any pop or drop;
/// `hash` is the stash commit and does not move, which is what lets a destructive
/// operation confirm it is about to touch the entry the user picked. `branch` is
/// `None` when the message carries no recognisable branch (a stash made in detached
/// HEAD reads `WIP on (no branch): …`), never a name invented from the text.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StashEntry {
    #[serde(rename = "ref")]
    pub reference: String,
    pub hash: String,
    pub at: i64,
    pub branch: Option<String>,
    pub message: String,
    /// Made by the application itself while switching branches (`APP_STASH_TAG`).
    /// The panel shows every stash; this only lets it tell them apart.
    pub from_app: bool,
}
