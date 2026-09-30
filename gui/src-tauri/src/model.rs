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

/// A diff hunk. Carries no patch text: stage / unstage / revert name hunks and lines
/// by index ([`HunkPick`]) and the backend rebuilds the patch from the diff it reads
/// again — a patch handed back by the client would be applied against whatever the
/// file has become since it was drawn.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// Lines of one hunk chosen for `lines_stage` / `lines_unstage` / `lines_revert`:
/// `hunk` and each line are indexes into `FileDiff::hunks` and `Hunk::lines` of the
/// diff the reader was shown. `"all"` is the whole hunk.
#[derive(Debug, Clone, Deserialize)]
pub struct HunkPick {
    pub hunk: usize,
    pub lines: LinePick,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum LinePick {
    All(AllLines),
    Lines(Vec<usize>),
}

/// The literal `"all"`.
#[derive(Debug, Clone, Copy, Deserialize)]
pub enum AllLines {
    #[serde(rename = "all")]
    All,
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
///
/// `digest` fingerprints the exact bytes git printed (`cli::fnv1a`) for a working-tree
/// diff; a line action sends it back and is refused as stale when the diff read again
/// is not the same. Empty for a revision diff, which nothing is applied from.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub binary: bool,
    pub digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_size: Option<u64>,
    pub merge_first_parent: bool,
    pub hunks: Vec<Hunk>,
    /// The diff changes a Git LFS pointer and nothing else (`engine::lfs`): the
    /// panel shows a card — sizes, oid, whether the content is local — instead of
    /// the pointer's three lines. The hunks are still there, for staging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lfs: Option<LfsDiff>,
}

/// One side of an LFS pointer change.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LfsSide {
    /// sha256 of the real content, 64 lowercase hex digits.
    pub oid: String,
    /// Bytes of the real content, as the pointer says.
    pub size: u64,
    /// The object is in the local LFS store, whole.
    pub downloaded: bool,
}

/// Whether the card offers "Download", and why not. `NotNeeded`: there is nothing
/// to fetch (the new side is local, or the file was deleted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LfsDownload {
    NotNeeded,
    /// `lfs_pull` would fetch exactly the new side's object.
    Available,
    /// git-lfs is not installed where git would look for it.
    NoLfs,
    /// The object is not the checked-out version of the file (a commit in
    /// history), or the path is not marked `filter=lfs` — `git lfs pull` would
    /// fetch something else or nothing.
    NotCheckedOut,
    /// The path cannot be written as a literal `--include` pattern.
    UnsafePath,
}

/// An LFS pointer change: `old` absent for an added file, `new` for a deleted one.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LfsDiff {
    pub old: Option<LfsSide>,
    pub new: Option<LfsSide>,
    pub download: LfsDownload,
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

/// Payload of the `repo-external-change` event (`crate::watch`): something outside
/// Graft moved refs, `HEAD` or an operation marker of this repository. `repo_path` is
/// spelled exactly as `RepoState.repo_path`, so the window can tell a late event of a
/// repository it has already left.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoExternalChange {
    pub repo_path: String,
}

/// What a background fetch did (`repo_fetch_background`). A failure is an `Err` as
/// usual; the window keeps it as a line in the status bar, not a banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackgroundFetch {
    /// It ran. Whatever it moved, the git-dir watcher reports — this answer is not
    /// a reason to refresh.
    Fetched,
    /// Something else was running on the repository; it gave way.
    Busy,
    /// A merge / rebase / cherry-pick / revert / bisect is unfinished.
    Operation,
    /// The repository has no remotes: nothing to fetch.
    NoRemotes,
}

/// One configured remote (`engine::remotes::list`). URLs come masked
/// (`exec::mask_credentials`): a token stored in a URL before Graft refused them is
/// not put on screen. `push_urls` empty means pushes go to `fetch_urls`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteInfo {
    pub name: String,
    pub fetch_urls: Vec<String>,
    pub push_urls: Vec<String>,
    /// Some URL of this remote carries a password or token in it.
    pub has_credentials: bool,
    /// Remote-tracking branches under `refs/remotes/<name>/` — what removing the
    /// remote deletes; the confirmation names the number first.
    pub branches: u32,
}

