import { createSignal } from "solid-js";

// Lightweight in-app i18n. Two locales, English is the default. Each entry is a
// function so interpolated strings share the same shape as static ones, and the
// `const ru: Dict` annotation makes TypeScript reject a missing or mistyped key.
//
// Reactivity: `d()` reads the `locale` signal, so `{d().refresh()}` in JSX (or
// any tracked scope) re-renders on a language switch. Do NOT hoist `d().x()`
// into a module-level constant — it would freeze at import time. Backend
// (Error::Rule) messages stay English in both locales by design.

export type Locale = "en" | "ru";

/** `ConflictKind` of `api.ts`, spelled out here: this module imports no API types. */
type ConflictKindKey =
  | "bothModified"
  | "bothAdded"
  | "deletedByUs"
  | "deletedByThem"
  | "addedByUs"
  | "addedByThem"
  | "bothDeleted";

const stored = localStorage.getItem("locale");
const [locale, setLocaleSignal] = createSignal<Locale>(stored === "ru" ? "ru" : "en");
export { locale };

export function setLocale(l: Locale) {
  setLocaleSignal(l);
  localStorage.setItem("locale", l);
  document.documentElement.lang = l;
}
export function toggleLocale() {
  setLocale(locale() === "ru" ? "en" : "ru");
}
document.documentElement.lang = locale();

// Russian has three plural forms (one / few / many); pick by the standard rule.
const ruPlural = (n: number, one: string, few: string, many: string) => {
  const m10 = n % 10;
  const m100 = n % 100;
  if (m10 === 1 && m100 !== 11) return one;
  if (m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14)) return few;
  return many;
};

