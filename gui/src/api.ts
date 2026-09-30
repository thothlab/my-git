import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

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
// git failures also carry the underlying output (credentials masked) and the id of
// the failed run's entry in the command journal.
export interface BackendError {
  kind: "git" | "io" | "parse" | "rule" | "stale";
  message: string;
  stderr?: string | null;
  journalId?: number | null;
}

/** The journal entry of a failed git run, when the error is one. */
export function errJournalId(e: unknown): number | null {
  const be = e as Partial<BackendError> | undefined;
  return be && typeof be === "object" && typeof be.journalId === "number" ? be.journalId : null;
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

/** Payload of the `repo-external-change` event (`src-tauri/src/watch.rs`):
 *  something outside Graft moved refs, HEAD or an operation marker. `repoPath`
 *  is spelled exactly as `RepoState.repoPath`. */
export interface RepoExternalChange {
  repoPath: string;
}

/** Not a command: the backend's git-dir watcher pushes this on its own. */
export const onRepoExternalChange = (
  cb: (e: RepoExternalChange) => void,
): Promise<UnlistenFn> =>
  listen<RepoExternalChange>("repo-external-change", (e) => cb(e.payload));

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

// rollback (task_03) — every discard is backed up first under refs/graft/discard
export type DiscardKind = "files" | "list" | "hunk" | "lines" | "restore";

/** One restorable backup: the paths a discard changed, as they were before it.
 * `id` is the handle `discardCheck` / `discardRestore` take; `at` is Unix seconds. */
export interface DiscardEntry {
  id: string;
  at: number;
  kind: DiscardKind;
  paths: string[];
}

/** A discard or a restore: the fresh state plus the backup it took (`null` when
 * nothing on disk changed). Run through `runWithOutput`, like `gitExec`. */
export interface DiscardOutcome {
  state: RepoState;
  backup: DiscardEntry | null;
}

export const fileRollback = (paths: string[]) =>
  invoke<DiscardOutcome>("file_rollback", { paths });

export const listRollback = (id: string) =>
  invoke<DiscardOutcome>("list_rollback", { id });

/** Restorable backups, newest first. Read-only. */
export const discardList = (limit: number) =>
  invoke<DiscardEntry[]>("discard_list", { limit });

/** Read-only: paths of backup `id` changed since the discard (empty: safe to restore). */
export const discardCheck = (id: string) => invoke<string[]>("discard_check", { id });

/** Put the files of backup `id` back as they were before the discard. Without
 * `force` the backend refuses (`stale`) over files changed since. */
export const discardRestore = (id: string, force: boolean) =>
  invoke<DiscardOutcome>("discard_restore", { id, force });

// diff & hunk staging (task_04)
export type DiffBase = "worktree" | "index" | "head";

export interface DiffLine {
  origin: " " | "+" | "-";
  content: string;
  oldNo: number | null;
  newNo: number | null;
}
/** A hunk carries no patch text: actions name hunks and lines by index
 * (`HunkPick`) and the backend rebuilds the patch from the diff it reads again. */
export interface Hunk {
  header: string;
  lines: DiffLine[];
}
export interface FileDiff {
  path: string;
  binary: boolean;
  /** Fingerprint of the exact diff git printed, for a working-tree / index diff;
   * a line action sends it back and is refused as `stale` when the diff read
   * again is another one. Empty for a revision diff. */
  digest: string;
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

/** Lines of one hunk: indexes into `Hunk.lines`, or the whole hunk. */
export interface HunkPick {
  hunk: number;
  lines: number[] | "all";
}

/**
 * Stage / unstage / revert the chosen lines of `path`. `digest` and `context` are
 * those of the diff the lines were chosen in — the worktree diff for stage and
 * revert, the index diff for unstage; the backend reads that diff again with the
 * same context (and never with whitespace ignored) and refuses a changed one as
 * `stale`.
 */
export const linesStage = (path: string, picks: HunkPick[], digest: string, context?: number) =>
  invoke<RepoState>("lines_stage", { path, picks, digest, context });
export const linesUnstage = (path: string, picks: HunkPick[], digest: string, context?: number) =>
  invoke<RepoState>("lines_unstage", { path, picks, digest, context });
export const linesRevert = (path: string, picks: HunkPick[], digest: string, context?: number) =>
  invoke<DiscardOutcome>("lines_revert", { path, picks, digest, context });

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

/** `force` is `--force-with-lease`; `force-hard` is a bare `--force`. */
export type PushMode = "normal" | "upstream" | "force" | "force-hard";
export const push = (mode: PushMode) => invoke<RepoState>("push", { mode });
export const fetchRemote = () => invoke<RepoState>("fetch");
export const pull = () => invoke<RepoState>("pull");

/** What `git_exec` (the git console panel) reports back — see model.rs. */
export interface GitExecResult {
  stdout: string;
  stderr: string;
  exitCode: number;
  /** Journal entry of this run — the console opens it. */
  journalId: number;
  state: RepoState;
}
export const gitExec = (args: string[]) => invoke<GitExecResult>("git_exec", { args });

// ── Command journal (engine/exec.rs) ─────────────────────────────────────────

/** A person's action, or the application reading state for itself. */
export type JournalOrigin = "user" | "background";

/** One git run, without its output — see `JournalSummary` in model.rs. */
export interface JournalSummary {
  /** Monotonic for the life of the process. */
  id: number;
  /** The directory git ran in (`-C`). */
  repo: string;
  /** Arguments after `git`, credentials masked. */
  argv: string[];
  /** Milliseconds since the Unix epoch. */
  startedAt: number;
  durationMs: number;
  /** null: git never started, or was killed by a signal. */
  exitCode: number | null;
  origin: JournalOrigin;
  /** Tauri command a user run belonged to. */
  action?: string | null;
}

export interface JournalOutput {
  id: number;
  stdout: string;
  stderr: string;
  stdoutTruncated: boolean;
  stderrTruncated: boolean;
  /** Per-stream limit the entry was kept under: 256 KB for a user action or a
   *  failed run, 16 KB for a successful background read. */
  limitBytes: number;
}

/** Entries newer than `after`, oldest first: the user ring when `mine`, else
 *  both rings (user 1000, background 2000) merged by id. */
export const journalList = (mine: boolean, after: number | null) =>
  invoke<JournalSummary[]>("journal_list", { mine, after });

/** Both streams of one entry; null once the ring has dropped it. */
export const journalOutput = (id: number) =>
  invoke<JournalOutput | null>("journal_output", { id });

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
  /** Author names, OR'd: a commit is kept if any of them matches. */
  authors: string[];
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
  authors: [],
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
  /** Every unmerged path with the kind of its conflict (`engine::conflict::list`). */
  conflicted: ConflictEntry[];
  /** A rebase stopped on an `edit` step: the original hash of that commit. */
  editStop?: string | null;
}

/** git's seven kinds of conflict, named after the letters `git status` prints
 *  (`UU`, `AA`, `DU`, `UD`, `AU`, `UA`, `DD`) — see `CONFLICT_CODES`. */
export type ConflictKind =
  | "bothModified"
  | "bothAdded"
  | "deletedByUs"
  | "deletedByThem"
  | "addedByUs"
  | "addedByThem"
  | "bothDeleted";

export interface ConflictEntry {
  path: string;
  kind: ConflictKind;
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
/** `oldPath` — the rename source when the caller already knows it (the file
 * history does); left out, the backend finds it in the commit's file list. */
export const commitFileDiff = (
  hash: string,
  path: string,
  whitespace: WhitespaceMode = "none",
  context?: number,
  oldPath?: string | null,
) =>
  invoke<FileDiff>("commit_file_diff", { hash, path, whitespace, context, oldPath: oldPath ?? null });

// history of one file (R05c) — `engine::file_history`
/** A commit that touched the file. `path` is the file's name **in this commit**
 * (`--follow` crosses renames); `oldPath` is set on the commit that renamed it. */
export interface FileHistoryCommit {
  hash: string;
  shortHash: string;
  parents: string[];
  author: string;
  authorEmail: string;
  authorAt: number;
  subject: string;
  refs: RefLabel[];
  path: string;
  oldPath: string | null;
  status: FileState;
}
/** Opaque: handed back verbatim. It pins the history to the commit the first
 * page was read from, so later pages never shift. */
export interface FileHistoryCursor {
  skip: number;
  anchor: string;
}
export interface FileHistoryPage {
  commits: FileHistoryCommit[];
  nextCursor: FileHistoryCursor | null;
}
/** `rev` — where the history starts (`null` — HEAD). Merge commits are not
 * listed, as with `git log --follow`. */
export const fileHistory = (
  path: string,
  rev: string | null,
  cursor: FileHistoryCursor | null,
  limit: number,
) => invoke<FileHistoryPage>("file_history", { path, rev, cursor, limit });
// blame of one file (R05b) — `engine::blame`
/** Why a file cannot be blamed; the lines are empty then. */
export type BlameBlock = "binary" | "too-large" | "missing" | "untracked";
/** The commit a line of the file is attributed to, and where it came from. */
export interface BlameOrigin {
  hash: string;
  shortHash: string;
  /** Empty for a root, a shallow edge and an uncommitted line. */
  parents: string[];
  author: string;
  authorEmail: string;
  authorAt: number;
  summary: string;
  /** The file's path **in this commit** — a rename is already followed. */
  path: string;
  /** The version git blamed further; `null` — the file was created here, or the
   * history ends here (`boundary`). */
  previous: { hash: string; path: string } | null;
  /** The earliest version reachable here (root commit or shallow edge). */
  boundary: boolean;
  /** The all-zero hash of a working-tree blame. */
  uncommitted: boolean;
}
export interface BlameLine {
  line: number;
  /** The line's number in the file as its origin commit left it. */
  origLine: number;
  text: string;
  /** Index into `Blame.origins`. */
  origin: number;
}
export interface Blame {
  path: string;
  /** Full hash blamed; `null` — the working tree. */
  rev: string | null;
  lines: BlameLine[];
  origins: BlameOrigin[];
  blocked: BlameBlock | null;
}
/** The blame of the version before a line's commit, and where the line lands:
 * `exact` — the line itself; else `from..to` is what the change replaced, or the
 * line it was inserted after. */
export interface BlameBefore {
  blame: Blame;
  from: number;
  to: number;
  exact: boolean;
}
/** `rev` — any revision naming a commit; `null` blames the working tree. */
export const fileBlame = (path: string, rev: string | null) =>
  invoke<Blame>("file_blame", { path, rev });
/** Step back from line `line` (its `origLine`) of `path` at `hash` into the
 * version its `previous` names. */
export const fileBlameBefore = (
  hash: string,
  path: string,
  line: number,
  prevHash: string,
  prevPath: string,
) => invoke<BlameBefore>("file_blame_before", { hash, path, line, prevHash, prevPath });

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

// interactive rebase, reword, squash (twig port, task 8) — `engine::rebase`

/** git's todo verbs the plan dialog offers. */
export type RebaseAction = "pick" | "reword" | "edit" | "squash" | "fixup" | "drop";

/** One line of an approved plan, oldest first. `message` is the new text of a
 *  `reword`, or — on a `squash` / `fixup` — the text of the commit the whole chain
 *  melds into. At most one message per chain; the backend refuses more. */
export interface RebaseStep {
  hash: string;
  action: RebaseAction;
  message?: string | null;
}

export interface RebaseCommit {
  hash: string;
  shortHash: string;
  author: string;
  authorAt: number;
  subject: string;
  /** The whole message, for prefilling a reword or a squash. */
  message: string;
}

/** Why history from a commit cannot be rewritten by an interactive rebase. */
export type RebaseBlock = "notOnBranch" | "merge" | "tooMany";

/** What rewriting from one commit (inclusive) up to HEAD would replay. */
export interface RebaseRange {
  /** HEAD's full hash when the range was read. */
  head: string;
  /** First parent of the commit; `null` for a root commit (`--root`). */
  base: string | null;
  /** Oldest first; empty when `blocked`. */
  commits: RebaseCommit[];
  blocked: RebaseBlock | null;
  /** Tracked files carry uncommitted changes — a rebase refuses them. */
  dirty: boolean;
  /** Commits of the range the upstream already has: rewriting them needs a force push. */
  published: number;
}

/** Read-only: asked when the log's menu opens and before the plan dialog shows. */
export const opRebaseRange = (hash: string) =>
  invoke<RebaseRange>("op_rebase_range", { hash });
/** Replay `hash` (inclusive) up to HEAD by the plan, oldest first. */
export const opRebaseStart = (hash: string, steps: RebaseStep[]) =>
  invoke<RepoState>("op_rebase_start", { hash, steps });
/** New message for one commit: HEAD by `commit --amend --only` (the index stays
 *  out of it), an older commit through a one-`reword` rebase. */
export const commitReword = (hash: string, message: string) =>
  invoke<RepoState>("commit_reword", { hash, message });
/** Meld a run of consecutive commits on HEAD's line into one. */
export const commitsSquash = (hashes: string[], message: string) =>
  invoke<RepoState>("commits_squash", { hashes, message });

export const opContinue = () => invoke<RepoState>("op_continue");

// conflict resolution (R05e)

/** One side of a conflict as the index holds it. `text` in `\n`, or `null` —
 *  then `blocked` says why, or `mode` does (`120000` symlink, `160000` submodule). */
export interface ConflictSide {
  text: string | null;
  blocked: EditBlock | null;
  mode: string;
}

/** A conflicted path for the conflict editor. A `null` side does not exist in
 *  the index — the file was deleted there, or never had a base. `worktree` is the
 *  file with git's markers, read as `file_read` reads it: its `digest` is what a
 *  save must hand back. `wholeOnly`: no line-level resolution is possible. */
export interface ConflictFile {
  path: string;
  kind: ConflictKind;
  base: ConflictSide | null;
  ours: ConflictSide | null;
  theirs: ConflictSide | null;
  worktree: TextFile;
  markerSize: number;
  wholeOnly: boolean;
}

export const conflictRead = (path: string) => invoke<ConflictFile>("conflict_read", { path });

/** Mark `path` resolved (`git add`), writing `text` first when given. `expect` is
 *  the digest the editor last saw — a file changed underneath is `kind: "stale"`
 *  and nothing is staged; `null` takes the file as it lies. */
export const conflictResolve = (
  path: string,
  text: string | null,
  eol: Eol,
  expect: string | null,
) => invoke<RepoState>("conflict_resolve", { path, text, eol, expect });

/** Resolve `path` by one whole side; the side that deleted the file resolves to
 *  the deletion. */
export const conflictTake = (path: string, side: "ours" | "theirs") =>
  invoke<RepoState>("conflict_take", { path, side });
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

// in-place editing of a working-tree file (prd_03)

/** Line endings of the file as they lie on disk; `file_write` takes it back so
 *  the text the webview edited in `\n` is restored to what the file had. */
export type Eol = "lf" | "crlf";
/** Why a working-tree file cannot be edited in place. A machine key: the message
 *  is assembled from both `i18n` dictionaries, unlike an `Error::Rule` text. */
export type EditBlock = "binary" | "too-large" | "mixed-eol" | "missing";

export interface TextFile {
  /** Content with line endings normalised to `\n`; `null` when `blocked`. */
  text: string | null;
  /** Fingerprint of the bytes as they lie on disk; `""` when `blocked`. */
  digest: string;
  eol: Eol;
  /** Information for the UI. The write does **not** derive the tail from it —
   *  `text` already carries its own trailing newline, or carries none. */
  finalNewline: boolean;
  blocked: EditBlock | null;
}

export interface FileWritten {
  /** Fingerprint of what now lies on disk. The client **must** replace its
   *  previous digest with it, or its own next write would look stale. */
  digest: string;
}

export const fileRead = (path: string) => invoke<TextFile>("file_read", { path });

/** Write `text` back to `path`, refusing when the bytes on disk no longer
 *  fingerprint to `expect`. `expect: ""` means "the file must not be there" and
 *  creates it — which is how "overwrite" recreates a file deleted underneath. */
export const fileWrite = (path: string, text: string, eol: Eol, expect: string) =>
  invoke<FileWritten>("file_write", { path, text, eol, expect });

/**
 * Is this failure the one the user can answer — the file changed underneath?
 *
 * Keyed on `kind` and on nothing else. Matching the prose would look like it
 * works on a hand check and break on the first rewording, translation included,
 * and the branch behind it is real: the user is offered a choice of two actions.
 */
export const isStaleError = (e: unknown): boolean =>
  !!e && typeof e === "object" && (e as Partial<BackendError>).kind === "stale";