/// Payload of the `repo-clone-progress` event: one line of `git clone --progress`
/// (a `\r`-redrawn meter arrives as its successive states), masked.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneProgress {
    pub line: String,
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

/// Which rule "Ignore" writes for an untracked path (`engine::ignore`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IgnoreKind {
    /// This one path, anchored at the root: `/<path>`.
    File,
    /// Every file with its extension, anywhere: `*.<ext>`.
    Extension,
    /// The folder it is in (or the folder entry itself): `/<folder>/`.
    Folder,
}

/// One entry of the "Ignore" menu: the kind and the exact line it would write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IgnoreChoice {
    pub kind: IgnoreKind,
    pub pattern: String,
}

/// Someone who authored commits in this history, offered as a co-author
/// (`log_co_authors`). One entry per address, compared without case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoAuthor {
    pub name: String,
    pub email: String,
    /// Commits authored with this address in the walked history.
    pub commits: u32,
}

/// One page of the log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    pub commits: Vec<LogCommit>,
    pub next_cursor: Option<LogCursor>,
    pub lane_overflow: bool,
}

/// A commit that touched one file, as the file history lists it
/// (`engine::file_history`). The log's row fields, minus the graph, plus where the
/// file was in **this** commit: `--follow` crosses renames, and the diff of an
/// older row must be asked for under the name the file had then.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryCommit {
    pub hash: String,
    pub short_hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub author_email: String,
    pub author_at: i64,
    pub subject: String,
    pub refs: Vec<RefLabel>,
    /// The file's path in this commit (the new name on the rename commit itself).
    pub path: String,
    /// The name before the change, on the commit that renamed the file.
    pub old_path: Option<String>,
    pub status: FileState,
}

/// Where the next page of a file history starts. Opaque to the client: handed
/// back verbatim. `anchor` pins the history to the commit the first page was read
/// from — see `engine::file_history` for why this is not the log's cursor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryCursor {
    pub skip: u32,
    pub anchor: String,
}

/// One page of a file history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryPage {
    pub commits: Vec<FileHistoryCommit>,
    pub next_cursor: Option<FileHistoryCursor>,
}

/// Why a file cannot be blamed (`engine::blame`). A machine key, not prose, for
/// the reason `EditBlock` is one: the text is assembled from both dictionaries,
/// and the reason is an answer to show in place of the lines, not a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlameBlock {
    /// A NUL byte — what git itself calls binary; its "lines" are not text.
    Binary,
    /// Over `BLAME_SIZE_CEILING` bytes or `BLAME_LINE_CEILING` lines.
    TooLarge,
    /// No file at this path in that revision (or on disk), or not a file there.
    Missing,
    /// On disk, but git does not know it: there is no history to blame.
    Untracked,
}

/// Where a line's commit took it from: the commit's parent that git blamed
/// further, and the path the file had there — a rename is already followed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlamePrevious {
    pub hash: String,
    pub path: String,
}

/// One commit a line is attributed to, as `--line-porcelain` describes it. Listed
/// once however many lines point to it; lines refer to it by index.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameOrigin {
    pub hash: String,
    pub short_hash: String,
    /// All parents of the commit (empty for a root, a shallow edge and an
    /// uncommitted line) — `DiffSource.parent` needs the real one, not a guess.
    pub parents: Vec<String>,
    pub author: String,
    pub author_email: String,
    pub author_at: i64,
    pub summary: String,
    /// The file's path **in this commit**.
    pub path: String,
    /// `None`: the file was created here, or the history ends here (`boundary`).
    pub previous: Option<BlamePrevious>,
    /// The earliest version reachable here — a root commit, or the edge of a
    /// shallow clone. Nothing can be blamed before it.
    pub boundary: bool,
    /// The all-zero hash of a working-tree blame: the line is not committed yet.
    pub uncommitted: bool,
}