const en = {
  // App / error boundary
  uiCrashTitle: () => "Something went wrong in the UI",
  reloadState: () => "Reload state",
  reloadWindow: () => "Reload window",
  // Toolbar
  themeTip: () => "Theme: auto → light → dark",
  refreshTip: () => "Refresh",
  langTip: () => "Language: English / Русский",
  openRepoBtn: () => "Open…",
  openRepoTitle: () => "Open repository",
  recentProjects: () => "Recent projects",
  noRepository: () => "No repository",
  // BranchMenu
  switchDirty: (target: string) => `You have uncommitted changes. Switch to "${target}"?`,
  stashAndSwitch: () => "Stash and switch",
  switchAsIs: () => "Switch as is",
  cancel: () => "Cancel",
  newBranchFromHead: () => "New branch from HEAD",
  filterBranches: () => "filter branches",
  newBranchItem: () => "New branch…",
  local: () => "Local",
  remote: () => "Remote",
  recentBranches: () => "Recent",
  branchMenuOptionsTip: () => "How this list is shown",
  optGroupByPrefix: () => "Group by prefix",
  optShowRemote: () => "Show remote branches",
  optShowRecent: () => "Show recent branches",
  // Modals
  confirm: () => "Confirm",
  // CommitPanel
  commitColon: () => "Commit:",
  selectedCount: (n: number) => ` · ${n} selected`,
  filesCount: (n: number) => `${n} ${n === 1 ? "file" : "files"}`,
  commitMessage: () => "Commit message",
  amendLast: () => "Amend last commit",
  commitAndPushTip: () => "Commit and Push",
  commitBtn: () => "Commit",
  commitPushBtn: () => "+ Push",
  untrackedSelectTip: () => "Untracked files are committed by selection",
  coAuthorsColon: () => "Co-authors:",
  coAuthorAdd: () => "Add from this history…",
  coAuthorRemove: (name: string) => `Remove co-author ${name}`,
  coAuthorsLoading: () => "Reading the authors of this history…",
  coAuthorsNone: () => "Nobody else has authored a commit in this history.",
  coAuthorsNoMatch: () => "No author in this history matches.",
  coAuthorCommits: (n: number) => `${n} ${n === 1 ? "commit" : "commits"}`,
  coAuthorsPreview: () => "Added to the end of the message:",
  // ChangesView
  changes: () => "Changes",
  newListBtn: () => "+ list",
  newChangelist: () => "New changelist",
  cleanTree: () => "No changes — the working tree is clean.",
  active: () => "active",
  rollbackTip: () => "Rollback selected to HEAD",
  rollbackConfirm: (n: number) =>
    `Revert ${n} selected file(s) to HEAD? The files are backed up first and can be restored; what was staged is not.`,
  collapseAll: () => "Collapse all",
  expandAll: () => "Expand all",
  groupByDirTip: () => "Group by directory",
  showIgnoredTip: () => "Show ignored files",
  viewOptionsTip: () => "View options",
  groupByHeader: () => "Group By",
  directory: () => "Directory",
  showHeader: () => "Show",
  ignoredFiles: () => "Ignored Files",
  settings: () => "Settings",
  appMenuTip: () => "Settings, docs and about",
  docs: () => "Docs",
  about: () => "About",
  // Git console
  gitConsole: () => "Git console",
  gitConsoleTip: () => "Git console",
  gitConsoleHint: () => "Runs git here, non-interactively — no editor, no credential prompt.",
  gitConsolePlaceholder: () => "git status",
  gitConsoleBadInput: (msg: string) => `Could not parse the command: ${msg}`,
  gitConsoleExit: (n: number) => `exit ${n}`,
  gitConsoleMine: () => "Mine",
  gitConsoleAll: () => "All",
  gitConsoleMineTip: () => "Only the git commands your actions ran",
  gitConsoleAllTip: () => "Every git command, including the ones Graft runs on its own to read the repository",
  gitConsoleEmptyMine: () => "Nothing you ran yet. Type a git command and press Enter — “All” also shows what Graft runs on its own.",
  gitConsoleEmptyAll: () => "No git commands yet.",
  gitConsoleNotStarted: () => "did not start",
  gitConsoleDuration: (ms: number) => `${ms} ms`,
  gitConsoleCwd: (path: string) => `in ${path}`,
  gitConsoleLoading: () => "Loading output…",
  gitConsoleEvicted: () =>
    "This entry has already left the journal (it keeps your last 1000 commands and Graft's last 2000 reads).",
  gitConsoleNoOutput: () => "No output.",
  gitConsoleTruncated: (kb: number) => `Output truncated at ${kb} KB.`,
  showOutput: () => "Show output",
  phaseGitExec: () => "git",
  themeLabel: () => "Theme",
  themeDesc: () => "Light, dark, or match the system",
  languageLabel: () => "Language",
  languageDesc: () => "Interface language",
  themeAuto: () => "Auto",
  themeLight: () => "Light",
  themeDark: () => "Dark",
  close: () => "Close",
  settingsAppearance: () => "Appearance",
  fontSizeLabel: () => "Font size",
  fontSizeDesc: () => "Size of the main text; smaller labels and rows scale with it",
  fontSizeValue: (px: number) => `${px}px`,
  aboutBlurb: () => "Native desktop git manager (Tauri + SolidJS).",
  sourceOnGithub: () => "Source on GitHub",
  // Self-update (About modal)
  checkForUpdates: () => "Check for updates",
  updChecking: () => "Checking\u2026",
  updUpToDate: () => "You are on the latest version.",
  updFound: (version: string) => `Version ${version} is available.`,
  updUnreachable: () => "Could not reach the update server. Check the connection and try again.",
  updUpdateTo: (version: string) => `Update to v${version}`,
  updInstalling: () => "Installing\u2026",
  updInstallFailed: (message: string) => `Failed to install the update: ${message}`,
  updLastChecked: (time: string) => `Last checked at ${time}`,
  revertFileConfirm: (path: string) => `Revert ${path} to HEAD? The files are backed up first and can be restored; what was staged is not.`,
  revertListConfirm: (name: string) =>
    `Revert all files in "${name}" to HEAD? The files are backed up first and can be restored; what was staged is not.`,
  renameChangelist: () => "Rename changelist",
  deleteListConfirm: (name: string) => `Delete list "${name}"? Files will return to Default.`,
  moveTo: () => "Move to",
  revertToHead: () => "Revert to HEAD",
  ignoreHeader: () => "Ignore",
  ignoreKind: (kind: string): string =>
    kind === "file" ? "This file" : kind === "extension" ? "All files of this type" : "The whole folder",
  makeActive: () => "Make active",
  renameItem: () => "Rename…",
  deleteList: () => "Delete list",
  revertListToHead: () => "Revert list to HEAD",
  // DiffView
  unstaged: () => "Unstaged",
  staged: () => "Staged",
  vsHead: () => "vs HEAD",
  selectFileHint: () => "Select a file on the left to see its diff.",
  diffUnavailable: () => "Diff unavailable for this state.",
  binaryFile: () => "Binary file.",
  noChangesForBase: () => "No changes for this base.",
  revertHunkConfirm: () => "Revert this hunk in the working tree? The file is backed up first and can be restored.",
  revertHunkWideConfirm: () =>
    "The context is expanded, so this hunk is wider than the region shown before expanding. Revert all of it in the working tree? The file is backed up first and can be restored.",
  // StatusBar
  changesCount: (n: number) => `${n} ${n === 1 ? "change" : "changes"}`,
  // Window modes
  modeChanges: () => "Changes",
  modeLog: () => "Log",
  modeChangesTip: () => "Local changes (Cmd/Ctrl+1)",
  modeLogTip: () => "Git history (Cmd/Ctrl+2)",
  focusHint: () => "Tab / Shift+Tab moves focus between panels",
  // Busy phases
  busyFetch: () => "Fetching…",
  busyPull: () => "Pulling…",
  // Undo / Redo of Graft's own actions (src-tauri/src/engine/undo.rs)
  busyUndo: () => "Undoing…",
  busyRedo: () => "Redoing…",
  undoTip: (what: string) => `Undo: ${what} (Cmd/Ctrl+Z)`,
  redoTip: (what: string) => `Redo: ${what} (Cmd/Ctrl+Shift+Z)`,
  undoUnavailable: (why: string) => `Nothing to undo: ${why}`,
  redoUnavailable: (why: string) => `Nothing to redo: ${why}`,
  /** A recorded step as the buttons name it. `action` is the Tauri command. */
  undoWhat: (action: string, detail: string | null): string => {
    const x = detail ?? "";
    const q = detail ? ` "${detail}"` : "";
    switch (action) {
      case "commit_list":
        return `commit${q}`;
      case "commit_reword":
        return `new message${q}`;
      case "commit_reset":
        return `reset to ${x}`;
      case "lines_stage":
        return `staging in ${x}`;
      case "lines_unstage":
        return `unstaging in ${x}`;
      case "branch_checkout":
      case "commit_checkout":
        return `checkout of ${x}`;
      case "branch_create":
        return `new branch ${x}`;
      case "branch_delete":
        return `deleting branch ${x}`;
      case "branch_rename":
        return `renaming ${x}`;
      case "tag_create":
        return `new tag ${x}`;
      case "branch_merge":
        return `merge of ${x}`;
      case "commit_cherry_pick":
        return `cherry-pick${q}`;
      case "commit_revert":
        return `revert${q}`;
      case "stash_push":
        return `stash${q}`;
      case "stash_pop":
        return `stash pop${q}`;
      case "stash_apply":
      case "stash_restore":
        return `stash apply${q}`;
      case "stash_drop":
        return `stash drop${q}`;
      case "file_rollback":
      case "list_rollback":
        return `rollback of ${x}`;
      case "lines_revert":
        return `reverted lines in ${x}`;
      case "discard_restore":
        return `restore of ${x}`;
      case "file_ignore":
        return `ignore rule ${x}`;
      default:
        return `${action}${q}`;
    }
  },
  /** Why Undo / Redo is unavailable — `UndoReasonCode` of `api.ts`. */
  undoReason: (code: string, action: string | null): string => {
    const name = action ?? "the action";
    switch (code) {
      case "empty":
        return "no recorded action yet";
      case "busy":
        return "an action is running";
      case "external":
        return "the repository changed outside a recorded action (a terminal, the file editor), so the history of actions ended";
      case "operation":
        return "an unfinished merge, rebase, cherry-pick or revert was involved";
      case "failed":
        return `${name} failed after changing the repository`;
      case "unverifiable":
        return "the repository could not be read to check it";
      case "concurrent":
        return "two actions ran at once";
      case "published":
        return `${name} published to the remote — that cannot be taken back`;
      case "fetched":
        return "the fetch brought new tags";
      case "integrated":
        return `${name} brought commits in from the remote`;
      case "history":
        return `${name} rewrote history (a rebase)`;
      case "console":
        return "a command in the git console changed the repository";
      case "dirty":
        return `${name} ran next to uncommitted changes — reversing it would lose them`;
      case "worktree":
        return `${name} changed files in the working tree (a hook?)`;
      case "stash-position":
        return "a stash below the newest one was popped or dropped";
      case "stash-dirty":
        return "the stash was restored onto local changes";
      case "no-earlier":
        return "nothing earlier";
      case "no-next":
        return "nothing was undone";
      case "inverse-failed":
        return "the last Undo / Redo stopped halfway — check the repository";
      case "bisect":
        return "a bisect (the search for the commit with the bug) was involved — its checkouts cannot be taken back step by step";
      case "remotes":
        return "a remote was renamed or removed — the branches tracking it changed with it";
      default:
        return `${name} cannot be undone`;
    }
  },
  undoConfirmHard: (redo: boolean, what: string, commits: number) =>
    `${redo ? "Redo" : "Undo"}: ${what}.\n\nThis runs git reset --hard: tracked files are rewritten to the other commit` +
    (commits > 0
      ? `, and ${commits} commit${commits === 1 ? "" : "s"} leave the branch (${redo ? "Undo" : "Redo"} brings them back).`
      : ".") +
    " Untracked files stay as they are.",
  // Log mode — panels
  branchesTitle: () => "Branches",
  logTitle: () => "Log",
  commitDetailsTitle: () => "Commit details",
  diffTitle: () => "Diff",
  changedFiles: () => "Changed files",
  favorites: () => "Favorites",
  detachedHead: (hash: string) => (hash ? `HEAD (detached at ${hash})` : "HEAD (detached)"),
  onBranch: (name: string) => `HEAD → ${name}`,
  // Log mode — empty and special states
  noCommitsTitle: () => "This repository has no commits yet",
  noCommitsHint: () => "Make the first commit in the Changes mode.",
  noRemoteBranches: () => "No remote branches",
  noBranchesYet: () => "No branches to show",
  loadingHistory: () => "Loading history…",
  selectCommitHint: () => "Select a commit to see its details.",
  // Log mode — commit details
  loadingCommitDetails: () => "Loading commit details…",
  authorLabel: () => "Author",
  sigLabel: () => "Signature",
  sigChecking: () => "checking…",
  sigFailed: () => "could not be read",
  sigUnsigned: () => "not signed",
  sigVerified: () => "verified",
  sigUnknownKey: () => "unknown key",
  sigMissingKey: () => "could not check: no public key",
  sigExpired: () => "expired",
  sigExpiredKey: () => "made with an expired key",
  sigRevoked: () => "made with a revoked key",
  sigBad: () => "bad signature",
  sigUnchecked: () => "could not check",
  sigFormatUnknown: () => "unknown format",
  sigHintSshNotListed: () =>
    "The signature is valid, but the allowed signers file (gpg.ssh.allowedSignersFile) does not list this key for this signer.",
  sigHintUntrusted: () => "The signature is valid, but the key is not trusted in your keyring.",
  sigHintSshNoSignersFile: () =>
    "git checks SSH signatures against an allowed signers file: set gpg.ssh.allowedSignersFile.",
  sigHintNoGpg: () => "gpg is not installed or could not run, so git could not check the signature.",
  sigHintNoGpgsm: () => "gpgsm is not installed or could not run, so git could not check the signature.",
  sigHintUnknownFormat: () => "git has no program to check a signature of this kind.",
  sigHintMissingKey: () => "The signer's public key is not in your keyring.",
  forgeOpenCommit: (forge: string) => `Open on ${forge}`,
  forgeAuthorCommits: (forge: string) => `Author's commits on ${forge}`,
  forgeOpenFailed: (why: string) => `Could not open the link: ${why}`,
  committerLabel: () => "Committer",
  inBranches: (n: number) => `In ${n} ${n === 1 ? "branch" : "branches"}`,
  noContainingBranches: () => "No branch contains this commit",
  showAllBranches: (n: number) => `show all ${n}`,
  showFewerBranches: () => "show fewer",
  branchesCapped: () =>
    "Only the first 64 branches were checked — the full list may be longer.",
  noChangedFiles: () => "This commit changed no files",
  treeOnlyTip: () => "Only in the directory tree",
  actionPending: () => "Not available yet — needs the history backend",
  // Log mode — toolbars
  expandAllTip: () => "Expand all",
  collapseAllTip: () => "Collapse all",
  favoritesOnlyTip: () => "Show only favourite branches",
  favoritesShowAllTip: () => "Show all branches again",
  newBranchTip: () => "New branch…",
  noMatches: () => "Nothing matched the filter",
  branchesFailed: () => "Could not read the branches",
  favoritesSection: () => "Favourites",
  favoriteAddTip: () => "Add to favourites — the branch moves to the top of the list (Cmd/Ctrl+D)",
  favoriteRemoveTip: () => "Remove from favourites — the branch goes back to the list below (Cmd/Ctrl+D)",
  noFavorites: () => "No favourite branches",
  noFavoritesHint: () => "Star a branch to keep it at the top of the list",
  fetchPruneTip: () => "Fetch from the remote and refresh the counters",
  fetching: () => "fetch",
  trackingTip: (upstream: string) => `Tracking ${upstream}: behind ↓ / ahead ↑`,
  filterCommits: () => "filter commits",
  // Diff panel (prd_02 task 10)
  diffWhitespace: () => "Whitespace",
  wsNone: () => "Do not ignore",
  wsTrailing: () => "Trailing",
  wsAll: () => "All",
  diffHighlight: () => "Highlight",
  hlWords: () => "Words",
  hlLines: () => "Lines",
  hlNone: () => "None",
  diffCount: (n: number) => `${n} ${n === 1 ? "difference" : "differences"}`,
  diffPrevTip: () => "Previous difference (Cmd/Ctrl+Up)",
  diffNextTip: () => "Next difference (Cmd/Ctrl+Down)",
  diffAtLast: () => "That was the last difference.",
  diffAtFirst: () => "That was the first difference.",
  diffNoDifferences: () => "No differences here.",
  foldedLines: (n: number) => `${n} unchanged lines hidden — click to show`,
  binarySizes: (a: string, b: string) => `Binary file, ${a} → ${b}`,
  sizeUnknown: () => "absent",
  bytes: (n: number) => `${n} B`,
  sizeScaled: (v: number, unit: "B" | "KB" | "MB" | "GB" | "TB") => `${v} ${unit}`,
  lfsTitle: () => "Git LFS object:",
  lfsAdded: (size: string) => `added, ${size}`,
  lfsRemoved: (size: string) => `removed, ${size}`,
  lfsUnchanged: (size: string) => `unchanged, ${size}`,
  lfsReplaced: (was: string, now: string) => `was ${was} → now ${now}`,
  lfsWas: () => "was",
  lfsOid: (short: string) => `oid ${short}…`,
  lfsNow: () => "now",
  lfsDownloaded: () => "downloaded",
  lfsNotDownloaded: () => "not downloaded",
  lfsDownload: () => "Download",
  lfsDownloadTip: () => "git lfs pull --include=<this file>: fetch the content and put it in the working tree",
  lfsDownloading: () => "git lfs pull",
  lfsNoProgram: () => "git-lfs is not installed, so the content cannot be downloaded from here.",
  lfsNotCheckedOut: () =>
    "Only the checked-out version of a file marked filter=lfs can be downloaded from here.",
  lfsUnsafePath: () =>
    "This file name cannot be passed to git lfs --include literally; download it from a terminal.",
  editOffLfs: () => "A Git LFS pointer is not edited here",
  readOnlySide: () => "read-only",
  workingTreeSide: () => "Working tree",
  indexSide: () => "Index",
  headSide: () => "HEAD",
  noParentSide: () => "no parent",
  mergeFirstParentNote: () => "Merge commit: changes against the first parent.",
  bigDiffNote: (n: number) => `${n} diff lines — large enough to stall the window.`,
  showWholeDiff: () => "Show it whole",
  hunkWhitespaceTip: () =>
    "Unavailable while whitespace is ignored: such a patch does not apply.",
  hunkWide: () => "wider",
  hunkWideTip: () =>
    "The context was expanded, so this hunk covers a wider region than it did before — staging or reverting it acts on all of it.",
  // Log mode — table, graph and selection (task 08)
  colSubject: () => "Commit",
  colAuthor: () => "Author",
  colDate: () => "Date",
  colResizeTip: () => "Drag to resize, double-click to reset",
  todayAt: (t: string) => `today ${t}`,
  yesterdayAt: (t: string) => `yesterday ${t}`,
  orderTip: () => "Commit order",
  orderDate: () => "Date order",
  orderTopo: () => "Topological order",
  highlightHeader: () => "Emphasis",
  highlightMine: () => "Dim commits that are neither mine nor on this branch",
  highlightHint: () => "Nothing leaves the list. To show only some people, use the User filter.",
  graphColorHeader: () => "Graph colours",
  graphColorBranch: () => "By branch",
  graphColorAge: () => "By commit age",
  graphAgeLegendTip: () =>
    "Each commit's node and date by the age of its author date: under a day · a week · a month · a year · older",
  ageStepLabel: (key: string): string =>
    ({ day: "< 1 day", week: "< 1 week", month: "< 1 month", year: "< 1 year" })[key] ?? "Older",
  loadingMore: () => "Loading more commits…",
  logEnd: () => "End of history",
  logCapReached: () => "The first 20 000 commits are shown — narrow the filter",
  logNoMatches: () => "Nothing matched the filters",
  newCommitsBtn: (n: number) => `+${n} new`,
  newCommitsTip: () => "Show the commits that arrived since the log was loaded",
  selectionLostNote: () =>
    "The selected commit is not in the reloaded pages — the newest one is selected",
  searchingNote: () => "Searching for a match…",
  noMoreMatches: () => "No more matches",
  searchCappedNote: () => "The search stopped at the row cap",
  refsMore: (n: number) => `+${n}`,
  selectedCommits: (n: number) => `${n} selected`,
  graphSuppressedTip: () =>
    "The filter breaks the history, so edges between commits are not drawn",
  offGraphLabel: () => "found by hash",
  offGraphTip: () =>
    "Fetched by hash from outside the loaded history: it has no place in the graph, so no edges are drawn around it",
  // ── Log filter bar (task 11) ───────────────────────────────────────────────
  fltSearchPlaceholder: () => "Search commits or hash",
  fltSearchTip: () =>
    "Highlights matches and moves between them. It does not narrow the list — the four filters do that.",
  fltRegexTip: () => "Regular expression",
  fltCaseTip: () => "Match case",
  fltRegexInvalid: (why: string) => `Invalid regular expression: ${why}`,
  fltPrevMatch: () => "Previous match (Cmd/Ctrl+Shift+G)",
  fltNextMatch: () => "Next match (Cmd/Ctrl+G) — loads further pages until it finds one",
  fltDimLabel: () => "Dim",
  fltDimTip: () => "Read rows that did not match the search muted",
  fltBranch: () => "Branch",
  fltAuthor: () => "User",
  fltDate: () => "Date",
  fltPaths: () => "Paths",
  fltHeadScope: () => "Current revision (HEAD)",
  fltClearAll: () => "Clear all",
  fltRemove: () => "Remove this filter",
  fltAuthorFilter: () => "filter authors",
  fltAuthorsEmpty: () => "No authors in this history",
  fltAuthorsLoading: () => "Reading authors…",
  fltAnyAuthor: () => "Any author",
  fltAuthorMe: (who: string) => `Me (${who})`,
  fltAuthorsChip: (n: number) => `${n} author${n === 1 ? "" : "s"}`,
  fltDateAny: () => "Any date",
  fltDateToday: () => "Today",
  fltDateWeek: () => "Last 7 days",
  fltDateMonth: () => "Last 30 days",
  fltDateYear: () => "Last 365 days",
  fltDateCustom: () => "Custom range",
  fltDateFrom: () => "From",
  fltDateTo: () => "To",
  fltDateApply: () => "Apply",
  fltDateTip: () =>
    "By committer date, the way git filters — not by the author date shown in the Date column",
  fltDateSince: (a: string) => `since ${a}`,
  fltDateUntil: (b: string) => `until ${b}`,
  fltDateRange: (a: string, b: string) => `${a} — ${b}`,
  fltPathPlaceholder: () => "path in the repository",
  fltPathAdd: () => "Add",
  fltPathChooseFile: () => "Choose files…",
  fltPathChooseDir: () => "Choose folder…",
  fltPathOutside: () => "Ignored: outside the repository",
  fltPathsChip: (n: number) => `${n} path${n === 1 ? "" : "s"}`,
  fltNoRepo: () => "No repository open",
  // Log mode — actions, menus, dialogs and operation state (task 12)
  menuCheckout: () => "Check out",
  menuCheckoutTracking: (name: string) => `Check out "${name}" as a local branch`,
  menuNewBranchHere: () => "New branch from here…",
  menuRenameBranch: () => "Rename…",
  menuDeleteBranch: () => "Delete branch…",
  menuDeleteRemoteBranch: () => "Delete on the remote…",
  menuMergeInto: (cur: string) => `Merge into "${cur}"`,
  menuRebaseOnto: (name: string) => `Rebase the current branch onto "${name}"`,
  menuPush: () => "Push",
  menuFetch: () => "Fetch",
  menuStashes: () => "Stashed changes…",
  menuUpdateBranch: (name: string) => `Update "${name}" from upstream`,
  menuCopyBranchName: () => "Copy branch name",
  menuCopyHash: (n: number) => (n === 1 ? "Copy hash" : `Copy hashes (${n})`),
  menuCompare: (n: number) => `Compare (${n})`,
  menuCompareWorktree: () => "Compare with the working tree",
  menuBranchFromCommit: () => "New branch from this commit…",
  menuTagCommit: () => "New tag on this commit…",
  menuCheckoutRevision: () => "Check out this revision…",
  menuRevertCommit: () => "Revert this commit",
  menuResetHere: () => "Reset the current branch to this commit…",
  menuCherryPick: () => "Cherry-pick onto the current branch",
  menuReword: () => "Edit the message…",
  menuSquash: (n: number) => (n < 2 ? "Squash selected commits…" : `Squash ${n} commits into one…`),
  menuRebaseFrom: () => "Interactive rebase from here…",
  whyRebaseBlocked: (why: string): string =>
    why === "merge"
      ? "a merge commit lies between this commit and HEAD — a rebase would flatten it"
      : why === "tooMany"
        ? "more than 1000 commits would be replayed"
        : "the commit is not on the current branch",
  whyRebaseUnknown: () => "could not read the commits to replay",
  whyDirtyTree: () => "tracked files have uncommitted changes — commit or stash them first",
  whyNeedTwoToSquash: () => "select two or more commits",
  whySquashRun: (why: string): string =>
    why === "merge"
      ? "a merge commit cannot be squashed"
      : "only commits that follow one another can be squashed",
  whySquashOffBranch: () =>
    "the selected commits are not one unbroken run on the current branch (a path filter can hide commits between them)",
  dlgMultilineHint: () => "Enter starts a new line; Cmd/Ctrl+Enter confirms.",
  dlgMessage: () => "Commit message",
  dlgRewordTitle: (hash: string) => `Edit the message of ${hash}`,
  dlgRewordNoteHead: () =>
    "Only the message changes: the commit gets a new hash, its changes stay as they are, and whatever is staged is left out of it.",
  dlgRewordNoteDeep: (after: number) =>
    `The commit and the ${after} ${after === 1 ? "commit" : "commits"} after it get new hashes; their changes are replayed unchanged (an interactive rebase).`,
  dlgRewordSubmit: () => "Change the message",
  dlgSquashTitle: (n: number) => `Squash ${n} commits into one`,
  dlgSquashNote: (after: number) =>
    after > 0
      ? `The selected commits melt into one new commit; the ${after} ${after === 1 ? "commit" : "commits"} after them are replayed unchanged.`
      : "The selected commits melt into one new commit.",
  dlgSquashMessage: () => "Message of the squashed commit",
  dlgSquashSubmit: () => "Squash",
  confirmRewritePublished: (n: number) =>
    `${n} of the commits to be rewritten ${n === 1 ? "is" : "are"} already on the upstream branch. After rewriting, the remote accepts the result only with a force push, and anyone who built on the old commits has to recover.\n\nRewrite anyway?`,
  rbTitle: (hash: string) => `Interactive rebase from ${hash}`,
  rbNote: (n: number) =>
    `Oldest first — the order git replays them in. ${n} ${n === 1 ? "commit is" : "commits are"} rewritten and get new hashes.`,
  rbAction: (a: string) =>
    (
      ({
        pick: "pick — keep",
        reword: "reword — new message",
        edit: "edit — stop to amend",
        squash: "squash — meld, keep message",
        fixup: "fixup — meld, drop message",
        drop: "drop — remove",
      }) as Record<string, string>
    )[a] ?? a,
  rbActionFor: (hash: string) => `Action for ${hash}`,
  rbMoveUp: () => "Move earlier (Alt+↑)",
  rbMoveDown: () => "Move later (Alt+↓)",
  rbMessageReword: () => "New message",
  rbMessageCombined: () => "Message of the combined commit (left as is, git joins the messages itself)",
  rbPreviewTitle: () => "Result, oldest first",
  rbPreviewStops: () => "stops here",
  rbPreviewReworded: () => "new message",
  rbSummary: (kept: number, melded: number, dropped: number) =>
    `${kept} kept · ${melded} melded · ${dropped} dropped`,
  rbProblem: (p: string): string =>
    p === "noneKept"
      ? "The plan has to keep at least one commit."
      : p === "firstMelds"
        ? "The oldest kept commit has nothing before it to be melded into."
        : "A message cannot be empty.",
  rbStart: () => "Start rebase",
  rbKeysHint: () =>
    "↑↓ row · P R E S F D action · Alt+↑↓ move · Cmd/Ctrl+Enter start · Esc cancel",
  opEditStop: (hash: string) =>
    `Stopped to edit ${hash}: change the files and commit them with "Amend last commit" in the Changes panel, then Continue.`,
  phaseReword: () => "reword",
  phaseSquash: () => "squash",
  phaseRebaseInteractive: () => "interactive rebase",
  whyCurrentBranch: () => "this is the current branch",
  whyRemoteBranch: () => "this is a remote branch",
  whyAlreadyContained: () => "the commit is already on the current branch",
  whyOperationRunning: () => "an unfinished operation is in progress — finish it first",
  whyMergeHasNoSkip: () => "a merge has no step to skip",
  whyNeedTwoCommits: () => "exactly two commits are needed",
  whyOneCommitOnly: () => "several commits are selected",
  whyChecking: () => "checking…",
  whyNoUpstreamUpdate: () => "the branch tracks no upstream to update from",
  whyDetachedHead: () => "HEAD is detached, there is no current branch",
  whyNoLocalBranch: () => "check out a branch first",
  dlgNewBranchTitle: (from: string) => `New branch from ${from}`,
  dlgBranchName: () => "Branch name",
  dlgCheckoutNew: () => "Switch to the new branch",
  dlgNewBranchNote: (previous: string) =>
    `The branch is created with "checkout -b", so it becomes the current one either way. With the box unticked the panel switches back to "${previous}" straight after — on a dirty working tree that switch back can fail, and the error then appears after a creation that succeeded.`,
  dlgRenameBranchTitle: (name: string) => `Rename "${name}"`,
  dlgNewName: () => "New name",
  dlgTagTitle: (hash: string) => `New tag on ${hash}`,
  dlgTagName: () => "Tag name",
  dlgTagMessage: () => "Message — leave empty for a lightweight tag",
  dlgCreate: () => "Create",
  dlgRename: () => "Rename",
  confirmDeleteBranch: (name: string) => `Delete branch "${name}"?`,
  confirmDeleteUnmerged: (name: string, n: number) =>
    `Branch "${name}" has ${n} ${n === 1 ? "commit" : "commits"} that no other branch contains. Deleting it loses ${n === 1 ? "it" : "them"}.\n\nDelete anyway?`,
  confirmDeleteRemote: (name: string) =>
    `Delete "${name}" on the remote? Everyone who fetches from it loses the branch.`,
  pushRejected: (branch: string, remote: string, why: string) =>
    `Pushing "${branch}" to ${remote} was refused:\n\n${why}\n\nForcing overwrites what is on the remote; commits pushed there by other people may be lost.`,
  pushForceLease: () => "Force push with lease — refuse if the remote moved since your last fetch",
  pushForceHard: () => "Force push — overwrite the remote whatever is on it",
  confirmCheckoutRevision: (hash: string) =>
    `Check out ${hash}. HEAD becomes detached: commits made here belong to no branch until you create one.`,
  confirmAbortOperation: (kind: string) =>
    `Abort the ${kind} and return the repository to the state it was in before it started? The work done during the operation is lost.`,
  confirmHardReset: (branch: string, hash: string, commits: number, dirty: boolean) =>
    `Reset "${branch}" to ${hash} in hard mode.\n\nDiscards: ${commits} ${commits === 1 ? "commit" : "commits"} after the target${dirty ? ", and every uncommitted change in the working tree and the index" : " (the working tree has no uncommitted changes)"}.`,
  resetChooseTitle: (branch: string, hash: string) =>
    `Reset "${branch}" to ${hash}. What happens to the changes?`,
  resetSoft: () => "soft — keep everything, staged",
  resetMixed: () => "mixed — keep the changes, unstaged",
  resetHard: () => "hard — discard the changes and the commits",
  resetKeep: () => "keep — move the branch, keep local edits",
  opMergeTitle: () => "Merge in progress",
  opRebaseTitle: (cur: string, total: string) => `Rebase in progress: ${cur} of ${total}`,
  opRebaseTitlePlain: () => "Rebase in progress",
  opCherryPickTitle: () => "Cherry-pick in progress",
  opRevertTitle: () => "Revert in progress",
  opConflicts: (n: number) => `${n} conflicted ${n === 1 ? "file" : "files"}:`,
  opNoConflicts: () => "No conflicted files — the operation is waiting to be continued.",
  opContinue: () => "Continue",
  opSkip: () => "Skip",
  opAbort: () => "Abort",
  opBlocksActions: () => "Other actions stay disabled until this operation finishes.",
  phaseMerge: () => "merge",
  phaseRebase: () => "rebase",
  phaseCherryPick: () => "cherry-pick",
  phaseRevert: () => "revert",
  phaseReset: () => "reset",
  phaseCheckout: () => "checkout",
  phasePush: () => "push",
  phaseForcePush: () => "push --force-with-lease",
  phaseForcePushHard: () => "push --force",
  phaseDeleteBranch: () => "delete branch",
  phaseRenameBranch: () => "rename branch",
  phaseCreateBranch: () => "create branch",
  phaseTag: () => "tag",
  phaseStashApply: () => "stash apply",
  phaseStashPop: () => "stash pop",
  phaseStashDrop: () => "stash drop",
  phaseStashPush: () => "stash push",
  phaseBranchUpdate: () => "update branch",
  phaseOpContinue: () => "continue",
  phaseOpSkip: () => "skip",
  phaseOpAbort: () => "abort",
  // Bisect (task 10): the search for the commit that brought a bug in
  phaseBisect: () => "bisect",
  bisectTitle: () => "Searching for the commit that brought the bug in",
  bisectTesting: (short: string, subject: string, steps: number | null) =>
    `testing ${short} ${subject}` +
    (steps === null ? "" : `, ≈${steps} ${steps === 1 ? "step" : "steps"} left`),
  bisectWaitBoth: () =>
    "Mark a commit with the bug and one without it: right-click a commit in the log, or use the buttons for the checked-out one.",
  bisectWaitGood: () => "Now mark a commit where the bug is absent — right-click it in the log.",
  bisectWaitBad: () => "Now mark a commit where the bug is present — right-click it in the log.",
  bisectFound: (term: string | null, short: string, subject: string) =>
    `${term === null ? "First bad commit" : `First "${term}" commit`}: ${short} ${subject}`,
  bisectCandidates: (n: number) =>
    `Only skipped commits are left: the first bad commit is one of these ${n}.`,
  bisectBroken: (problem: string) =>
    `The state of the bisect cannot be read, so only finishing it is possible. ${problem}`,
  bisectBtnBad: () => "Bug present",
  bisectBtnGood: () => "Bug absent",
  bisectBtnSkip: () => "Cannot test · Skip",
  bisectBtnTerm: (term: string) => `Mark "${term}"`,
  bisectBtnFinish: () => "Finish",
  bisectMarkTip: (what: string, short: string) => `${what}: ${short}`,
  bisectFinishTip: (where: string) => `git bisect reset — back to ${where}`,
  bisectShowInLog: () => "Show in log",
  bisectReturnCommit: (short: string) => `commit ${short}`,
  bisectReturnUnknown: () => "where it started",
  confirmBisectFinish: (where: string) =>
    `Finish the search and return to ${where}? The answers given so far are discarded.`,
  confirmBisectStart: (bad: string, good: string | null) =>
    good === null
      ? `Start searching for the commit that brought the bug in, with ${bad} marked "bug present".\n\nNext, mark a commit where the bug was still absent: right-click it in the log. Git then checks out commits one by one for you to test.`
      : `Start searching for the commit that brought the bug in between ${good} (bug absent) and ${bad} (bug present)?\n\nGit checks out commits one by one for you to test; the strip above the log takes your answers.`,
  menuBisectStartBad: () => "Find the bug: it is present here…",
  menuBisectBetween: () => "Find the bug between these two…",
  menuBisectMark: (role: "bad" | "good" | "skip", term: string | null) =>
    role === "skip"
      ? "Bisect: cannot test · skip"
      : term !== null
        ? `Bisect: mark "${term}"`
        : role === "bad"
          ? "Bisect: bug present here"
          : "Bisect: bug absent here",
  whyBisectNeedsTwo: () => "select exactly two commits: the newer one with the bug, the older one without",
  whyBisectRunning: () => "a bisect is in progress — finish it first",
  whyBisectBroken: () => "the state of the bisect cannot be read — only Finish is possible",
  whyBisectNoCommit: () => "no commit is checked out to test",
  bisectChip: (kind: string, bad: string | null, good: string | null): string => {
    switch (kind) {
      case "culprit":
        return bad === null ? "first bad" : `first ${bad}`;
      case "bad":
        return bad ?? "bad";
      case "good":
        return good ?? "good";
      case "skip":
        return "skipped";
      case "testing":
        return "testing";
      default:
        return "candidate";
    }
  },
  bisectChipTip: (kind: string): string => {
    switch (kind) {
      case "culprit":
        return "Bisect: the first bad commit — the search ended here";
      case "bad":
        return "Bisect: the bad end of the range — the bug is present here";
      case "good":
        return "Bisect: marked — the bug is absent here";
      case "skip":
        return "Bisect: skipped — this commit could not be tested";
      case "testing":
        return "Bisect: checked out for the current test";
      default:
        return "Bisect: only skipped commits were left — the first bad commit may be this one";
    }
  },
  // Stash manager
  stashesTitle: () => "Stashed changes",
  stashesTip: () => "Stashed changes",
  stashesEmpty: () => "This repository has no stashes.",
  stashesBanner: (n: number) => `${n} stashed ${n === 1 ? "change set" : "change sets"} — open`,
  stashSelectOne: () => "Select a stash",
  stashNoBranch: () => "no branch",
  stashNoMessage: () => "(no message)",
  stashFromApp: () => "by Graft",
  stashFromAppTip: () => "Stashed by the application while switching branches",
  stashFilesTitle: () => "Files in the stash",
  stashFilesEmpty: () => "No tracked file changed in this stash.",
  stashFilesNote: () => "Untracked files stashed along are not listed here — git keeps them apart.",
  // Backups of rolled-back work (refs/graft/discard)
  dismiss: () => "Dismiss",
  discardedFiles: (n: number) => `Rolled back ${n} ${n === 1 ? "file" : "files"}`,
  discardedList: (n: number) => `Rolled back a changelist: ${n} ${n === 1 ? "file" : "files"}`,
  discardedHunk: (path: string) => `Reverted a hunk in ${path}`,
  discardedLines: (path: string) => `Reverted lines in ${path}`,
  discardedRestore: (n: number) => `Restored ${n} ${n === 1 ? "file" : "files"} from a backup`,
  discardUndo: () => "Undo",
  discardStaleConfirm: (paths: string[]) =>
    `These files changed after the rollback:\n\n${paths.join("\n")}\n\nRestore anyway? Their current versions are backed up first, so this can be undone too.`,
  phaseDiscardRestore: () => "restore",
  restoreDiscardedMenu: () => "Restore discarded…",
  restoreDiscardedNoRepo: () => "Open a repository first",
  // Remotes (RemotesPanel) and clone (CloneDialog)
  remotesMenu: () => "Remotes…",
  cloneMenu: () => "Clone…",
  remotesTitle: () => "Remotes",
  remotesEmpty: () => "This repository has no remotes. Add one to push and fetch.",
  remoteFetch: () => "fetch",
  remotePush: () => "push",
  remotePushSame: () => "same as fetch",
  remoteNoUrl: () => "no address",
  remoteBranches: (n: number) => `${n} remote ${n === 1 ? "branch" : "branches"}`,
  remoteHasCredentials: () =>
    "This address has a password or token in it (hidden here). Replace it with an address without one and let a credential helper keep the secret.",
  remoteSelectOne: () => "Select a remote",
  remoteAddBtn: () => "Add…",
  remoteRenameBtn: () => "Rename…",
  remoteUrlBtn: () => "Change address…",
  remotePushUrlBtn: () => "Push address…",
  remoteRemoveBtn: () => "Remove…",
  remoteFormAdd: () => "Add a remote",
  remoteFormRename: (name: string) => `Rename remote "${name}"`,
  remoteFormUrl: (name: string) => `Address of "${name}"`,
  remoteFormPushUrl: (name: string) => `Push address of "${name}"`,
  remoteNameLabel: () => "Name",
  remoteUrlLabel: () => "Address",
  remoteUrlPlaceholder: () => "https://host/org/repo.git or git@host:org/repo.git",
  remotePushUrlNote: () => "Leave empty to push to the fetch address.",
  remoteUrlNote: () =>
    "https, ssh, git, file or a local path. No password or token in the address — a credential helper keeps those.",
  remoteSave: () => "Save",
  remoteHttpWarn: () => "http:// is not encrypted: what is sent can be read on the way.",
  remoteLoginNote: (user: string) =>
    `Signs in as "${user}": git asks for the password through the credential helper. Keep the password out of the address.`,
  remotesKeys: () => "↑↓ select · Enter change address · Delete remove · Esc close",
  confirmRemoteRemove: (name: string, n: number) =>
    `Remove remote "${name}"?\n\n` +
    (n > 0
      ? `Its ${n} remote-tracking ${n === 1 ? "branch is" : "branches are"} deleted with it, and branches tracking it lose their upstream.`
      : "Branches tracking it lose their upstream.") +
    " The branches on the server are not touched.",
  phaseRemoteAdd: () => "remote add",
  phaseRemoteRename: () => "remote rename",
  phaseRemoteRemove: () => "remote remove",
  phaseRemoteSetUrl: () => "remote set-url",
  cloneTitle: () => "Clone a repository",
  cloneUrlLabel: () => "Repository address",
  cloneParentLabel: () => "Into folder",
  cloneParentNone: () => "not chosen",
  cloneChooseParent: () => "Choose…",
  cloneParentDialog: () => "Folder to clone into",
  cloneNameLabel: () => "New folder name",
  cloneDest: (path: string) => `The repository goes to ${path}`,
  cloneNeedParent: () => "Choose the folder to clone into",
  cloneNeedName: () => "Enter a name for the new folder",
  cloneStart: () => "Clone",
  cloneStop: () => "Stop",
  cloneRunning: () => "Cloning…",
  cloneCancelled: () => "Stopped. The half-made folder was removed.",
  cloneKeys: () => "Enter clone · Esc close (stops a running clone)",
  discardsTitle: () => "Discarded changes",
  discardsEmpty: () => "Nothing has been rolled back in this repository yet.",
  discardFilesTitle: () => "Files in the backup",
  discardSelectOne: () => "Select a backup",
  discardNote: () =>
    "Only the working tree is restored: what was staged is not staged again. Every restore is backed up as well.",
  discardKeysHint: () => "↑↓ select · Enter restore · Esc close",
  discardRestoreBtn: () => "Restore",
  discardRestoreTip: () => "Put these files back as they were before",
  stashApplyBtn: () => "Apply",
  stashApplyTip: () => "Put the changes back and keep the stash",
  stashPopBtn: () => "Apply and drop",
  stashPopTip: () => "Put the changes back and remove the stash",
  stashDropBtn: () => "Drop…",
  stashDropTip: () => "Discard the stash without applying it",
  stashPushBtn: () => "Stash current changes…",
  stashPushTitle: () => "Message for the stash (optional)",
  confirmStashDrop: (label: string) =>
    `Drop the stash "${label}"?\n\nIts changes are discarded and nothing in the application can bring them back.`,
  copyFailed: () => "Could not copy to the clipboard.",
  compareSides: (a: string, b: string) => `${a} → ${b}`,
  contextMenuTip: () => "Actions (Cmd/Ctrl+Enter)",
  compareFilesTitle: () => "Changed between the two revisions",
  openCurrentVersion: () => "Open the current version",
  openCurrentVersionTip: () =>
    "Show this file in the Changes panel — only while it has uncommitted changes",
  openCurrentUnchanged: () => "The file has no uncommitted changes.",
  expandGap: (n: number) => `Show ${n} hidden lines`,
  expandGapTip: () => "Ask git for the patch with more context around the changes",
  gapTooLarge: (n: number, max: number) =>
    `${n} hidden lines is past the ${max} this panel expands: git would widen every hunk of the file at once. Open the file itself to read that region.`,
  expandTooLarge: (n: number, max: number) =>
    `With that region revealed the patch comes back at ${n} lines, past the ${max} this panel draws: git widens every hunk of the file at once. The diff is left as it was. Open the file itself to read that region.`,
  // Editing the working-tree side. The ceiling named below mirrors
  // `EDIT_SIZE_CEILING` in `engine/cli.rs` (2 MiB).
  editToggle: () => "Edit",
  editToggleTip: () => "Edit this file in the right column",
  editOffUnified: () => "Editing is offered in the side-by-side view.",
  editOffReadOnly: () =>
    "The right side is a revision, not the working tree, so it cannot be edited.",
  editOffLoading: () => "Reading the file…",
  editOffBinary: () => "This file is not text.",
  editOffTooLarge: () => "The file is larger than 2 MiB, the ceiling for editing here.",
  editOffMixedEol: () =>
    "The file mixes CRLF and LF line endings; editing it here would rewrite every line of it.",
  editOffMissing: () => "The file is no longer on disk.",
  editUnsaved: () => "unsaved",
  editUnsavedTip: () => "What was typed has not reached the file yet.",
  editSaveTip: () => "Cmd/Ctrl+S writes the file and recomputes the comparison",
  editStaleAsk: () =>
    "The file changed on disk after it was opened here. Reread it from disk, or overwrite it with the text you typed?",
  editStaleReread: () => "Reread from disk (the typed text is lost)",
  editStaleOverwrite: () => "Overwrite with the typed text",
  hunkEditTip: () =>
    "The comparison on screen was not computed from the file as it is now. Finish editing to stage or revert.",
  // Choosing lines to stage / unstage / revert
  linePickTip: () => "Click to choose this line, Shift+click to choose the range up to it",
  linesChosen: (n: number) => `${n} ${n === 1 ? "line" : "lines"} chosen`,
  stageLines: () => "Stage lines",
  unstageLines: () => "Unstage lines",
  revertLines: () => "Revert lines",
  clearLines: () => "Clear",
  stageLinesTip: () => "Put the chosen lines into the index (Cmd/Ctrl+Shift+S)",
  unstageLinesTip: () => "Take the chosen lines out of the index (Cmd/Ctrl+Shift+U)",
  revertLinesTip: () =>
    "Undo the chosen lines in the working tree; the file is backed up first (Cmd/Ctrl+Shift+Backspace)",
  revertLinesConfirm: (n: number) =>
    `Revert ${n} chosen ${n === 1 ? "line" : "lines"} in the working tree? The file is backed up first and can be restored.`,
  linesKeysWorktree: () =>
    "Cmd/Ctrl+Shift+J / K: extend · Cmd/Ctrl+Shift+S: stage · Cmd/Ctrl+Shift+Backspace: revert",
  linesKeysIndex: () => "Cmd/Ctrl+Shift+J / K: extend · Cmd/Ctrl+Shift+U: unstage",
  fileHistoryItem: () => "File history",
  fileHistoryTip: () => "Every commit that touched this file, renames followed",
  fileHistoryPickFile: () => "Select a file first",
  fileHistoryTitle: (path: string) => `History of ${path}`,
  fileHistoryFrom: (rev: string) => `from ${rev}`,
  fileHistoryLoading: () => "Reading the history…",
  fileHistoryLoadingMore: () => "Loading more…",
  fileHistoryEmpty: () => "No commit has touched this file yet",
  fileHistoryEmptyHint: () =>
    "An untracked or newly added file has no history until it is committed.",
  fileHistoryCount: (n: number, more: boolean) =>
    `${n}${more ? "+" : ""} ${n === 1 && !more ? "commit" : "commits"}`,
  fileHistoryMergesNote: () =>
    "Renames are followed. Merge commits are not listed, as in git log --follow: their changes appear on the commits they merged.",
  fileHistoryRenamedFrom: (old: string) => `renamed from ${old}`,
  fileHistoryKeys: () =>
    "↑ ↓ Home End: move · Enter: show in the log · ⌘/Ctrl+B: blame · Esc: close",
  fileHistoryShowInLog: () => "Show in log",
  commitNotInLog: (hash: string) =>
    `Commit ${hash} could not be found in the log — it may be outside every ref the log walks.`,

  // blame (R05b)
  blameItem: () => "Blame",
  blameTitle: (path: string) => `Blame of ${path}`,
  blameAt: (rev: string) => `at ${rev}`,
  blameWorkingTree: () => "working tree",
  blameLoading: () => "Running blame…",
  blameLines: (n: number) => `${n} ${n === 1 ? "line" : "lines"}`,
  blameEmptyFile: () => "The file is empty in this version.",
  blameBlocked: (kind: "binary" | "too-large" | "missing" | "untracked") =>
    ({
      binary: "A binary file has no lines to blame.",
      "too-large": "The file is too large to blame (over 4 MB or 50,000 lines).",
      missing: "There is no such file in this version.",
      untracked: "The file is not tracked by git yet — it has no history to blame.",
    })[kind],
  blameDeletedReason: () => "The file is deleted in this version — there is nothing to blame",
  blameUncommitted: () => "Not committed",
  blameUncommittedNote: () =>
    "Not committed yet. The diff below is the working tree against the version before.",
  blameSelectLine: () => "Select a line to see the commit that last changed it.",
  blameOpenInLog: () => "Open commit in log",
  blameOpenInLogUncommitted: () => "The line is not committed yet — it has no commit",
  blameBefore: () => "Blame before this change",
  blameBeforeBoundary: () =>
    "This is the earliest version reachable here (a root commit or the edge of a shallow clone)",
  blameBeforeCreated: () => "The file was created in this commit — the line had no earlier version",
  blameBeforeNew: () => "The file is not committed yet — there is no earlier version",
  blameBack: (n: number) => `Back (${n})`,
  blameBackTip: () => "Back to the previous blame (Esc)",
  blamePathInCommit: (path: string) => `In this commit the file was ${path}`,
  blameLandedExact: (line: number) => `The line was at ${line} in this version.`,
  blameLanded: (from: number, to: number) =>
    `The line was introduced by the change. Highlighted: what it replaced, or the line it was inserted after (${from === to ? from : `${from}–${to}`}).`,
  blameKeys: () =>
    "↑ ↓ PgUp PgDn Home End: move · Enter: open in log · ⌘/Ctrl+B: blame before · ⌘/Ctrl+↑↓: next/prev difference · Esc: back / close",
  conflictResolveItem: () => "Resolve conflict…",
  conflictResolveBtn: () => "Resolve…",
  conflictResolveTip: (path: string) => `Resolve the conflict in ${path}`,
  conflictTitle: (path: string) => `Resolve conflict: ${path}`,
  conflictKind: (k: ConflictKindKey) =>
    ({
      bothModified: "both sides changed the file",
      bothAdded: "both sides added the file",
      deletedByUs: "deleted on our side, changed on theirs",
      deletedByThem: "changed on our side, deleted on theirs",
      addedByUs: "added on our side only",
      addedByThem: "added on their side only",
      bothDeleted: "deleted on both sides",
    })[k],
  conflictLoading: () => "Reading the conflict…",
  conflictOurs: () => "Ours",
  conflictBase: () => "Base",
  conflictTheirs: () => "Theirs",
  conflictResult: () => "Result — what Save writes to the file",
  conflictLeft: (n: number) =>
    n === 0 ? "no conflicts left" : `${n} ${n === 1 ? "conflict" : "conflicts"} left`,
  conflictBlock: (i: number, n: number) => `Conflict ${i} of ${n}`,
  conflictBlockOpen: () => "unresolved",
  conflictBlockTaken: (how: string) => `resolved: ${how}`,
  conflictHowOurs: () => "ours",
  conflictHowTheirs: () => "theirs",
  conflictHowBothOT: () => "ours → theirs",
  conflictHowBothTO: () => "theirs → ours",
  conflictHowLines: (n: number) => `${n} ${n === 1 ? "line" : "lines"} picked`,
  conflictHowManual: () => "edited by hand",
  conflictTakeOurs: () => "Take ours",
  conflictTakeTheirs: () => "Take theirs",
  conflictBothOT: () => "Both: ours → theirs",
  conflictBothTO: () => "Both: theirs → ours",
  conflictResetBlock: () => "Reset",
  conflictResetBlockTip: () => "Put the markers of this block back",
  conflictPickTip: () => "Click: add this line to the result (again: remove it)",
  conflictEmptySide: () => "(empty)",
  conflictPrev: () => "Previous",
  conflictNext: () => "Next",
  conflictNavTip: () => "Previous / next unresolved conflict (⇧F7 / F7)",
  conflictUndo: () => "Undo",
  conflictRedo: () => "Redo",
  conflictNothingToUndo: () => "Nothing to undo in this editor",
  conflictNothingToRedo: () => "Nothing to redo in this editor",
  conflictSave: () => "Save",
  conflictSaved: () => "Saved",
  conflictUnsaved: () => "Unsaved changes",
  conflictMarkResolved: () => "Mark resolved",
  conflictMarkResolvedTip: () => "Save the result and stage it (git add): the conflict is resolved",
  conflictMarkAsIs: () => "Mark resolved as it is on disk",
  conflictMarkAsIsTip: () => "Stage the file exactly as it lies in the working tree (git add)",
  conflictWholeOurs: () => "Whole file: ours",
  conflictWholeTheirs: () => "Whole file: theirs",
  conflictWholeTip: (side: string) => `Resolve the whole file with the ${side} version (git checkout --${side} + git add)`,
  conflictWholeDeletes: (side: string) => `Whole file: ${side} (delete it)`,
  conflictWholeDeletesTip: (side: string) =>
    `The file does not exist on the ${side} side: resolving with it deletes the file (git rm)`,
  conflictWholeDiscards: () => "The result has unsaved edits. Resolve the whole file with one side and lose them?",
  conflictDiscardEdits: () => "The result has unsaved edits. Close the editor and lose them?",
  conflictMarkersLeft: (lines: string) =>
    `Conflict markers are still in the result (line ${lines}). Mark the file resolved with them in it?`,
  conflictMarkAnyway: () => "Mark resolved anyway",
  conflictRebaseNote: () =>
    "During a rebase “ours” is the branch being rebased onto, and “theirs” is your commit being replayed.",
  conflictNoBase: () =>
    "The markers carry no base (merge.conflictStyle is merge). Set it to diff3 or zdiff3 to see the base here, or look at the whole files.",
  conflictViewBlocks: () => "Blocks",
  conflictViewFiles: () => "Whole files",
  conflictSideAbsent: () => "Not on this side: the file is deleted here.",
  conflictSymlink: () => "A symbolic link: resolved whole only.",
  conflictSubmodule: () => "A submodule: resolved whole only.",
  conflictWhyDeleted: () =>
    "One side deleted the file, so there are no markers to work through: keep the deletion or keep the file.",
  conflictWhyWhole: (why: string) => `Only whole-file resolution is possible here. ${why}`,
  conflictParse: (reason: string, line: number, size: number | undefined, expected: number) =>
    reason === "marker-size"
      ? `Line ${line}: conflict markers ${size} characters long, while this file expects ${expected}. The conflict-marker-size attribute was probably changed after the merge; edit the result by hand.`
      : `Line ${line}: ${
          {
            nested: "a conflict opens inside another one",
            unterminated: "this conflict is never closed",
            "no-separator": "the conflict closes before its ======= separator",
            "stray-base": "a base marker (|||||||) out of place",
            "stray-separator": "a second ======= separator in one conflict",
            "stray-closing": "a closing marker with no conflict open",
          }[reason] ?? reason
        }. The block tools are off until the markers read again; edit the result by hand.`,
  conflictResolvedNext: (path: string) => `Resolved. Next conflicted file: ${path}`,
  conflictOpenNext: () => "Open next",
  conflictAllResolved: (op: string) => `All conflicts are resolved. Continue the ${op}?`,
  conflictNoneLeft: () => "Resolved. No conflicted files are left.",
  conflictKeys: () =>
    "Click a line: add it to the result · F7 / ⇧F7: next / previous conflict · ⌘/Ctrl+Z, ⌘/Ctrl+⇧Z: undo / redo · ⌘/Ctrl+S: save · Esc: close",
  phaseConflictResolve: () => "mark resolved",
  phaseConflictTake: () => "resolve with one side",
};