/// One line of the blamed file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameLine {
    /// Line number in the blamed file, 1-based.
    pub line: u32,
    /// Line number in the file as the origin commit left it.
    pub orig_line: u32,
    pub text: String,
    /// Index into `Blame.origins`.
    pub origin: u32,
}

/// A whole file, blamed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Blame {
    pub path: String,
    /// The commit blamed, as a full hash; `None` — the working tree.
    pub rev: Option<String>,
    pub lines: Vec<BlameLine>,
    pub origins: Vec<BlameOrigin>,
    /// Set, the lines are empty and this says why.
    pub blocked: Option<BlameBlock>,
}

/// "Blame before this change": the blame of the version before a line's commit,
/// and where that line lands in it. `exact` — the line itself is there (it moved,
/// it was not changed); otherwise `from..=to` is what the change replaced, or the
/// line it was inserted after.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameBefore {
    pub blame: Blame,
    pub from: u32,
    pub to: u32,
    pub exact: bool,
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
    /// Author names to keep, OR'd together — `git log` accepts `--author` more
    /// than once and matches a commit whose author satisfies any of them. A
    /// single `Option<String>` was the one shape that could not say "these two
    /// people", which is the question a log filter is most often asked.
    pub authors: Vec<String>,
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

/// The kind of a commit signature, read from its armour line (`engine::signature`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SignatureFormat {
    Openpgp,
    Ssh,
    X509,
    Unknown,
}

/// What checking a commit's signature gave. `%G?` letters, except `Unsigned` (no
/// `gpgsig` header) and `Unchecked` (a signature git did not check: `N` for a signed
/// commit — no allowed signers file, no `gpg` — or a verifier that failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignatureStatus {
    Unsigned,
    /// `G`
    Verified,
    /// `U`: valid, but the key is not trusted / not in the allowed signers file.
    UnknownKey,
    /// `E`: cannot be checked, typically the public key is missing.
    MissingKey,
    /// `X`: a good signature that has expired.
    Expired,
    /// `Y`: a good signature made by a key that has expired.
    ExpiredKey,
    /// `R`: a good signature made by a revoked key.
    Revoked,
    /// `B`
    Bad,
    Unchecked,
}

/// The signature of one commit (`commit_signature`). Asked for the open commit
/// only: verifying runs gpg / ssh-keygen / gpgsm.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitSignature {
    pub status: SignatureStatus,
    /// `None` exactly when `Unsigned`.
    pub format: Option<SignatureFormat>,
    /// `%GS`: who signed, as the verifier names them.
    pub signer: Option<String>,
    /// `%GK`: the key (id, or the SSH key's fingerprint).
    pub key: Option<String>,
    /// `%GF`: the key's fingerprint.
    pub fingerprint: Option<String>,
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
    /// `git bisect` — the search for the commit that brought a bug in. The lowest
    /// priority of all: a cherry-pick stopped on a conflict in the middle of a
    /// bisect is reported as the cherry-pick, and `OperationState.bisect` still
    /// carries the search.
    Bisect,
}

/// State of an unfinished operation. `kind: None` means the repository is calm.
///
/// `conflicted` names every unmerged path with what kind of conflict it is — the
/// same list, from the same `ls-files -u`, whether an operation is running or not;
/// with `kind: None` it is left empty (a conflict with no operation — a `stash pop`
/// that collided — is visible in the Changes panel instead).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationState {
    pub kind: OperationKind,
    pub current: Option<u32>,
    pub total: Option<u32>,
    pub conflicted: Vec<ConflictEntry>,
    /// A rebase stopped on an `edit` step: the (original) hash of the commit it
    /// stopped at, so the strip can say "amend it, then continue" instead of "no
    /// conflicts". Read from the last line of `rebase-merge/done`, not from the
    /// `amend` marker: git writes that one for a failed squash too.
    #[serde(default)]
    pub edit_stop: Option<String>,
    /// The bisect under way, whatever `kind` says: filled whenever git's
    /// `BISECT_START` exists, so a merge stopped inside a bisect does not hide the
    /// search (`kind` names the innermost operation, the one to finish first).
    #[serde(default)]
    pub bisect: Option<BisectState>,
}

/// A `git bisect` in progress, read by `engine::bisect` from the files git keeps
/// for it (`BISECT_START`, `BISECT_TERMS`, `BISECT_LOG`) and from `refs/bisect/*`.
///
/// `term_bad` / `term_good` are the repository's words — `bad` / `good` unless the
/// bisect was started with `--term-new` / `--term-old` (or `--term-bad` / `--term-good`).
/// Every hash is a full object id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BisectState {
    pub term_bad: String,
    pub term_good: String,
    /// The branch `git bisect reset` goes back to; `None` when the bisect started
    /// on a detached HEAD — then `start_commit` is where it goes back to.
    pub start_branch: Option<String>,
    pub start_commit: Option<String>,
    /// The commit under test: what `git bisect good` without a revision marks —
    /// HEAD, or `BISECT_HEAD` for a `--no-checkout` bisect.
    pub current: Option<String>,
    pub current_subject: Option<String>,
    /// The bad end of the range (`refs/bisect/<term_bad>`); git keeps only the
    /// newest bad answer.
    pub bad: Option<String>,
    pub good: Vec<String>,
    pub skip: Vec<String>,
    /// The answer: the first bad commit, once git has named it and no later mark
    /// reopened the search.
    pub first_bad: Option<String>,
    pub first_bad_subject: Option<String>,
    /// Only skipped commits were left: the first bad commit is one of these.
    pub candidates: Vec<String>,
    /// Revisions left to test after the current one, and roughly how many steps
    /// that takes — `git rev-list --bisect-vars`, the numbers git itself prints.
    /// `None` while either end of the range is unknown, or once it is over.
    pub remaining: Option<u32>,
    pub steps: Option<u32>,
    /// `BISECT_LOG` or `refs/bisect/*` could not be read: the parse error, word for
    /// word. The bisect is still reported (so it can be ended); the marks are not.
    pub problem: Option<String>,
}

/// One command of an interactive rebase plan (git's todo verbs, `break`/`exec`
/// and friends left out — the dialog does not offer them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RebaseAction {
    Pick,
    Reword,
    Edit,
    Squash,
    Fixup,
    Drop,
}

/// One line of the plan the user approved, oldest first. `message` is the new
/// text of a `reword`, or — on a `squash` / `fixup` — the text of the commit the
/// whole chain melds into (`engine::rebase::compile` says where it is applied).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RebaseStep {
    pub hash: String,
    pub action: RebaseAction,
    #[serde(default)]
    pub message: Option<String>,
}

/// A commit an interactive rebase would replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RebaseCommit {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub author_at: i64,
    pub subject: String,
    /// The whole message (`%B`), for prefilling a reword or a squash.
    pub message: String,
}

/// Why history from a commit cannot be rewritten by an interactive rebase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RebaseBlock {
    /// The commit is not an ancestor of HEAD.
    NotOnBranch,
    /// A merge commit lies between the commit and HEAD: a plain rebase would
    /// flatten it.
    Merge,
    /// More commits than a plan can hold (`engine::rebase::MAX_STEPS`).
    TooMany,
}

/// What rewriting history from one commit up to HEAD would replay, and whether it
/// may. Asked before the menu is drawn and before the dialog opens.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RebaseRange {
    /// HEAD's full hash when the range was read.
    pub head: String,
    /// First parent of the commit — the rebase base; `None` for a root commit.
    pub base: Option<String>,
    /// Oldest first; empty when `blocked`.
    pub commits: Vec<RebaseCommit>,
    pub blocked: Option<RebaseBlock>,
    /// Tracked files carry uncommitted changes (untracked files do not count).
    pub dirty: bool,
    /// How many of the replayed commits the upstream of the current branch already
    /// has — rewriting them means a force push.
    pub published: u32,
}