type Dict = typeof en;

const ru: Dict = {
  uiCrashTitle: () => "Что-то пошло не так в UI",
  reloadState: () => "Перечитать состояние",
  reloadWindow: () => "Перезагрузить окно",
  themeTip: () => "Тема: auto → light → dark",
  refreshTip: () => "Обновить",
  langTip: () => "Язык: English / Русский",
  openRepoBtn: () => "Открыть…",
  openRepoTitle: () => "Открыть репозиторий",
  recentProjects: () => "Недавние проекты",
  noRepository: () => "Нет репозитория",
  switchDirty: (target) =>
    `Есть незакоммиченные изменения. Переключиться на "${target}"?`,
  stashAndSwitch: () => "Спрятать в stash и переключиться",
  switchAsIs: () => "Переключиться как есть",
  cancel: () => "Отмена",
  newBranchFromHead: () => "Новая ветка от HEAD",
  filterBranches: () => "фильтр веток",
  newBranchItem: () => "Новая ветка…",
  local: () => "Локальные",
  remote: () => "Удалённые",
  recentBranches: () => "Недавние",
  branchMenuOptionsTip: () => "Как показывать список",
  optGroupByPrefix: () => "Группировать по префиксу",
  optShowRemote: () => "Показывать удалённые ветки",
  optShowRecent: () => "Показывать недавние ветки",
  confirm: () => "Подтвердить",
  commitColon: () => "Коммит:",
  selectedCount: (n) => ` · выбрано ${n}`,
  filesCount: (n) => `${n} ${ruPlural(n, "файл", "файла", "файлов")}`,
  commitMessage: () => "Сообщение коммита",
  amendLast: () => "Изменить последний коммит",
  commitAndPushTip: () => "Коммит и Push",
  commitBtn: () => "Коммит",
  commitPushBtn: () => "+ Push",
  untrackedSelectTip: () => "Untracked-файлы коммитятся выбором",
  coAuthorsColon: () => "Соавторы:",
  coAuthorAdd: () => "Добавить из истории…",
  coAuthorRemove: (name: string) => `Убрать соавтора ${name}`,
  coAuthorsLoading: () => "Читаю авторов этой истории…",
  coAuthorsNone: () => "Кроме вас, в этой истории никто не коммитил.",
  coAuthorsNoMatch: () => "Среди авторов истории совпадений нет.",
  coAuthorCommits: (n: number) => `${n} ${ruPlural(n, "коммит", "коммита", "коммитов")}`,
  coAuthorsPreview: () => "Допишется в конец сообщения:",
  changes: () => "Изменения",
  newListBtn: () => "+ список",
  newChangelist: () => "Новый changelist",
  cleanTree: () => "Нет изменений — рабочее дерево чистое.",
  active: () => "активный",
  rollbackTip: () => "Откатить отмеченные к HEAD",
  rollbackConfirm: (n) =>
    `Откатить отмеченные файлы (${n}) к HEAD? Файлы сначала сохраняются в копию, их можно будет вернуть; содержимое индекса — нет.`,
  collapseAll: () => "Свернуть всё",
  expandAll: () => "Развернуть всё",
  groupByDirTip: () => "Группировать по каталогам",
  showIgnoredTip: () => "Показывать игнорируемые",
  viewOptionsTip: () => "Параметры вида",
  groupByHeader: () => "Группировать по",
  directory: () => "Каталогам",
  showHeader: () => "Показывать",
  ignoredFiles: () => "Игнорируемые файлы",
  settings: () => "Настройки",
  appMenuTip: () => "Настройки, документация и о программе",
  docs: () => "Документация",
  about: () => "О программе",
  // Git-консоль
  gitConsole: () => "Git-консоль",
  gitConsoleTip: () => "Git-консоль",
  gitConsoleHint: () => "Выполняет git прямо здесь, без интерактива — без редактора и запроса учётных данных.",
  gitConsolePlaceholder: () => "git status",
  gitConsoleBadInput: (msg: string) => `Не удалось разобрать команду: ${msg}`,
  gitConsoleExit: (n: number) => `код завершения ${n}`,
  gitConsoleMine: () => "Мои",
  gitConsoleAll: () => "Все",
  gitConsoleMineTip: () => "Только git-команды, запущенные вашими действиями",
  gitConsoleAllTip: () => "Все git-команды, включая те, что Graft запускает сам, чтобы прочитать репозиторий",
  gitConsoleEmptyMine: () => "Вы ещё ничего не запускали. Введите git-команду и нажмите Enter — во вкладке «Все» видно и то, что Graft запускает сам.",
  gitConsoleEmptyAll: () => "Git-команд ещё не было.",
  gitConsoleNotStarted: () => "не запустилась",
  gitConsoleDuration: (ms: number) => `${ms} мс`,
  gitConsoleCwd: (path: string) => `в ${path}`,
  gitConsoleLoading: () => "Загрузка вывода…",
  gitConsoleEvicted: () =>
    "Эта запись уже вытеснена из журнала (он хранит ваши последние 1000 команд и последние 2000 фоновых чтений Graft).",
  gitConsoleNoOutput: () => "Вывода нет.",
  gitConsoleTruncated: (kb: number) => `Вывод обрезан на ${kb} КБ.`,
  showOutput: () => "Показать вывод",
  phaseGitExec: () => "git",
  themeLabel: () => "Тема",
  themeDesc: () => "Светлая, тёмная или как в системе",
  languageLabel: () => "Язык",
  languageDesc: () => "Язык интерфейса",
  themeAuto: () => "Авто",
  themeLight: () => "Светлая",
  themeDark: () => "Тёмная",
  close: () => "Закрыть",
  settingsAppearance: () => "Оформление",
  fontSizeLabel: () => "Размер шрифта",
  fontSizeDesc: () => "Размер основного текста; подписи и строки масштабируются вместе с ним",
  fontSizeValue: (px: number) => `${px} px`,
  aboutBlurb: () => "Нативный десктоп git-менеджер (Tauri + SolidJS).",
  sourceOnGithub: () => "Исходники на GitHub",
  checkForUpdates: () => "Проверить обновления",
  updChecking: () => "Проверяю\u2026",
  updUpToDate: () => "У вас последняя версия.",
  updFound: (version) => `Доступна версия ${version}.`,
  updUnreachable: () =>
    "Не удалось достучаться до сервера обновлений. Проверьте связь и попробуйте ещё раз.",
  updUpdateTo: (version) => `Обновить до v${version}`,
  updInstalling: () => "Устанавливаю\u2026",
  updInstallFailed: (message) => `Не удалось установить обновление: ${message}`,
  updLastChecked: (time) => `Последняя проверка в ${time}`,
  revertFileConfirm: (path) =>
    `Откатить ${path} к HEAD? Файлы сначала сохраняются в копию, их можно будет вернуть; содержимое индекса — нет.`,
  revertListConfirm: (name) =>
    `Откатить все файлы списка "${name}" к HEAD? Файлы сначала сохраняются в копию, их можно будет вернуть; содержимое индекса — нет.`,
  renameChangelist: () => "Переименовать changelist",
  deleteListConfirm: (name) =>
    `Удалить список "${name}"? Файлы вернутся в Default.`,
  moveTo: () => "Переместить в",
  revertToHead: () => "Откатить к HEAD",
  ignoreHeader: () => "Игнорировать",
  ignoreKind: (kind) =>
    kind === "file" ? "Этот файл" : kind === "extension" ? "Все файлы этого типа" : "Всю папку",
  makeActive: () => "Сделать активным",
  renameItem: () => "Переименовать…",
  deleteList: () => "Удалить список",
  revertListToHead: () => "Откатить список к HEAD",
  unstaged: () => "Не в индексе",
  staged: () => "В индексе",
  vsHead: () => "vs HEAD",
  selectFileHint: () => "Выберите файл слева, чтобы увидеть diff.",
  diffUnavailable: () => "diff недоступен для этого состояния",
  binaryFile: () => "Бинарный файл",
  noChangesForBase: () => "Нет изменений для этой базы.",
  revertHunkConfirm: () =>
    "Откатить этот hunk в рабочем дереве? Файл сначала сохраняется в копию, его можно будет вернуть.",
  revertHunkWideConfirm: () =>
    "Контекст расширен, поэтому hunk шире участка, который был виден до разворота. Откатить его целиком в рабочем дереве? Файл сначала сохраняется в копию, его можно будет вернуть.",
  changesCount: (n) => `${n} ${ruPlural(n, "изменение", "изменения", "изменений")}`,
  modeChanges: () => "Изменения",
  modeLog: () => "Лог",
  modeChangesTip: () => "Локальные изменения (Cmd/Ctrl+1)",
  modeLogTip: () => "История git (Cmd/Ctrl+2)",
  focusHint: () => "Tab / Shift+Tab переключают фокус между панелями",
  busyFetch: () => "Забираем изменения…",
  busyPull: () => "Подтягиваем изменения…",
  busyUndo: () => "Отменяем…",
  busyRedo: () => "Повторяем…",
  undoTip: (what) => `Отменить: ${what} (Cmd/Ctrl+Z)`,
  redoTip: (what) => `Повторить: ${what} (Cmd/Ctrl+Shift+Z)`,
  undoUnavailable: (why) => `Отменять нечего: ${why}`,
  redoUnavailable: (why) => `Повторять нечего: ${why}`,
  undoWhat: (action, detail) => {
    const x = detail ?? "";
    const q = detail ? ` «${detail}»` : "";
    switch (action) {
      case "commit_list":
        return `коммит${q}`;
      case "commit_reword":
        return `новое сообщение${q}`;
      case "commit_reset":
        return `reset на ${x}`;
      case "lines_stage":
        return `добавление в индекс: ${x}`;
      case "lines_unstage":
        return `исключение из индекса: ${x}`;
      case "branch_checkout":
      case "commit_checkout":
        return `переключение на ${x}`;
      case "branch_create":
        return `создание ветки ${x}`;
      case "branch_delete":
        return `удаление ветки ${x}`;
      case "branch_rename":
        return `переименование ${x}`;
      case "tag_create":
        return `создание тега ${x}`;
      case "branch_merge":
        return `слияние ${x}`;
      case "commit_cherry_pick":
        return `cherry-pick${q}`;
      case "commit_revert":
        return `revert${q}`;
      case "stash_push":
        return `stash${q}`;
      case "stash_pop":
        return `stash pop${q}`;
      case "stash_apply":
      case "stash_restore":
        return `stash apply${q}`;
      case "stash_drop":
        return `удаление stash${q}`;
      case "file_rollback":
      case "list_rollback":
        return `откат ${x}`;
      case "lines_revert":
        return `откат строк в ${x}`;
      case "discard_restore":
        return `восстановление ${x}`;
      case "file_ignore":
        return `правило игнора ${x}`;
      default:
        return `${action}${q}`;
    }
  },
  undoReason: (code, action) => {
    const name = action ?? "действие";
    switch (code) {
      case "empty":
        return "записанных действий пока нет";
      case "busy":
        return "выполняется действие";
      case "external":
        return "репозиторий изменился вне записанного действия (терминал, редактор файла) — история действий закончилась";
      case "operation":
        return "затронута незавершённая операция (merge, rebase, cherry-pick или revert)";
      case "failed":
        return `${name} завершилось ошибкой, успев изменить репозиторий`;
      case "unverifiable":
        return "не удалось прочитать репозиторий для проверки";
      case "concurrent":
        return "два действия шли одновременно";
      case "published":
        return `${name} опубликовало изменения на сервере — это не отменить`;
      case "fetched":
        return "fetch принёс новые теги";
      case "integrated":
        return `${name} принесло коммиты с сервера`;
      case "history":
        return `${name} переписало историю (rebase)`;
      case "console":
        return "команда в git-консоли изменила репозиторий";
      case "dirty":
        return `${name} выполнялось при незакоммиченных изменениях — отмена их бы потеряла`;
      case "worktree":
        return `${name} изменило файлы рабочего дерева (хук?)`;
      case "stash-position":
        return "применён или удалён не самый новый stash";
      case "stash-dirty":
        return "stash восстановлен поверх локальных изменений";
      case "no-earlier":
        return "раньше ничего нет";
      case "no-next":
        return "ничего не отменялось";
      case "inverse-failed":
        return "последняя отмена / повтор остановилась на полпути — проверьте репозиторий";
      case "bisect":
        return "затронут bisect (поиск коммита с ошибкой) — его переключения не откатываются по шагам";
      case "remotes":
        return "remote переименован или удалён — вместе с ним поменялись ветки, которые его отслеживали";
      default:
        return `${name} нельзя отменить`;
    }
  },
  undoConfirmHard: (redo, what, commits) =>
    `${redo ? "Повторить" : "Отменить"}: ${what}.\n\nБудет выполнен git reset --hard: отслеживаемые файлы перезапишутся версией другого коммита` +
    (commits > 0
      ? `, и ${commits} ${ruPlural(commits, "коммит уйдёт", "коммита уйдут", "коммитов уйдут")} из ветки (${redo ? "отмена" : "повтор"} вернёт их).`
      : ".") +
    " Неотслеживаемые файлы останутся как есть.",
  branchesTitle: () => "Ветки",
  logTitle: () => "Лог",
  commitDetailsTitle: () => "Детали коммита",
  diffTitle: () => "Diff",
  changedFiles: () => "Изменённые файлы",
  favorites: () => "Избранное",
  detachedHead: (hash) => (hash ? `HEAD (отделён на ${hash})` : "HEAD (отделён)"),
  onBranch: (name) => `HEAD → ${name}`,
  noCommitsTitle: () => "В репозитории пока нет коммитов",
  noCommitsHint: () => "Сделайте первый коммит в режиме «Изменения».",
  noRemoteBranches: () => "Нет удалённых веток",
  noBranchesYet: () => "Веток пока нет",
  loadingHistory: () => "Загружаем историю…",
  selectCommitHint: () => "Выберите коммит, чтобы увидеть детали.",
  loadingCommitDetails: () => "Загружаем детали коммита…",
  authorLabel: () => "Автор",
  sigLabel: () => "Подпись",
  sigChecking: () => "проверяется…",
  sigFailed: () => "не прочиталась",
  sigUnsigned: () => "без подписи",
  sigVerified: () => "проверено",
  sigUnknownKey: () => "неизвестный ключ",
  sigMissingKey: () => "не удалось проверить: нет открытого ключа",
  sigExpired: () => "просрочена",
  sigExpiredKey: () => "ключ просрочен",
  sigRevoked: () => "ключ отозван",
  sigBad: () => "плохая подпись",
  sigUnchecked: () => "не удалось проверить",
  sigFormatUnknown: () => "неизвестный формат",
  sigHintSshNotListed: () =>
    "Подпись верна, но файл допущенных подписантов (gpg.ssh.allowedSignersFile) не знает этого ключа у этого подписанта.",
  sigHintUntrusted: () => "Подпись верна, но ключу нет доверия в вашей связке ключей.",
  sigHintSshNoSignersFile: () =>
    "git проверяет SSH-подписи по файлу допущенных подписантов: задайте gpg.ssh.allowedSignersFile.",
  sigHintNoGpg: () => "gpg не установлен или не запустился — git не смог проверить подпись.",
  sigHintNoGpgsm: () => "gpgsm не установлен или не запустился — git не смог проверить подпись.",
  sigHintUnknownFormat: () => "У git нет программы для проверки подписи такого вида.",
  sigHintMissingKey: () => "Открытого ключа подписанта нет в вашей связке ключей.",
  forgeOpenCommit: (forge) => `Открыть на ${forge}`,
  forgeAuthorCommits: (forge) => `Коммиты автора на ${forge}`,
  forgeOpenFailed: (why) => `Не удалось открыть ссылку: ${why}`,
  committerLabel: () => "Коммиттер",
  inBranches: (n) => `В ${n} ${ruPlural(n, "ветке", "ветках", "ветках")}`,
  noContainingBranches: () => "Коммит не содержится ни в одной ветке",
  showAllBranches: (n) => `показать все ${n}`,
  showFewerBranches: () => "свернуть",
  branchesCapped: () =>
    "Проверены только первые 64 ветки — полный список может быть длиннее.",
  noChangedFiles: () => "Коммит не изменил ни одного файла",
  treeOnlyTip: () => "Только в режиме дерева",
  actionPending: () => "Пока недоступно — нужен бэкенд истории",
  expandAllTip: () => "Развернуть всё",
  collapseAllTip: () => "Свернуть всё",
  favoritesOnlyTip: () => "Показывать только избранные ветки",
  favoritesShowAllTip: () => "Показать все ветки снова",
  newBranchTip: () => "Новая ветка…",
  noMatches: () => "Ничего не найдено",
  branchesFailed: () => "Не удалось прочитать ветки",
  favoritesSection: () => "Избранное",
  favoriteAddTip: () => "В избранное - ветка встанет в начало списка (Cmd/Ctrl+D)",
  favoriteRemoveTip: () => "Убрать из избранного - ветка вернётся в список ниже (Cmd/Ctrl+D)",
  noFavorites: () => "Избранных веток нет",
  noFavoritesHint: () => "Пометьте ветку звездой, чтобы держать её в начале списка",
  fetchPruneTip: () => "Забрать с remote и обновить счётчики",
  fetching: () => "fetch",
  trackingTip: (upstream) => `Отслеживает ${upstream}: отстаёт ↓ / опережает ↑`,
  filterCommits: () => "фильтр коммитов",
  diffWhitespace: () => "Пробелы",
  wsNone: () => "Не игнорировать",
  wsTrailing: () => "Конечные",
  wsAll: () => "Все",
  diffHighlight: () => "Подсветка",
  hlWords: () => "Слова",
  hlLines: () => "Строки",
  hlNone: () => "Без подсветки",
  diffCount: (n) => `${n} ${ruPlural(n, "различие", "различия", "различий")}`,
  diffPrevTip: () => "Предыдущее различие (Cmd/Ctrl+Вверх)",
  diffNextTip: () => "Следующее различие (Cmd/Ctrl+Вниз)",
  diffAtLast: () => "Это было последнее различие.",
  diffAtFirst: () => "Это было первое различие.",
  diffNoDifferences: () => "Различий здесь нет.",
  foldedLines: (n) =>
    `скрыто ${n} ${ruPlural(n, "неизменённая строка", "неизменённые строки", "неизменённых строк")} — нажмите, чтобы показать`,
  binarySizes: (a, b) => `Бинарный файл, ${a} → ${b}`,
  sizeUnknown: () => "нет",
  bytes: (n) => `${n} Б`,
  sizeScaled: (v, unit) =>
    `${String(v).replace(".", ",")} ${{ B: "Б", KB: "КБ", MB: "МБ", GB: "ГБ", TB: "ТБ" }[unit]}`,
  lfsTitle: () => "Объект Git LFS:",
  lfsAdded: (size) => `добавлен, ${size}`,
  lfsRemoved: (size) => `удалён, ${size}`,
  lfsUnchanged: (size) => `не изменился, ${size}`,
  lfsReplaced: (was, now) => `было ${was} → стало ${now}`,
  lfsWas: () => "было",
  lfsOid: (short) => `oid ${short}…`,
  lfsNow: () => "стало",
  lfsDownloaded: () => "скачан",
  lfsNotDownloaded: () => "не скачан",
  lfsDownload: () => "Загрузить",
  lfsDownloadTip: () => "git lfs pull --include=<этот файл>: скачать содержимое и положить его в рабочее дерево",
  lfsDownloading: () => "git lfs pull",
  lfsNoProgram: () => "git-lfs не установлен — отсюда содержимое не скачать.",
  lfsNotCheckedOut: () =>
    "Отсюда скачивается только выгруженная версия файла с атрибутом filter=lfs.",
  lfsUnsafePath: () =>
    "Имя этого файла нельзя буквально передать в git lfs --include — скачайте его из терминала.",
  editOffLfs: () => "Указатель Git LFS здесь не правится",
  readOnlySide: () => "только чтение",
  workingTreeSide: () => "Рабочее дерево",
  indexSide: () => "Индекс",
  headSide: () => "HEAD",
  noParentSide: () => "без родителя",
  mergeFirstParentNote: () =>
    "Merge-коммит: изменения относительно первого родителя.",
  bigDiffNote: (n) => `${n} строк diff — столько может подвесить окно.`,
  showWholeDiff: () => "Показать целиком",
  hunkWhitespaceTip: () =>
    "Недоступно при игнорировании пробелов: такой патч не применяется.",
  hunkWide: () => "шире",
  hunkWideTip: () =>
    "Контекст расширен, поэтому hunk охватывает более широкий участок, чем до разворота — постановка и откат подействуют на весь этот участок.",
  colSubject: () => "Коммит",
  colAuthor: () => "Автор",
  colDate: () => "Дата",
  colResizeTip: () => "Тяните, чтобы изменить; двойной клик — сброс",
  todayAt: (t) => `сегодня ${t}`,
  yesterdayAt: (t) => `вчера ${t}`,
  orderTip: () => "Порядок коммитов",
  orderDate: () => "По дате",
  orderTopo: () => "Топологический",
  highlightHeader: () => "Подсветка",
  highlightMine: () => "Приглушать чужие коммиты и коммиты не из этой ветки",
  highlightHint: () => "Из списка ничего не исчезает. Чтобы оставить только нужных людей, есть фильтр User.",
  graphColorHeader: () => "Цвета графа",
  graphColorBranch: () => "По веткам",
  graphColorAge: () => "По возрасту коммита",
  graphAgeLegendTip: () =>
    "Узел и дата коммита — по возрасту даты автора: меньше суток · недели · месяца · года · старше",
  ageStepLabel: (key) =>
    ({ day: "< суток", week: "< недели", month: "< месяца", year: "< года" })[key] ?? "Старше",
  loadingMore: () => "Загружаем ещё коммиты…",
  logEnd: () => "Конец истории",
  logCapReached: () => "Показаны первые 20 000 коммитов — уточните фильтр",
  logNoMatches: () => "По фильтрам ничего не найдено",
  newCommitsBtn: (n) => `+${n} новых`,
  newCommitsTip: () => "Показать коммиты, появившиеся после загрузки лога",
  selectionLostNote: () =>
    "Выбранный коммит не попал в перезагруженные страницы — выбран самый новый",
  searchingNote: () => "Ищем совпадение…",
  noMoreMatches: () => "Больше совпадений нет",
  searchCappedNote: () => "Поиск остановился на потолке строк",
  refsMore: (n) => `+${n}`,
  selectedCommits: (n) => `выбрано ${n}`,
  graphSuppressedTip: () =>
    "Фильтр разрывает историю, поэтому связи между коммитами не рисуются",
  offGraphLabel: () => "найден по хэшу",
  offGraphTip: () =>
    "Достан по хэшу за пределами загруженной истории: места в графе у него нет, поэтому связи вокруг него не рисуются",
  // ── Строка фильтров лога (задача 11) ───────────────────────────────────────
  fltSearchPlaceholder: () => "Поиск по коммитам и хэшу",
  fltSearchTip: () =>
    "Подсвечивает совпадения и переходит по ним. Список не сужает — это делают четыре фильтра.",
  fltRegexTip: () => "Регулярное выражение",
  fltCaseTip: () => "Учитывать регистр",
  fltRegexInvalid: (why) => `Неверное регулярное выражение: ${why}`,
  fltPrevMatch: () => "Предыдущее совпадение (Cmd/Ctrl+Shift+G)",
  fltNextMatch: () => "Следующее совпадение (Cmd/Ctrl+G) — догружает страницы, пока не найдёт",
  fltDimLabel: () => "Приглушить",
  fltDimTip: () => "Показывать несовпавшие строки приглушённо",
  fltBranch: () => "Ветка",
  fltAuthor: () => "Автор",
  fltDate: () => "Дата",
  fltPaths: () => "Пути",
  fltHeadScope: () => "Текущая ревизия (HEAD)",
  fltClearAll: () => "Сбросить всё",
  fltRemove: () => "Снять этот фильтр",
  fltAuthorFilter: () => "фильтр авторов",
  fltAuthorsEmpty: () => "В этой истории нет авторов",
  fltAuthorsLoading: () => "Читаем авторов…",
  fltAnyAuthor: () => "Любой автор",
  fltAuthorMe: (who) => `Я (${who})`,
  fltAuthorsChip: (n) => `${n} ${n % 10 === 1 && n % 100 !== 11 ? "автор" : n % 10 >= 2 && n % 10 <= 4 && (n % 100 < 12 || n % 100 > 14) ? "автора" : "авторов"}`,
  fltDateAny: () => "Любая дата",
  fltDateToday: () => "Сегодня",
  fltDateWeek: () => "Последние 7 дней",
  fltDateMonth: () => "Последние 30 дней",
  fltDateYear: () => "Последние 365 дней",
  fltDateCustom: () => "Произвольный период",
  fltDateFrom: () => "С",
  fltDateTo: () => "По",
  fltDateApply: () => "Применить",
  fltDateTip: () =>
    "По дате коммиттера, как фильтрует сам git, — не по дате автора из колонки «Дата»",
  fltDateSince: (a) => `с ${a}`,
  fltDateUntil: (b) => `по ${b}`,
  fltDateRange: (a, b) => `${a} — ${b}`,
  fltPathPlaceholder: () => "путь в репозитории",
  fltPathAdd: () => "Добавить",
  fltPathChooseFile: () => "Выбрать файлы…",
  fltPathChooseDir: () => "Выбрать каталог…",
  fltPathOutside: () => "Пропущено: вне репозитория",
  fltPathsChip: (n) => `${n} ${n % 10 === 1 && n % 100 !== 11 ? "путь" : n % 10 >= 2 && n % 10 <= 4 && (n % 100 < 12 || n % 100 > 14) ? "пути" : "путей"}`,
  fltNoRepo: () => "Репозиторий не открыт",
  menuCheckout: () => "Переключиться",
  menuCheckoutTracking: (name) => `Переключиться на "${name}" локальной веткой`,
  menuNewBranchHere: () => "Создать ветку отсюда…",
  menuRenameBranch: () => "Переименовать…",
  menuDeleteBranch: () => "Удалить ветку…",
  menuDeleteRemoteBranch: () => "Удалить на сервере…",
  menuMergeInto: (cur) => `Влить в "${cur}"`,
  menuRebaseOnto: (name) => `Перебазировать текущую ветку на "${name}"`,
  menuPush: () => "Отправить (push)",
  menuFetch: () => "Получить с сервера (fetch)",
  menuStashes: () => "Спрятанные изменения…",
  menuUpdateBranch: (name: string) => `Обновить "${name}" из upstream`,
  menuCopyBranchName: () => "Копировать имя ветки",
  menuCopyHash: (n) => (n === 1 ? "Копировать хэш" : `Копировать хэши (${n})`),
  menuCompare: (n) => `Сравнить (${n})`,
  menuCompareWorktree: () => "Сравнить с рабочим деревом",
  menuBranchFromCommit: () => "Создать ветку от этого коммита…",
  menuTagCommit: () => "Поставить тег на этот коммит…",
  menuCheckoutRevision: () => "Перейти на эту ревизию…",
  menuRevertCommit: () => "Откатить коммит (revert)",
  menuResetHere: () => "Сбросить текущую ветку на этот коммит…",
  menuCherryPick: () => "Перенести коммит в текущую ветку",
  menuReword: () => "Изменить сообщение…",
  menuSquash: (n) =>
    n < 2 ? "Объединить выбранные коммиты…" : `Объединить ${n} ${ruPlural(n, "коммит", "коммита", "коммитов")} в один…`,
  menuRebaseFrom: () => "Интерактивный rebase отсюда…",
  whyRebaseBlocked: (why) =>
    why === "merge"
      ? "между этим коммитом и HEAD есть merge-коммит — rebase его развернёт"
      : why === "tooMany"
        ? "пришлось бы переписать больше 1000 коммитов"
        : "коммит не в текущей ветке",
  whyRebaseUnknown: () => "не удалось прочитать коммиты для переписывания",
  whyDirtyTree: () => "в отслеживаемых файлах есть незакоммиченные изменения — закоммитьте или спрячьте их",
  whyNeedTwoToSquash: () => "выберите два коммита или больше",
  whySquashRun: (why) =>
    why === "merge"
      ? "merge-коммит объединить нельзя"
      : "объединить можно только коммиты, идущие друг за другом",
  whySquashOffBranch: () =>
    "выбранные коммиты не идут подряд в текущей ветке (фильтр по пути может скрывать коммиты между ними)",
  dlgMultilineHint: () => "Enter — новая строка; Cmd/Ctrl+Enter — подтвердить.",
  dlgMessage: () => "Сообщение коммита",
  dlgRewordTitle: (hash) => `Сообщение коммита ${hash}`,
  dlgRewordNoteHead: () =>
    "Меняется только сообщение: коммит получит новый хэш, изменения в нём останутся прежними, подготовленное в индексе в него не попадёт.",
  dlgRewordNoteDeep: (after) =>
    `Коммит и ${after} ${ruPlural(after, "коммит", "коммита", "коммитов")} после него получат новые хэши; их изменения переиграются без правок (интерактивный rebase).`,
  dlgRewordSubmit: () => "Изменить сообщение",
  dlgSquashTitle: (n) => `Объединить ${n} ${ruPlural(n, "коммит", "коммита", "коммитов")} в один`,
  dlgSquashNote: (after) =>
    after > 0
      ? `Выбранные коммиты сольются в один новый; ${after} ${ruPlural(after, "коммит", "коммита", "коммитов")} после них переиграются без правок.`
      : "Выбранные коммиты сольются в один новый.",
  dlgSquashMessage: () => "Сообщение объединённого коммита",
  dlgSquashSubmit: () => "Объединить",
  confirmRewritePublished: (n) =>
    `${n} из переписываемых ${ruPlural(n, "коммита", "коммитов", "коммитов")} уже есть в upstream-ветке. После переписывания удалённый репозиторий примет результат только через force push, а тем, кто строил работу на старых коммитах, придётся её переносить.\n\nВсё равно переписать?`,
  rbTitle: (hash) => `Интерактивный rebase от ${hash}`,
  rbNote: (n) =>
    `Сверху самый старый — в этом порядке git их переиграет. ${n} ${ruPlural(n, "коммит будет переписан", "коммита будут переписаны", "коммитов будут переписаны")} и получат новые хэши.`,
  rbAction: (a) =>
    (
      ({
        pick: "pick — оставить",
        reword: "reword — новое сообщение",
        edit: "edit — остановиться для правки",
        squash: "squash — влить, сохранить сообщение",
        fixup: "fixup — влить, без сообщения",
        drop: "drop — удалить",
      }) as Record<string, string>
    )[a] ?? a,
  rbActionFor: (hash) => `Действие для ${hash}`,
  rbMoveUp: () => "Раньше (Alt+↑)",
  rbMoveDown: () => "Позже (Alt+↓)",
  rbMessageReword: () => "Новое сообщение",
  rbMessageCombined: () => "Сообщение объединённого коммита (если не менять, git склеит сообщения сам)",
  rbPreviewTitle: () => "Результат, сверху самый старый",
  rbPreviewStops: () => "остановка",
  rbPreviewReworded: () => "новое сообщение",
  rbSummary: (kept, melded, dropped) =>
    `оставлено ${kept} · влито ${melded} · удалено ${dropped}`,
  rbProblem: (p) =>
    p === "noneKept"
      ? "План должен оставить хотя бы один коммит."
      : p === "firstMelds"
        ? "Самому старому оставленному коммиту не во что вливаться."
        : "Сообщение не может быть пустым.",
  rbStart: () => "Начать rebase",
  rbKeysHint: () =>
    "↑↓ строка · P R E S F D действие · Alt+↑↓ перенос · Cmd/Ctrl+Enter начать · Esc отмена",
  opEditStop: (hash) =>
    `Остановка для правки ${hash}: поменяйте файлы и закоммитьте их с «Изменить последний коммит» в панели изменений, затем «Продолжить».`,
  phaseReword: () => "изменение сообщения",
  phaseSquash: () => "объединение коммитов",
  phaseRebaseInteractive: () => "интерактивный rebase",
  whyCurrentBranch: () => "это текущая ветка",
  whyRemoteBranch: () => "это удалённая ветка",
  whyAlreadyContained: () => "коммит уже содержится в текущей ветке",
  whyOperationRunning: () => "идёт незавершённая операция — сначала завершите её",
  whyMergeHasNoSkip: () => "у merge нет пропуска шага",
  whyNeedTwoCommits: () => "нужно ровно два коммита",
  whyOneCommitOnly: () => "выбрано несколько коммитов",
  whyChecking: () => "проверяем…",
  whyNoUpstreamUpdate: () => "у ветки нет upstream, неоткуда обновляться",
  whyDetachedHead: () => "HEAD отделён, текущей ветки нет",
  whyNoLocalBranch: () => "сначала перейдите на ветку",
  dlgNewBranchTitle: (from) => `Новая ветка от ${from}`,
  dlgBranchName: () => "Имя ветки",
  dlgCheckoutNew: () => "Перейти на созданную ветку",
  dlgNewBranchNote: (previous) =>
    `Ветка создаётся через "checkout -b", поэтому в любом случае становится текущей. Со снятой галкой панель сразу вернётся на "${previous}" — на грязном дереве этот возврат может не удаться, и тогда ошибка появится уже после удавшегося создания.`,
  dlgRenameBranchTitle: (name) => `Переименовать "${name}"`,
  dlgNewName: () => "Новое имя",
  dlgTagTitle: (hash) => `Тег на коммит ${hash}`,
  dlgTagName: () => "Имя тега",
  dlgTagMessage: () => "Сообщение — пусто означает лёгкий тег",
  dlgCreate: () => "Создать",
  dlgRename: () => "Переименовать",
  confirmDeleteBranch: (name) => `Удалить ветку "${name}"?`,
  confirmDeleteUnmerged: (name, n) =>
    `У ветки "${name}" ${n} ${ruPlural(n, "коммит", "коммита", "коммитов")}, которых нет ни в одной другой ветке. Удаление потеряет их.\n\nВсё равно удалить?`,
  confirmDeleteRemote: (name) =>
    `Удалить "${name}" на сервере? Ветка исчезнет у всех, кто оттуда получает.`,
  pushRejected: (branch, remote, why) =>
    `Отправка "${branch}" в ${remote} отклонена:\n\n${why}\n\nПринудительная отправка перезапишет то, что лежит на сервере; коммиты, отправленные туда другими, могут быть потеряны.`,
  pushForceLease: () => "Принудительно с проверкой - откажет, если сервер ушёл вперёд с вашего последнего fetch",
  pushForceHard: () => "Принудительно - перезаписать сервер, что бы на нём ни было",
  confirmCheckoutRevision: (hash) =>
    `Перейти на ${hash}. HEAD станет отделённым: коммиты, сделанные здесь, не будут принадлежать ни одной ветке, пока вы её не создадите.`,
  confirmAbortOperation: (kind) =>
    `Прервать ${kind} и вернуть репозиторий в состояние до начала операции? Работа, сделанная по ходу, будет потеряна.`,
  confirmHardReset: (branch, hash, commits, dirty) =>
    `Сброс "${branch}" на ${hash} в режиме hard.\n\nБудет потеряно: ${commits} ${ruPlural(commits, "коммит", "коммита", "коммитов")} после выбранного${dirty ? ", а также все незакоммиченные изменения в рабочем дереве и индексе" : " (незакоммиченных изменений нет)"}.`,
  resetChooseTitle: (branch, hash) =>
    `Сброс "${branch}" на ${hash}. Что сделать с изменениями?`,
  resetSoft: () => "soft — оставить всё, изменения в индексе",
  resetMixed: () => "mixed — оставить изменения вне индекса",
  resetHard: () => "hard — выбросить изменения и коммиты",
  resetKeep: () => "keep — сдвинуть ветку, сохранив правки",
  opMergeTitle: () => "Идёт merge",
  opRebaseTitle: (cur, total) => `Идёт rebase: ${cur} из ${total}`,
  opRebaseTitlePlain: () => "Идёт rebase",
  opCherryPickTitle: () => "Идёт cherry-pick",
  opRevertTitle: () => "Идёт revert",
  opConflicts: (n) =>
    `${n} ${ruPlural(n, "конфликтный файл", "конфликтных файла", "конфликтных файлов")}:`,
  opNoConflicts: () => "Конфликтных файлов нет — операция ждёт продолжения.",
  opContinue: () => "Продолжить",
  opSkip: () => "Пропустить",
  opAbort: () => "Прервать",
  opBlocksActions: () => "Остальные действия недоступны, пока операция не закончена.",
  phaseMerge: () => "merge",
  phaseRebase: () => "rebase",
  phaseCherryPick: () => "cherry-pick",
  phaseRevert: () => "revert",
  phaseReset: () => "reset",
  phaseCheckout: () => "checkout",
  phasePush: () => "push",
  phaseForcePush: () => "push --force-with-lease",
  phaseForcePushHard: () => "push --force",
  phaseDeleteBranch: () => "удаление ветки",
  phaseRenameBranch: () => "переименование ветки",
  phaseCreateBranch: () => "создание ветки",
  phaseTag: () => "тег",
  phaseStashApply: () => "stash apply",
  phaseStashPop: () => "stash pop",
  phaseStashDrop: () => "stash drop",
  phaseStashPush: () => "stash push",
  phaseBranchUpdate: () => "обновление ветки",
  phaseOpContinue: () => "continue",
  phaseOpSkip: () => "skip",
  phaseOpAbort: () => "abort",
  // Bisect (задача 10): поиск коммита, который принёс ошибку
  phaseBisect: () => "bisect",
  bisectTitle: () => "Поиск коммита с ошибкой",
  bisectTesting: (short, subject, steps) =>
    `проверяется ${short} ${subject}` +
    (steps === null ? "" : `, осталось ≈${steps} ${ruPlural(steps, "шаг", "шага", "шагов")}`),
  bisectWaitBoth: () =>
    "Отметьте коммит с ошибкой и коммит без неё: правый клик по коммиту в логе или кнопки для текущего.",
  bisectWaitGood: () => "Теперь отметьте коммит, где ошибки ещё нет, — правый клик по нему в логе.",
  bisectWaitBad: () => "Теперь отметьте коммит, где ошибка уже есть, — правый клик по нему в логе.",
  bisectFound: (term, short, subject) =>
    `${term === null ? "Первый плохой коммит" : `Первый коммит «${term}»`}: ${short} ${subject}`,
  bisectCandidates: (n) =>
    `Остались только пропущенные коммиты: первый плохой — один из этих ${n}.`,
  bisectBroken: (problem) =>
    `Состояние bisect не читается, поэтому его можно только закончить. ${problem}`,
  bisectBtnBad: () => "Ошибка есть",
  bisectBtnGood: () => "Ошибки нет",
  bisectBtnSkip: () => "Не проверить · Пропустить",
  bisectBtnTerm: (term) => `Отметить «${term}»`,
  bisectBtnFinish: () => "Закончить",
  bisectMarkTip: (what, short) => `${what}: ${short}`,
  bisectFinishTip: (where) => `git bisect reset — вернуться на ${where}`,
  bisectShowInLog: () => "Показать в логе",
  bisectReturnCommit: (short) => `коммит ${short}`,
  bisectReturnUnknown: () => "исходную позицию",
  confirmBisectFinish: (where) =>
    `Закончить поиск и вернуться на ${where}? Данные ответы будут сброшены.`,
  confirmBisectStart: (bad, good) =>
    good === null
      ? `Начать поиск коммита, который принёс ошибку: ${bad} отмечен как «ошибка есть».\n\nДальше отметьте коммит, где ошибки ещё не было, — правым кликом в логе. Затем git будет по одному выдавать коммиты на проверку.`
      : `Искать коммит, который принёс ошибку, между ${good} (ошибки нет) и ${bad} (ошибка есть)?\n\nGit будет по одному выдавать коммиты на проверку, ответы принимает полоса над логом.`,
  menuBisectStartBad: () => "Искать коммит с ошибкой: здесь ошибка есть…",
  menuBisectBetween: () => "Искать коммит с ошибкой между выбранными…",
  menuBisectMark: (role, term) =>
    role === "skip"
      ? "Bisect: не проверить · пропустить"
      : term !== null
        ? `Bisect: отметить «${term}»`
        : role === "bad"
          ? "Bisect: здесь ошибка есть"
          : "Bisect: здесь ошибки нет",
  whyBisectNeedsTwo: () => "выделите ровно два коммита: новый — с ошибкой, старый — без неё",
  whyBisectRunning: () => "идёт поиск коммита с ошибкой (bisect) — сначала закончите его",
  whyBisectBroken: () => "состояние bisect не читается — можно только закончить",
  whyBisectNoCommit: () => "нет коммита на проверке",
  bisectChip: (kind, bad, good) => {
    switch (kind) {
      case "culprit":
        return bad === null ? "первый плохой" : `первый ${bad}`;
      case "bad":
        return bad ?? "плохой";
      case "good":
        return good ?? "хороший";
      case "skip":
        return "пропущен";
      case "testing":
        return "проверяется";
      default:
        return "кандидат";
    }
  },
  bisectChipTip: (kind) => {
    switch (kind) {
      case "culprit":
        return "Bisect: первый плохой коммит — поиск закончился здесь";
      case "bad":
        return "Bisect: плохая граница диапазона — ошибка здесь есть";
      case "good":
        return "Bisect: отмечен — ошибки здесь нет";
      case "skip":
        return "Bisect: пропущен — этот коммит не удалось проверить";
      case "testing":
        return "Bisect: выдан на текущую проверку";
      default:
        return "Bisect: остались только пропущенные — первый плохой может быть этим";
    }
  },
  // Менеджер стешей
  stashesTitle: () => "Спрятанные изменения",
  stashesTip: () => "Спрятанные изменения",
  stashesEmpty: () => "В этом репозитории нет спрятанных изменений.",
  stashesBanner: (n: number) =>
    `${n} ${ruPlural(n, "спрятанное изменение", "спрятанных изменения", "спрятанных изменений")} — открыть`,
  stashSelectOne: () => "Выберите запись",
  stashNoBranch: () => "без ветки",
  stashNoMessage: () => "(без сообщения)",
  stashFromApp: () => "Graft",
  stashFromAppTip: () => "Спрятано приложением при переключении веток",
  stashFilesTitle: () => "Файлы в записи",
  stashFilesEmpty: () => "В этой записи нет изменённых отслеживаемых файлов.",
  stashFilesNote: () =>
    "Неотслеживаемые файлы, спрятанные вместе с остальными, здесь не перечислены — git хранит их отдельно.",
  dismiss: () => "Скрыть",
  discardedFiles: (n) => `Откатано ${n} ${ruPlural(n, "файл", "файла", "файлов")}`,
  discardedList: (n) =>
    `Откачен changelist: ${n} ${ruPlural(n, "файл", "файла", "файлов")}`,
  discardedHunk: (path) => `Откачен фрагмент в ${path}`,
  discardedLines: (path) => `Откачены строки в ${path}`,
  discardedRestore: (n) =>
    `Восстановлено из копии: ${n} ${ruPlural(n, "файл", "файла", "файлов")}`,
  discardUndo: () => "Вернуть",
  discardStaleConfirm: (paths) =>
    `Эти файлы изменились после отката:\n\n${paths.join("\n")}\n\nВсё равно восстановить? Их текущие версии сначала будут сохранены в копию, так что и это можно будет вернуть.`,
  phaseDiscardRestore: () => "восстановление",
  restoreDiscardedMenu: () => "Восстановить откаченное…",
  restoreDiscardedNoRepo: () => "Сначала откройте репозиторий",
  remotesMenu: () => "Remotes…",
  cloneMenu: () => "Клонировать…",
  remotesTitle: () => "Remotes",
  remotesEmpty: () => "У репозитория нет remote. Добавьте, чтобы делать push и fetch.",
  remoteFetch: () => "fetch",
  remotePush: () => "push",
  remotePushSame: () => "как у fetch",
  remoteNoUrl: () => "адреса нет",
  remoteBranches: (n: number) =>
    `${n} ${ruPlural(n, "удалённая ветка", "удалённые ветки", "удалённых веток")}`,
  remoteHasCredentials: () =>
    "В адресе пароль или токен (здесь скрыт). Замените адрес на такой, где его нет, а секрет пусть хранит credential helper.",
  remoteSelectOne: () => "Выберите remote",
  remoteAddBtn: () => "Добавить…",
  remoteRenameBtn: () => "Переименовать…",
  remoteUrlBtn: () => "Изменить адрес…",
  remotePushUrlBtn: () => "Адрес для push…",
  remoteRemoveBtn: () => "Удалить…",
  remoteFormAdd: () => "Новый remote",
  remoteFormRename: (name: string) => `Переименовать remote «${name}»`,
  remoteFormUrl: (name: string) => `Адрес «${name}»`,
  remoteFormPushUrl: (name: string) => `Адрес для push у «${name}»`,
  remoteNameLabel: () => "Имя",
  remoteUrlLabel: () => "Адрес",
  remoteUrlPlaceholder: () => "https://host/org/repo.git или git@host:org/repo.git",
  remotePushUrlNote: () => "Пусто — push идёт на адрес fetch.",
  remoteUrlNote: () =>
    "https, ssh, git, file или локальный путь. Без пароля и токена в адресе — их хранит credential helper.",
  remoteSave: () => "Сохранить",
  remoteHttpWarn: () => "http:// не шифруется: переданное можно прочитать по дороге.",
  remoteLoginNote: (user: string) =>
    `Вход как «${user}»: пароль git спросит через credential helper. В адрес пароль не пишите.`,
  remotesKeys: () => "↑↓ выбор · Enter изменить адрес · Delete удалить · Esc закрыть",
  confirmRemoteRemove: (name: string, n: number) =>
    `Удалить remote «${name}»?\n\n` +
    (n > 0
      ? `Вместе с ним удаляются его удалённые ветки (${n}), а ветки, которые его отслеживали, потеряют upstream.`
      : "Ветки, которые его отслеживали, потеряют upstream.") +
    " Ветки на сервере не затрагиваются.",
  phaseRemoteAdd: () => "remote add",
  phaseRemoteRename: () => "remote rename",
  phaseRemoteRemove: () => "remote remove",
  phaseRemoteSetUrl: () => "remote set-url",
  cloneTitle: () => "Клонировать репозиторий",
  cloneUrlLabel: () => "Адрес репозитория",
  cloneParentLabel: () => "В папку",
  cloneParentNone: () => "не выбрана",
  cloneChooseParent: () => "Выбрать…",
  cloneParentDialog: () => "Папка, куда клонировать",
  cloneNameLabel: () => "Имя новой папки",
  cloneDest: (path: string) => `Репозиторий окажется в ${path}`,
  cloneNeedParent: () => "Выберите папку, куда клонировать",
  cloneNeedName: () => "Введите имя новой папки",
  cloneStart: () => "Клонировать",
  cloneStop: () => "Остановить",
  cloneRunning: () => "Клонирование…",
  cloneCancelled: () => "Остановлено. Недоделанная папка удалена.",
  cloneKeys: () => "Enter клонировать · Esc закрыть (останавливает идущее клонирование)",
  discardsTitle: () => "Откаченные изменения",
  discardsEmpty: () => "В этом репозитории ещё ничего не откатывали.",
  discardFilesTitle: () => "Файлы в копии",
  discardSelectOne: () => "Выберите копию",
  discardNote: () =>
    "Восстанавливается только рабочее дерево: то, что было в индексе, в индекс не возвращается. Каждое восстановление тоже сохраняется в копию.",
  discardKeysHint: () => "↑↓ выбор · Enter восстановить · Esc закрыть",
  discardRestoreBtn: () => "Восстановить",
  discardRestoreTip: () => "Вернуть эти файлы такими, какими они были до отката",
  stashApplyBtn: () => "Применить",
  stashApplyTip: () => "Вернуть изменения, запись оставить",
  stashPopBtn: () => "Применить и снять",
  stashPopTip: () => "Вернуть изменения и удалить запись",
  stashDropBtn: () => "Удалить…",
  stashDropTip: () => "Удалить запись, не применяя её",
  stashPushBtn: () => "Спрятать текущие изменения…",
  stashPushTitle: () => "Сообщение для записи (необязательно)",
  confirmStashDrop: (label: string) =>
    `Удалить запись "${label}"?\n\nЕё изменения будут потеряны, вернуть их из приложения будет нечем.`,
  copyFailed: () => "Не удалось скопировать в буфер обмена.",
  compareSides: (a, b) => `${a} → ${b}`,
  contextMenuTip: () => "Действия (Cmd/Ctrl+Enter)",
  compareFilesTitle: () => "Изменено между двумя ревизиями",
  openCurrentVersion: () => "Открыть текущую версию",
  openCurrentVersionTip: () =>
    "Показать файл в панели изменений - только пока он изменён и не закоммичен",
  openCurrentUnchanged: () => "У файла нет незакоммиченных изменений.",
  expandGap: (n) => `Показать ${n} скрытых строк`,
  expandGapTip: () => "Запросить патч с большим контекстом вокруг изменений",
  gapTooLarge: (n, max) =>
    `${n} скрытых строк - больше потолка в ${max}: git расширил бы все хунки файла разом. Чтобы прочитать этот участок, откройте сам файл.`,
  expandTooLarge: (n, max) =>
    `С раскрытым участком патч приходит на ${n} строк - больше потолка в ${max}, который панель рисует: git расширяет все хунки файла разом. Diff оставлен как был. Чтобы прочитать этот участок, откройте сам файл.`,
  editToggle: () => "Правка",
  editToggleTip: () => "Править этот файл в правой колонке",
  editOffUnified: () => "Правка предлагается в двухколоночном виде.",
  editOffReadOnly: () =>
    "Справа ревизия, а не рабочее дерево - её нельзя править.",
  editOffLoading: () => "Читаем файл…",
  editOffBinary: () => "Файл не текстовый.",
  editOffTooLarge: () => "Файл больше 2 МиБ - потолка для правки здесь.",
  editOffMixedEol: () =>
    "В файле смешаны переводы строк CRLF и LF; правка переписала бы все его строки.",
  editOffMissing: () => "Файла больше нет на диске.",
  editUnsaved: () => "не сохранено",
  editUnsavedTip: () => "Набранное ещё не дошло до файла.",
  editSaveTip: () => "Cmd/Ctrl+S записывает файл и пересчитывает сравнение",
  editStaleAsk: () =>
    "Файл на диске изменился после того, как его здесь открыли. Перечитать его с диска или перезаписать набранным текстом?",
  editStaleReread: () => "Перечитать с диска (набранное будет потеряно)",
  editStaleOverwrite: () => "Перезаписать набранным",
  hunkEditTip: () =>
    "Diff на экране посчитан не по текущему файлу. Закончите правку, чтобы ставить в индекс или откатывать.",
  linePickTip: () => "Щелчок выбирает строку, Shift+щелчок — диапазон до неё",
  linesChosen: (n) => `${ruPlural(n, "выбрана", "выбраны", "выбрано")} ${n} ${ruPlural(n, "строка", "строки", "строк")}`,
  stageLines: () => "Подготовить строки",
  unstageLines: () => "Убрать строки",
  revertLines: () => "Откатить строки",
  clearLines: () => "Сбросить",
  stageLinesTip: () => "Поставить выбранные строки в индекс (Cmd/Ctrl+Shift+S)",
  unstageLinesTip: () => "Убрать выбранные строки из индекса (Cmd/Ctrl+Shift+U)",
  revertLinesTip: () =>
    "Отменить выбранные строки в рабочем дереве; файл сначала сохраняется в копию (Cmd/Ctrl+Shift+Backspace)",
  revertLinesConfirm: (n) =>
    `Откатить ${n} ${ruPlural(n, "выбранную строку", "выбранные строки", "выбранных строк")} в рабочем дереве? Файл сначала сохраняется в копию, его можно будет вернуть.`,
  linesKeysWorktree: () =>
    "Cmd/Ctrl+Shift+J / K: расширить · Cmd/Ctrl+Shift+S: подготовить · Cmd/Ctrl+Shift+Backspace: откатить",
  linesKeysIndex: () => "Cmd/Ctrl+Shift+J / K: расширить · Cmd/Ctrl+Shift+U: убрать",
  fileHistoryItem: () => "История файла",
  fileHistoryTip: () => "Все коммиты, которые меняли этот файл, с учётом переименований",
  fileHistoryPickFile: () => "Сначала выберите файл",
  fileHistoryTitle: (path) => `История ${path}`,
  fileHistoryFrom: (rev) => `от ${rev}`,
  fileHistoryLoading: () => "Читаю историю…",
  fileHistoryLoadingMore: () => "Загружаю ещё…",
  fileHistoryEmpty: () => "Ни один коммит ещё не менял этот файл",
  fileHistoryEmptyHint: () =>
    "У неотслеживаемого или только что добавленного файла истории нет, пока его не закоммитят.",
  fileHistoryCount: (n, more) =>
    `${n}${more ? "+" : ""} ${ruPlural(n, "коммит", "коммита", "коммитов")}`,
  fileHistoryMergesNote: () =>
    "Переименования учитываются. Merge-коммиты не показываются, как в git log --follow: их изменения видны в тех коммитах, которые они слили.",
  fileHistoryRenamedFrom: (old) => `переименован из ${old}`,
  fileHistoryKeys: () =>
    "↑ ↓ Home End: переход · Enter: показать в логе · ⌘/Ctrl+B: blame · Esc: закрыть",
  fileHistoryShowInLog: () => "Показать в логе",
  commitNotInLog: (hash) =>
    `Коммит ${hash} не найден в логе — возможно, он недостижим ни от одной ссылки, которую обходит лог.`,

  blameItem: () => "Blame (авторство строк)",
  blameTitle: (path) => `Blame: ${path}`,
  blameAt: (rev) => `в ${rev}`,
  blameWorkingTree: () => "рабочее дерево",
  blameLoading: () => "Выполняю blame…",
  blameLines: (n) => `${n} ${ruPlural(n, "строка", "строки", "строк")}`,
  blameEmptyFile: () => "В этой версии файл пуст.",
  blameBlocked: (kind) =>
    ({
      binary: "У двоичного файла нет строк для blame.",
      "too-large": "Файл слишком большой для blame (больше 4 МБ или 50 000 строк).",
      missing: "В этой версии такого файла нет.",
      untracked: "Файл ещё не отслеживается git — у него нет истории для blame.",
    })[kind],
  blameDeletedReason: () => "В этой версии файл удалён — показывать нечего",
  blameUncommitted: () => "Не закоммичено",
  blameUncommittedNote: () =>
    "Строка ещё не закоммичена. Ниже — рабочее дерево против предыдущей версии.",
  blameSelectLine: () => "Выберите строку, чтобы увидеть коммит, который менял её последним.",
  blameOpenInLog: () => "Открыть коммит в логе",
  blameOpenInLogUncommitted: () => "Строка ещё не закоммичена — коммита у неё нет",
  blameBefore: () => "Blame до этого изменения",
  blameBeforeBoundary: () =>
    "Это самая ранняя доступная версия (корневой коммит или край неглубокого клона)",
  blameBeforeCreated: () => "Файл создан в этом коммите — раньше этой строки не было",
  blameBeforeNew: () => "Файл ещё не закоммичен — более ранней версии нет",
  blameBack: (n) => `Назад (${n})`,
  blameBackTip: () => "Вернуться к предыдущему blame (Esc)",
  blamePathInCommit: (path) => `В этом коммите файл назывался ${path}`,
  blameLandedExact: (line) => `В этой версии строка была на ${line}.`,
  blameLanded: (from, to) =>
    `Строка появилась в этом изменении. Подсвечено то, что она заменила, или строка, после которой вставлена (${from === to ? from : `${from}–${to}`}).`,
  blameKeys: () =>
    "↑ ↓ PgUp PgDn Home End: переход · Enter: открыть в логе · ⌘/Ctrl+B: blame до изменения · ⌘/Ctrl+↑↓: следующее/предыдущее различие · Esc: назад / закрыть",
  conflictResolveItem: () => "Разрешить конфликт…",
  conflictResolveBtn: () => "Разрешить…",
  conflictResolveTip: (path) => `Разрешить конфликт в ${path}`,
  conflictTitle: (path) => `Разрешение конфликта: ${path}`,
  conflictKind: (k) =>
    ({
      bothModified: "файл изменён с обеих сторон",
      bothAdded: "файл добавлен с обеих сторон",
      deletedByUs: "удалён у нас, изменён у них",
      deletedByThem: "изменён у нас, удалён у них",
      addedByUs: "добавлен только у нас",
      addedByThem: "добавлен только у них",
      bothDeleted: "удалён с обеих сторон",
    })[k],
  conflictLoading: () => "Читаю конфликт…",
  conflictOurs: () => "Наши (ours)",
  conflictBase: () => "База (base)",
  conflictTheirs: () => "Их (theirs)",
  conflictResult: () => "Результат — это Сохранить запишет в файл",
  conflictLeft: (n) =>
    n === 0 ? "конфликтов не осталось" : `осталось ${n} ${ruPlural(n, "конфликт", "конфликта", "конфликтов")}`,
  conflictBlock: (i, n) => `Конфликт ${i} из ${n}`,
  conflictBlockOpen: () => "не разрешён",
  conflictBlockTaken: (how) => `разрешён: ${how}`,
  conflictHowOurs: () => "наши",
  conflictHowTheirs: () => "их",
  conflictHowBothOT: () => "наши → их",
  conflictHowBothTO: () => "их → наши",
  conflictHowLines: (n) => `${ruPlural(n, "выбрана", "выбраны", "выбрано")} ${n} ${ruPlural(n, "строка", "строки", "строк")}`,
  conflictHowManual: () => "правлен вручную",
  conflictTakeOurs: () => "Взять наши",
  conflictTakeTheirs: () => "Взять их",
  conflictBothOT: () => "Оба: наши → их",
  conflictBothTO: () => "Оба: их → наши",
  conflictResetBlock: () => "Сбросить",
  conflictResetBlockTip: () => "Вернуть маркеры этого блока",
  conflictPickTip: () => "Клик: добавить строку в результат (повторный — убрать)",
  conflictEmptySide: () => "(пусто)",
  conflictPrev: () => "Предыдущий",
  conflictNext: () => "Следующий",
  conflictNavTip: () => "Предыдущий / следующий неразрешённый конфликт (⇧F7 / F7)",
  conflictUndo: () => "Отменить",
  conflictRedo: () => "Повторить",
  conflictNothingToUndo: () => "В этом редакторе нечего отменять",
  conflictNothingToRedo: () => "В этом редакторе нечего повторять",
  conflictSave: () => "Сохранить",
  conflictSaved: () => "Сохранено",
  conflictUnsaved: () => "Есть несохранённые правки",
  conflictMarkResolved: () => "Отметить разрешённым",
  conflictMarkResolvedTip: () => "Сохранить результат и добавить в индекс (git add): конфликт разрешён",
  conflictMarkAsIs: () => "Отметить разрешённым как есть на диске",
  conflictMarkAsIsTip: () => "Добавить файл в индекс ровно таким, как он лежит в рабочем дереве (git add)",
  conflictWholeOurs: () => "Весь файл: наши",
  conflictWholeTheirs: () => "Весь файл: их",
  conflictWholeTip: (side) => `Разрешить весь файл версией ${side} (git checkout --${side} + git add)`,
  conflictWholeDeletes: (side) => `Весь файл: ${side} (удалить)`,
  conflictWholeDeletesTip: (side) =>
    `На стороне ${side} файла нет: разрешение этой стороной удаляет файл (git rm)`,
  conflictWholeDiscards: () => "В результате есть несохранённые правки. Разрешить весь файл одной стороной и потерять их?",
  conflictDiscardEdits: () => "В результате есть несохранённые правки. Закрыть редактор и потерять их?",
  conflictMarkersLeft: (lines) =>
    `В результате остались маркеры конфликта (строка ${lines}). Всё равно отметить файл разрешённым?`,
  conflictMarkAnyway: () => "Всё равно отметить",
  conflictRebaseNote: () =>
    "Во время rebase «наши» (ours) — ветка, на которую идёт rebase, а «их» (theirs) — ваш переносимый коммит.",
  conflictNoBase: () =>
    "В маркерах нет базы (merge.conflictStyle = merge). Поставьте diff3 или zdiff3, чтобы видеть её здесь, или смотрите файлы целиком.",
  conflictViewBlocks: () => "Блоки",
  conflictViewFiles: () => "Файлы целиком",
  conflictSideAbsent: () => "На этой стороне файла нет: он удалён.",
  conflictSymlink: () => "Символическая ссылка: разрешается только целиком.",
  conflictSubmodule: () => "Подмодуль: разрешается только целиком.",
  conflictWhyDeleted: () =>
    "Одна из сторон удалила файл, маркеров нет: оставьте удаление или оставьте файл.",
  conflictWhyWhole: (why) => `Здесь возможно только разрешение файла целиком. ${why}`,
  conflictParse: (reason, line, size, expected) =>
    reason === "marker-size"
      ? `Строка ${line}: маркеры конфликта длиной ${size} символов, а для этого файла ожидается ${expected}. Вероятно, атрибут conflict-marker-size поменяли после слияния; правьте результат вручную.`
      : `Строка ${line}: ${
          {
            nested: "конфликт открывается внутри другого",
            unterminated: "конфликт не закрыт",
            "no-separator": "конфликт закрывается раньше разделителя =======",
            "stray-base": "маркер базы (|||||||) не на своём месте",
            "stray-separator": "второй разделитель ======= в одном конфликте",
            "stray-closing": "закрывающий маркер без открытого конфликта",
          }[reason] ?? reason
        }. Инструменты блоков выключены, пока маркеры не читаются; правьте результат вручную.`,
  conflictResolvedNext: (path) => `Разрешено. Следующий конфликтный файл: ${path}`,
  conflictOpenNext: () => "Открыть следующий",
  conflictAllResolved: (op) => `Все конфликты разрешены. Продолжить ${op}?`,
  conflictNoneLeft: () => "Разрешено. Конфликтных файлов не осталось.",
  conflictKeys: () =>
    "Клик по строке: добавить в результат · F7 / ⇧F7: следующий / предыдущий конфликт · ⌘/Ctrl+Z, ⌘/Ctrl+⇧Z: отменить / повторить · ⌘/Ctrl+S: сохранить · Esc: закрыть",
  phaseConflictResolve: () => "отметить разрешённым",
  phaseConflictTake: () => "разрешить одной стороной",
};

/** Current locale's dictionary. Reactive: reads the `locale` signal. */
export const d = () => (locale() === "ru" ? ru : en);

/**
 * The one date format of the window, and the language decides it.
 *
 * Two rules were in the code at once: the log table wrote `15.09.2020` by hand
 * while the details card asked `toLocaleString(locale())` and got `9/15/2020,
 * 10:39:28 AM` — two formats for one commit, side by side on one screen. Dates
 * are a visible string like any other, so they follow `locale()` and nothing
 * else: not the machine's regional settings, not a per-call choice at the call
 * site. Reading `locale()` here also makes every date re-render on a language
 * switch.
 */
export const dateLocale = (): string => (locale() === "ru" ? "ru-RU" : "en-US");

/** Unix seconds from git (`%at` / `%ct`) as a date. */
export const fmtDate = (unixSeconds: number): string =>
  new Date(unixSeconds * 1000).toLocaleDateString(dateLocale());

/** Unix seconds from git as a date with the time of day. */
export const fmtDateTime = (unixSeconds: number): string =>
  new Date(unixSeconds * 1000).toLocaleString(dateLocale());

const AGO_STEPS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 86400],
  ["month", 30 * 86400],
  ["week", 7 * 86400],
  ["day", 86400],
  ["hour", 3600],
  ["minute", 60],
];

/**
 * Unix seconds from git as a distance from now — "3 months ago" — in the
 * window's language, the way a blame gutter shows age. The largest whole unit
 * wins; under a minute is "now". The exact date belongs next to it, not instead.
 */
export const fmtAgo = (unixSeconds: number, nowMs: number = Date.now()): string => {
  const secs = Math.max(0, Math.round(nowMs / 1000 - unixSeconds));
  const rtf = new Intl.RelativeTimeFormat(dateLocale(), { numeric: "auto" });
  for (const [unit, size] of AGO_STEPS) {
    if (secs >= size) return rtf.format(-Math.floor(secs / size), unit);
  }
  return rtf.format(0, "second");
};