/// What kind of conflict an unmerged path is in — git's own seven, named after the
/// two letters `git status` prints for them. Derived from which index stages exist
/// (1 base, 2 ours, 3 theirs), the same rule git uses for those letters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictKind {
    /// `UU` — stages 1, 2, 3: both sides changed the file.
    BothModified,
    /// `AA` — stages 2, 3: both sides added it, there is no base.
    BothAdded,
    /// `DU` — stages 1, 3: we deleted it, they changed it.
    DeletedByUs,
    /// `UD` — stages 1, 2: we changed it, they deleted it.
    DeletedByThem,
    /// `AU` — stage 2 only: we added it (the other side renamed onto it, typically).
    AddedByUs,
    /// `UA` — stage 3 only: they added it.
    AddedByThem,
    /// `DD` — stage 1 only: both deleted it (typically a rename on both sides).
    BothDeleted,
}

/// One unmerged path and the kind of its conflict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictEntry {
    pub path: String,
    pub kind: ConflictKind,
}

/// One side of a conflict as the index holds it (stage 1, 2 or 3).
///
/// `text` is the blob normalised to `\n`, or `None` when it is not text: `blocked`
/// then says why (`binary`, `too-large`), or `mode` does — a symlink (`120000`) or a
/// submodule (`160000`) is resolved whole, never line by line.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictSide {
    pub text: Option<String>,
    pub blocked: Option<EditBlock>,
    pub mode: String,
}

/// Everything the conflict editor needs about one unmerged path.
///
/// A side that is `None` does not exist in the index — the file was deleted on that
/// side (`DeletedByUs` has no `ours`), or never existed there (`BothAdded` has no
/// `base`). `worktree` is the file with git's markers in it, read exactly as the
/// in-place editor reads a file, so its `digest` is what a save must hand back.
/// `markerSize` is the `conflict-marker-size` attribute of the path (7 unless set).
/// `wholeOnly` is the single answer to "can this be resolved line by line": false
/// only when every present side and the working file are text.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictFile {
    pub path: String,
    pub kind: ConflictKind,
    pub base: Option<ConflictSide>,
    pub ours: Option<ConflictSide>,
    pub theirs: Option<ConflictSide>,
    pub worktree: TextFile,
    pub marker_size: u32,
    pub whole_only: bool,
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

/// Line endings of a working-tree file, as they lie on disk.
///
/// Inbound as well as outbound: `file_write` takes it back so the text the webview
/// edited in `\n` is restored to what the file actually had. It is the *only* thing
/// the write derives — the tail of the text, blank lines and final newline alike, is
/// written exactly as given. A file with *mixed*
/// endings has no value here — it is refused for editing instead (`EditBlock::MixedEol`),
/// because rewriting it would normalise every line and produce a whole-file diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Eol {
    Lf,
    Crlf,
}

/// Why a working-tree file cannot be edited in place. A machine key, not prose:
/// the message is assembled in the frontend from both `i18n` dictionaries, unlike
/// `Error::Rule`, whose text deliberately stays English in both locales.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EditBlock {
    Binary,
    TooLarge,
    MixedEol,
    Missing,
}

/// A working-tree file read for in-place editing.
///
/// `text` is normalised to `\n` — a textarea works in `\n`, and handing out `\r\n`
/// would show `^M`. `digest` is the fingerprint of the bytes **as they lie on disk**,
/// which is the only way a later write can tell that the file changed underneath;
/// every successful write returns a new one and the client keeps the *latest*.
///
/// `finalNewline` is information for the UI, not an instruction to the write: `text`
/// already carries its own trailing newline, or carries none.
/// When `blocked` is set, `text` is `None` and `digest` is empty — there is nothing
/// on disk to fingerprint (`Missing`) or the bytes were deliberately not read
/// (`TooLarge`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextFile {
    pub text: Option<String>,
    pub digest: String,
    pub eol: Eol,
    pub final_newline: bool,
    pub blocked: Option<EditBlock>,
}

/// What a successful `file_write` reports back: the fingerprint of what now lies on
/// disk. The client **must** replace its previous digest with it, or the next
/// automatic save would compare against a digest its own write already made stale.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileWritten {
    pub digest: String,
}

/// What `git_exec` (the git console panel) reports back: both streams and the
/// exit code of the command the user typed, plus the `RepoState` after running
/// it — an arbitrary git command can change anything, so this carries the same
/// full state every other mutation does (see CLAUDE.md "Мутация возвращает
/// целиком RepoState"), just alongside the output the panel needs to show.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    /// The journal entry of this run, so the console can open it (0: not journaled).
    pub journal_id: u64,
    pub state: RepoState,
}

/// Who a journaled git run was for: a person's action, or the application reading
/// state for itself. Declared at the call site, see `engine::exec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JournalOrigin {
    User,
    Background,
}

/// One git run as `journal_list` reports it — everything but the output, which can
/// be half a megabyte per entry and is fetched on demand by `journal_output`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalSummary {
    /// Monotonic for the life of the process; `journal_list(after)` pages on it.
    pub id: u64,
    /// The `-C` directory the command ran in — the repository, and its cwd.
    pub repo: String,
    /// Arguments after `git -C <repo>`, credentials masked.
    pub argv: Vec<String>,
    /// Milliseconds since the Unix epoch.
    pub started_at: u64,
    pub duration_ms: u64,
    /// `None`: git never started, or was ended by a signal.
    pub exit_code: Option<i32>,
    pub origin: JournalOrigin,
    /// The Tauri command a user run belonged to (`branch_rename`, `git_exec`, …).
    pub action: Option<String>,
}

/// Both streams of one journal entry, masked and cut at `limit_bytes` each: 256 KB
/// for a user action or a failed run, 16 KB for a successful background read.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalOutput {
    pub id: u64,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// The per-stream limit this entry was kept under.
    pub limit_bytes: u64,
}

/// What a discard backup was taken for (`engine::discard`). Travels as the
/// `Graft-Kind` trailer of the backup commits, so `git log refs/graft/discard` says it
/// in words as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscardKind {
    /// Files rolled back to HEAD (`file_rollback`).
    Files,
    /// A whole changelist rolled back to HEAD (`list_rollback`).
    List,
    /// Whole hunks reverted in the working tree (`lines_revert` with `"all"`).
    Hunk,
    /// Chosen lines reverted in the working tree (`lines_revert`).
    Lines,
    /// A backup restored — itself undoable, from the same list.
    Restore,
}

/// One restorable backup: the working-tree paths a discard changed, as they were
/// before it. `id` is the backup's "after" commit under `refs/graft/discard`, the
/// handle `discard_check` / `discard_restore` take back. `at` is Unix seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscardEntry {
    pub id: String,
    pub at: i64,
    pub kind: DiscardKind,
    pub paths: Vec<String>,
}

/// Result of a discard or a restore: the fresh state, as every mutation returns
/// ("Мутация возвращает целиком RepoState"), plus the backup it took — `None` when
/// nothing on disk changed, so there is nothing to offer back. The client runs it
/// through `runWithOutput`, as it does `GitExecResult`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscardOutcome {
    pub state: RepoState,
    pub backup: Option<DiscardEntry>,
}

// ── Undo / Redo of the application's own actions (`engine::undo`) ────────────

/// Which way `undo_step` moves along the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UndoDirection {
    Undo,
    Redo,
}

/// Why Undo (or Redo) is not available. A code, never prose: every visible string
/// lives in `src/i18n.ts`, and the client words each code in both locales.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UndoReasonCode {
    /// Nothing recorded yet in this repository.
    Empty,
    /// An action (or an Undo / Redo) is running on the repository right now.
    Busy,
    /// The repository changed outside a recorded action (terminal, editor, another
    /// tool) — the chain ended.
    External,
    /// A merge / rebase / cherry-pick / revert was unfinished before or after.
    Operation,
    /// The action failed after changing the repository.
    Failed,
    /// The repository could not be read to check it.
    Unverifiable,
    /// Two actions ran at once, so neither can be told from the other.
    Concurrent,
    /// A push published commits or deleted a remote branch.
    Published,
    /// A fetch changed more than remote-tracking refs (new tags).
    Fetched,
    /// A pull or an update from upstream brought commits in.
    Integrated,
    /// A rebase (or anything else rewriting history) ran.
    History,
    /// A command typed in the git console changed the repository.
    Console,
    /// The action has no safe inverse.
    Unsupported,
    /// A hard reset or a merge with uncommitted changes around it: reversing it
    /// by `reset --hard` would lose them.
    Dirty,
    /// The action changed files in the working tree it should not have (a hook).
    Worktree,
    /// A stash other than the newest was popped or dropped; Undo cannot put it
    /// back at its place.
    StashPosition,
    /// A stash was restored onto local changes; the two cannot be told apart.
    StashDirty,
    /// Only for one side: the other side has a step, this one does not.
    NoEarlier,
    NoNext,
    /// An Undo / Redo stopped halfway; the repository needs a look.
    InverseFailed,
    /// A bisect was under way before or after the action: its checkouts move
    /// HEAD through history on git's schedule, not the user's.
    Bisect,
    /// A remote was renamed or removed: its remote-tracking branches went with it
    /// and the branches tracking it were re-pointed or unset, which an inverse that
    /// puts an upstream back (a deleted branch's) relies on.
    Remotes,
}

/// `action` names the command (`push`, `branch_rebase_onto`, …) where the reason is
/// about one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoReason {
    pub code: UndoReasonCode,
    #[serde(default)]
    pub action: Option<String>,
}

/// One direction of the journal as the toolbar shows it. `id` is set when the
/// direction is available and names the step `undo_step` must be given back, so a
/// confirmation shown for one step can never run another. `action` is the Tauri
/// command that made the step, `detail` what it acted on (a commit subject, a branch,
/// a path). `destructive` asks for a confirmation (`reset --hard`), and
/// `lost_commits` says how many commits leave the branch then.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoSide {
    pub id: Option<u64>,
    pub action: Option<String>,
    pub detail: Option<String>,
    pub destructive: bool,
    pub lost_commits: u32,
    pub reason: Option<UndoReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoState {
    pub undo: UndoSide,
    pub redo: UndoSide,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `api.ts` sends: `lines` is an array of indexes or the string
    /// `"all"`; anything else is refused at the boundary, not read as "all".
    #[test]
    fn hunk_picks_read_the_client_shape() {
        let v: Vec<HunkPick> =
            serde_json::from_str(r#"[{"hunk":0,"lines":"all"},{"hunk":2,"lines":[1,3]}]"#).unwrap();
        assert!(matches!(v[0].lines, LinePick::All(AllLines::All)));
        assert!(matches!(&v[1].lines, LinePick::Lines(l) if l == &[1, 3]));
        assert_eq!(v[1].hunk, 2);
        assert!(serde_json::from_str::<Vec<HunkPick>>(r#"[{"hunk":0,"lines":"some"}]"#).is_err());
    }

    /// A file of a changelist reaches `api.ts` as `FileStatus` — camelCase like
    /// every boundary type. In snake_case `old_path` arrived where the client reads
    /// `oldPath`, and a staged rename lost its old name on the way: the file
    /// history opened from Changes asked for the new name, which HEAD has never seen.
    #[test]
    fn a_changelist_file_crosses_the_boundary_in_camel_case() {
        let view = ChangelistView {
            id: "default".into(),
            name: "Changes".into(),
            comment: String::new(),
            is_default: true,
            is_unversioned: false,
            is_ignored: false,
            files: vec![FileStatus {
                path: "new.txt".into(),
                status: FileState::Renamed,
                old_path: Some("old.txt".into()),
                staged: true,
                unstaged: false,
            }],
        };
        let json = serde_json::to_value(&view).unwrap();
        let file = &json["files"][0];
        assert_eq!(file["oldPath"], "old.txt", "the name api.ts reads: {file}");
        assert!(file.get("old_path").is_none(), "no snake_case twin: {file}");
        for key in ["path", "status", "staged", "unstaged"] {
            assert!(file.get(key).is_some(), "{key} missing: {file}");
        }
        // no old name — the key is simply absent, which api.ts types as optional
        let plain = FileStatus { old_path: None, ..view.files[0].clone() };
        assert!(serde_json::to_value(&plain).unwrap().get("oldPath").is_none());
    }
}
