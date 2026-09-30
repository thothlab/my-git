use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, State};

use crate::changelists::{self, Store};
use crate::engine::cli::CliEngine;
use crate::engine::GitEngine;
use crate::error::{Error, Result};
use crate::engine::exec::{self, mask_credentials};
use crate::engine::{
    blame as blame_engine, branches, commit as commit_engine, conflict as conflict_engine, discard, file_history as file_history_engine, log as log_engine, ops,
};
use crate::model::{
    Blame, BlameBefore, BranchInfo, BranchNode, ChangelistView, CommitDetails, CommitFileEntry, ConflictFile, DiscardEntry,
    DiscardKind, DiscardOutcome, Eol, FileDiff, FileHistoryCursor, FileHistoryPage, HunkPick,
    LinePick,
    FileState, FileStatus, FileWritten, GitExecResult, JournalOutput, JournalSummary, LogCursor,
    LogFilter, LogPage, RepoExternalChange, RepoState, StashEntry, TextFile, UiState,
};
use crate::uistate;
use crate::watch::{self, RepoWatcher};

/// Holds the currently open repository root. Commands are `async` at the Tauri layer
/// (see lib.rs) so long git work never blocks the UI thread.
#[derive(Default)]
pub struct AppState {
    pub repo: Mutex<Option<PathBuf>>,
    /// Whether the synthetic "Ignored Files" list is included in the state.
    pub show_ignored: AtomicBool,
    /// The git-dir watcher of the open repository (`crate::watch`). Replaced when
    /// another repository is opened — the old one stops on drop.
    pub watcher: Mutex<Option<RepoWatcher>>,
}

impl AppState {
    pub fn repo_path(&self) -> Result<PathBuf> {
        self.repo
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Error::Rule("repository not open".into()))
    }
}

// ── State assembly ───────────────────────────────────────────────────────────

/// Compute full repo state: snapshot → sync the changelist store (persisting only if
/// the sync changed it) → resolve views.
pub fn build_state(state: &State<AppState>) -> Result<RepoState> {
    let repo = state.repo_path()?;
    let snap = CliEngine::new(&repo).snapshot()?;

    let mut store = changelists::load(&repo)?;
    if changelists::sync(&mut store, &snap) {
        changelists::save(&repo, &store)?;
    }
    let mut views = changelists::build_views(&store, &snap);

    // Append the synthetic, read-only "Ignored Files" list when requested. Fetched
    // separately (never through `sync`) so ignored paths never touch the store.
    if state.show_ignored.load(Ordering::Relaxed) {
        let ignored = CliEngine::new(&repo).ignored()?;
        if !ignored.is_empty() {
            views.push(ChangelistView {
                id: "ignored".into(),
                name: "Ignored Files".into(),
                comment: String::new(),
                is_default: false,
                is_unversioned: true,
                is_ignored: true,
                files: ignored
                    .into_iter()
                    .map(|p| FileStatus {
                        path: p,
                        status: FileState::Ignored,
                        old_path: None,
                        staged: false,
                        unstaged: false,
                    })
                    .collect(),
            });
        }
    }

    Ok(RepoState {
        repo_path: repo.display().to_string(),
        branch: snap.branch,
        upstream: snap.upstream,
        ahead: snap.ahead,
        behind: snap.behind,
        detached: snap.detached,
        active_changelist_id: store.active_changelist_id.clone(),
        changelists: views,
        // The unfinished-operation banner has to appear on its own (История 30), and
        // every mutation already returns RepoState — so this travels with the state
        // instead of a second `op_state` command that would be a rival source of truth.
        operation: ops::detect_state(&repo)?,
        user_email: crate::engine::cli::user_email(&repo),
    })
}

/// Toggle inclusion of the synthetic "Ignored Files" list (session-scoped).
#[tauri::command]
pub async fn set_show_ignored(state: State<'_, AppState>, value: bool) -> Result<RepoState> {
    state.show_ignored.store(value, Ordering::Relaxed);
    build_state(&state)
}

/// Load store → run a validated mutation → persist → recompute state.
fn mutate<F>(state: &State<AppState>, f: F) -> Result<RepoState>
where
    F: FnOnce(&mut Store) -> Result<()>,
{
    let repo = state.repo_path()?;
    let mut store = changelists::load(&repo)?;
    f(&mut store)?;
    changelists::save(&repo, &store)?;
    build_state(state)
}

// ── Commands ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn repo_open(
    app: AppHandle,
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<RepoState> {
    let start = path.unwrap_or_else(|| ".".to_string());
    let root = CliEngine::resolve_root(Path::new(&start))?;
    *state.repo.lock().unwrap() = Some(root.clone());
    watch_repo(&app, &state, &root);
    build_state(&state)
}

/// Point the git-dir watcher at `root`: kept if it already watches it, else the old
/// one is dropped (stopped) first and a new one started.
///
/// A watcher that cannot start is not a failed open: the repository is readable, and
/// what the watcher would add — seeing a terminal's commit without coming back to
/// the window — the focus refresh still delivers.
fn watch_repo(app: &AppHandle, state: &State<AppState>, root: &Path) {
    let mut slot = state.watcher.lock().unwrap();
    if slot.as_ref().is_some_and(|w| w.repo() == root) {
        return;
    }
    *slot = None;
    let app = app.clone();
    let started = watch::start(
        root,
        || exec::OWN_ACTIONS.within(watch::OWN_GRACE),
        move |repo| {
            let _ = app.emit(
                "repo-external-change",
                RepoExternalChange {
                    repo_path: repo.display().to_string(),
                },
            );
        },
    );
    match started {
        Ok(w) => *slot = Some(w),
        Err(e) => eprintln!("graft: not watching {}: {e}", root.display()),
    }
}

#[tauri::command]
pub async fn repo_state(state: State<'_, AppState>) -> Result<RepoState> {
    build_state(&state)
}

#[tauri::command]
pub async fn changelist_create(state: State<'_, AppState>, name: String) -> Result<RepoState> {
    mutate(&state, |s| changelists::create(s, &name).map(|_| ()))
}

#[tauri::command]
pub async fn changelist_rename(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> Result<RepoState> {
    mutate(&state, |s| changelists::rename(s, &id, &name))
}

#[tauri::command]
pub async fn changelist_set_comment(
    state: State<'_, AppState>,
    id: String,
    comment: String,
) -> Result<RepoState> {
    mutate(&state, |s| changelists::set_comment(s, &id, &comment))
}

#[tauri::command]
pub async fn changelist_delete(state: State<'_, AppState>, id: String) -> Result<RepoState> {
    mutate(&state, |s| changelists::delete(s, &id))
}

#[tauri::command]
pub async fn changelist_set_active(state: State<'_, AppState>, id: String) -> Result<RepoState> {
    mutate(&state, |s| changelists::set_active(s, &id))
}

#[tauri::command]
pub async fn files_move(
    state: State<'_, AppState>,
    paths: Vec<String>,
    to_list_id: String,
) -> Result<RepoState> {
    mutate(&state, |s| changelists::move_files(s, &paths, &to_list_id))
}

/// Roll files back to HEAD, backing their working-tree copies up first
/// (`engine::discard`). A backup that cannot be taken stops the rollback.
#[tauri::command]
pub async fn file_rollback(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<DiscardOutcome> {
    let repo = state.repo_path()?;
    let backup = exec::as_user("file_rollback", || {
        discard::with_backup(&repo, DiscardKind::Files, &paths, || {
            CliEngine::new(&repo).rollback(&paths)
        })
    })?;
    // reverted files are no longer changed; build_state's sync prunes them
    Ok(DiscardOutcome {
        state: build_state(&state)?,
        backup,
    })
}

#[tauri::command]
pub async fn list_rollback(state: State<'_, AppState>, id: String) -> Result<DiscardOutcome> {
    let repo = state.repo_path()?;
    let store = changelists::load(&repo)?;
    let paths = changelists::list_paths(&store, &id);
    let backup = exec::as_user("list_rollback", || {
        discard::with_backup(&repo, DiscardKind::List, &paths, || {
            CliEngine::new(&repo).rollback(&paths)
        })
    })?;
    Ok(DiscardOutcome {
        state: build_state(&state)?,
        backup,
    })
}

/// Restorable discard backups, newest first — at most `limit`. Read-only.
#[tauri::command]
pub async fn discard_list(state: State<'_, AppState>, limit: u32) -> Result<Vec<DiscardEntry>> {
    discard::list(&state.repo_path()?, limit as usize)
}

/// Read-only: the paths of backup `id` changed since the discard. Empty means a
/// restore overwrites nothing new; otherwise the client asks before forcing.
#[tauri::command]
pub async fn discard_check(state: State<'_, AppState>, id: String) -> Result<Vec<String>> {
    discard::stale_paths(&state.repo_path()?, &id)
}

/// Put the files of backup `id` back as they were before the discard. Refused with
/// `stale` over files changed since, unless `force`; the restore is itself backed up.
#[tauri::command]
pub async fn discard_restore(
    state: State<'_, AppState>,
    id: String,
    force: bool,
) -> Result<DiscardOutcome> {
    let repo = state.repo_path()?;
    let backup = exec::as_user("discard_restore", || discard::restore(&repo, &id, force))?;
    Ok(DiscardOutcome {
        state: build_state(&state)?,
        backup,
    })
}

// ── diff & hunk-level staging (task_04) ──────────────────────────────────────

/// `context` is how many unchanged lines to keep around each change; absent
/// means "as before" — no `-U` on the command line at all (R46i, D04).
#[tauri::command]
pub async fn diff_file(
    state: State<'_, AppState>,
    path: String,
    against: String,
    whitespace: String,
    context: Option<u32>,
) -> Result<FileDiff> {
    CliEngine::new(state.repo_path()?).diff_file(&path, &against, &whitespace, context)
}

/// Read a working-tree file for in-place editing. Read-only, own type — the frontend
/// drives it with `createResource` + `refetch`.
#[tauri::command]
pub async fn file_read(state: State<'_, AppState>, path: String) -> Result<TextFile> {
    CliEngine::new(state.repo_path()?).read_text_file(&path)
}

/// Write an edited working-tree file back.
///
/// The one mutation of this project that does **not** return `RepoState` and does not
/// go through `run()`: it fires on every pause in typing, and republishing global
/// state that often would flicker the toolbar busy and re-lay out the panel under the
/// caret. It reports the new digest instead; the client replaces its previous one with
/// it, or its own next save would be refused as stale.
///
/// `text` is written as given, endings converted to `eol`: there is no `finalNewline`
/// argument, because the trailing newline is part of the text the editor holds.
#[tauri::command]
pub async fn file_write(
    state: State<'_, AppState>,
    path: String,
    text: String,
    eol: Eol,
    expect: String,
) -> Result<FileWritten> {
    let digest = CliEngine::new(state.repo_path()?).write_text_file(&path, &text, eol, &expect)?;
    Ok(FileWritten { digest })
}

/// Stage the chosen lines of a file (a whole hunk is all of its lines).
///
/// The client names hunks and lines of the working-tree diff it was shown and sends
/// that diff's `digest` and `context`; the patch is rebuilt here from the diff read
/// again, and a diff that no longer matches the digest is `Error::Stale`.
#[tauri::command]
pub async fn lines_stage(
    state: State<'_, AppState>,
    path: String,
    picks: Vec<HunkPick>,
    digest: String,
    context: Option<u32>,
) -> Result<RepoState> {
    exec::as_user("lines_stage", || {
        let eng = CliEngine::new(state.repo_path()?);
        let patch = eng.selection_patch(&path, "worktree", &picks, &digest, context, false)?;
        eng.apply_patch(&patch, true, false)
    })?;
    build_state(&state)
}

/// Unstage the chosen lines of a file: the same, over `diff --cached`, reversed.
#[tauri::command]
pub async fn lines_unstage(
    state: State<'_, AppState>,
    path: String,
    picks: Vec<HunkPick>,
    digest: String,
    context: Option<u32>,
) -> Result<RepoState> {
    exec::as_user("lines_unstage", || {
        let eng = CliEngine::new(state.repo_path()?);
        let patch = eng.selection_patch(&path, "index", &picks, &digest, context, true)?;
        eng.apply_patch(&patch, true, true)
    })?;
    build_state(&state)
}

/// Revert the chosen lines of a file in the working tree, backing the file up first.
/// The working-tree diff applied in reverse: the mirror of staging, the same rule as
/// unstaging. The backup's paths come from the patch as git reads it.
#[tauri::command]
pub async fn lines_revert(
    state: State<'_, AppState>,
    path: String,
    picks: Vec<HunkPick>,
    digest: String,
    context: Option<u32>,
) -> Result<DiscardOutcome> {
    let repo = state.repo_path()?;
    // Named for what the reader did: whole hunks from the hunk buttons, lines otherwise.
    let kind = if picks.iter().all(|p| matches!(p.lines, LinePick::All(_))) {
        DiscardKind::Hunk
    } else {
        DiscardKind::Lines
    };
    let backup = exec::as_user("lines_revert", || {
        let eng = CliEngine::new(&repo);
        let patch = eng.selection_patch(&path, "worktree", &picks, &digest, context, true)?;
        let paths = discard::patch_paths(&repo, &patch)?;
        discard::with_backup(&repo, kind, &paths, || eng.apply_patch(&patch, false, true))
    })?;
    Ok(DiscardOutcome {
        state: build_state(&state)?,
        backup,
    })
}

// ── commit (task_05) ─────────────────────────────────────────────────────────

/// Commit a changelist (`id`) or an explicit `paths` subset (marked files). Explicit
/// paths win over `id`.
#[tauri::command]
pub async fn commit_list(
    state: State<'_, AppState>,
    id: Option<String>,
    paths: Option<Vec<String>>,
    message: String,
    amend: bool,
) -> Result<RepoState> {
    let repo = state.repo_path()?;
    let store = changelists::load(&repo)?;
    let paths = match paths {
        Some(p) if !p.is_empty() => p,
        _ => {
            let id = id.ok_or_else(|| Error::Rule("no list or files selected".into()))?;
            changelists::list_paths(&store, &id)
        }
    };
    if paths.is_empty() {
        return Err(Error::Rule("no files to commit".into()));
    }
    exec::as_user("commit_list", || CliEngine::new(&repo).commit_paths(&paths, &message, amend))?;
    build_state(&state)
}

// ── branches & remotes (task_06) ─────────────────────────────────────────────

#[tauri::command]
pub async fn branch_list(state: State<'_, AppState>) -> Result<Vec<BranchInfo>> {
    CliEngine::new(state.repo_path()?).branches()
}

#[tauri::command]
pub async fn branch_create(
    state: State<'_, AppState>,
    name: String,
    from: Option<String>,
) -> Result<RepoState> {
    exec::as_user("branch_create", || {
        CliEngine::new(state.repo_path()?).create_branch(&name, from.as_deref())
    })?;
    build_state(&state)
}

#[tauri::command]
pub async fn branch_checkout(
    state: State<'_, AppState>,
    name: String,
    stash: bool,
) -> Result<RepoState> {
    exec::as_user("branch_checkout", || CliEngine::new(state.repo_path()?).checkout(&name, stash))?;
    build_state(&state)
}

#[tauri::command]
pub async fn push(state: State<'_, AppState>, mode: String) -> Result<RepoState> {
    exec::as_user("push", || CliEngine::new(state.repo_path()?).push(&mode))?;
    build_state(&state)
}

#[tauri::command]
pub async fn fetch(state: State<'_, AppState>) -> Result<RepoState> {
    exec::as_user("fetch", || CliEngine::new(state.repo_path()?).fetch())?;
    build_state(&state)
}

/// The git console panel: run an arbitrary `git <args>` in the open repository.
/// See `CliEngine::exec_raw` — a non-zero exit is output for the user to read,
/// not an `Err` here; only a failure to spawn `git` itself is.
#[tauri::command]
pub async fn git_exec(state: State<'_, AppState>, args: Vec<String>) -> Result<GitExecResult> {
    let repo = state.repo_path()?;
    let out = exec::as_user("git_exec", || CliEngine::new(&repo).exec_raw(&args))?;
    Ok(GitExecResult {
        stdout: mask_credentials(&out.stdout).into_owned(),
        stderr: mask_credentials(&out.stderr).into_owned(),
        exit_code: out.exit_code,
        journal_id: out.journal,
        state: build_state(&state)?,
    })
}

#[tauri::command]
pub async fn pull(state: State<'_, AppState>) -> Result<RepoState> {
    exec::as_user("pull", || CliEngine::new(state.repo_path()?).pull())?;
    build_state(&state)
}

// ── history panel: log (prd_02, task 03) ─────────────────────────────────────

#[tauri::command]
pub async fn log_page(
    state: State<'_, AppState>,
    filter: LogFilter,
    cursor: Option<LogCursor>,
    limit: u32,
) -> Result<LogPage> {
    log_engine::page(&state.repo_path()?, &filter, cursor.as_ref(), limit)
}

#[tauri::command]
pub async fn log_authors(state: State<'_, AppState>) -> Result<Vec<String>> {
    log_engine::authors(&state.repo_path()?)
}

// ── history panel: one commit (prd_02, task 04) ──────────────────────────────

#[tauri::command]
pub async fn commit_details(state: State<'_, AppState>, hash: String) -> Result<CommitDetails> {
    commit_engine::details(&state.repo_path()?, &hash)
}

#[tauri::command]
pub async fn commit_files(
    state: State<'_, AppState>,
    hash: String,
) -> Result<Vec<CommitFileEntry>> {
    commit_engine::files(&state.repo_path()?, &hash)
}

#[tauri::command]
pub async fn commit_file_diff(
    state: State<'_, AppState>,
    hash: String,
    path: String,
    whitespace: String,
    context: Option<u32>,
    old_path: Option<String>,
) -> Result<FileDiff> {
    commit_engine::file_diff(
        &state.repo_path()?,
        &hash,
        &path,
        old_path.as_deref(),
        &whitespace,
        context,
    )
}

/// Every commit that touched one file, renames followed (R05c). Read-only.
/// `rev` is where the history starts — `None` is `HEAD`; the Log mode passes the
/// commit whose file was picked, so the path is the one the file had there.
#[tauri::command]
pub async fn file_history(
    state: State<'_, AppState>,
    path: String,
    rev: Option<String>,
    cursor: Option<FileHistoryCursor>,
    limit: u32,
) -> Result<FileHistoryPage> {
    file_history_engine::page(&state.repo_path()?, &path, rev.as_deref(), cursor.as_ref(), limit)
}

/// Blame of one file (R05b). Read-only. `rev` is any revision naming a commit;
/// `None` blames the working tree, uncommitted lines included. An unfit file
/// (binary, too large, missing, untracked) is `blocked`, not an error.
#[tauri::command]
pub async fn file_blame(
    state: State<'_, AppState>,
    path: String,
    rev: Option<String>,
) -> Result<Blame> {
    blame_engine::file(&state.repo_path()?, rev.as_deref(), &path)
}

/// "Blame before this change": the blame of the version a line's commit started
/// from (`prev_hash` / `prev_path` — the line's `previous`) and where line `line`
/// of `path` at `hash` lands in it. Read-only.
#[tauri::command]
pub async fn file_blame_before(
    state: State<'_, AppState>,
    hash: String,
    path: String,
    line: u32,
    prev_hash: String,
    prev_path: String,
) -> Result<BlameBefore> {
    blame_engine::before(&state.repo_path()?, &hash, &path, line, &prev_hash, &prev_path)
}

/// Which of these commits the current revision cannot reach — the input behind
/// the log's row emphasis (R45i, D05). Read-only, asked per loaded page.
#[tauri::command]
pub async fn commits_unreachable(
    state: State<'_, AppState>,
    hashes: Vec<String>,
) -> Result<Vec<String>> {
    commit_engine::unreachable_from_head(&state.repo_path()?, &hashes)
}

#[tauri::command]
pub async fn commits_compare(
    state: State<'_, AppState>,
    from: String,
    to: String,
) -> Result<Vec<CommitFileEntry>> {
    commit_engine::compare(&state.repo_path()?, &from, &to)
}

#[tauri::command]
pub async fn commits_compare_diff(
    state: State<'_, AppState>,
    from: String,
    to: String,
    path: String,
    whitespace: String,
    context: Option<u32>,
) -> Result<FileDiff> {
    commit_engine::compare_diff(&state.repo_path()?, &from, &to, &path, &whitespace, context)
}

// ── history panel: branch tree (prd_02, task 05) ─────────────────────────────

#[tauri::command]
pub async fn branch_tree(state: State<'_, AppState>) -> Result<Vec<BranchNode>> {
    branches::tree(&state.repo_path()?)
}

#[tauri::command]
pub async fn branch_rename(
    state: State<'_, AppState>,
    from: String,
    to: String,
) -> Result<RepoState> {
    exec::as_user("branch_rename", || branches::rename(&state.repo_path()?, &from, &to))?;
    build_state(&state)
}

#[tauri::command]
pub async fn branch_delete(
    state: State<'_, AppState>,
    name: String,
    remote: bool,
    force: bool,
) -> Result<RepoState> {
    exec::as_user("branch_delete", || branches::delete(&state.repo_path()?, &name, remote, force))?;
    build_state(&state)
}

/// Read-only: how many commits deleting `name` would lose, by git's own definition
/// of "not fully merged". Its own command because the confirmation dialog has to
/// name the number **before** the deletion, not learn it from a failed attempt.
#[tauri::command]
pub async fn branch_unmerged_count(state: State<'_, AppState>, name: String) -> Result<u32> {
    branches::unmerged_count(&state.repo_path()?, &name)
}

#[tauri::command]
pub async fn branch_merge(state: State<'_, AppState>, name: String) -> Result<RepoState> {
    exec::as_user("branch_merge", || branches::merge(&state.repo_path()?, &name))?;
    build_state(&state)
}

#[tauri::command]
pub async fn branch_rebase_onto(state: State<'_, AppState>, name: String) -> Result<RepoState> {
    exec::as_user("branch_rebase_onto", || branches::rebase_onto(&state.repo_path()?, &name))?;
    build_state(&state)
}

// ── history panel: operations on commits (prd_02, task 06) ───────────────────

#[tauri::command]
pub async fn commit_revert(state: State<'_, AppState>, hash: String) -> Result<RepoState> {
    exec::as_user("commit_revert", || ops::revert(&state.repo_path()?, &hash))?;
    build_state(&state)
}

#[tauri::command]
pub async fn commit_reset(
    state: State<'_, AppState>,
    hash: String,
    mode: String,
) -> Result<RepoState> {
    exec::as_user("commit_reset", || ops::reset(&state.repo_path()?, &hash, &mode))?;
    build_state(&state)
}

#[tauri::command]
pub async fn commit_cherry_pick(state: State<'_, AppState>, hash: String) -> Result<RepoState> {
    exec::as_user("commit_cherry_pick", || ops::cherry_pick(&state.repo_path()?, &hash))?;
    build_state(&state)
}

/// Read-only: is this commit already on the current branch (by ancestry or by an
/// equivalent patch)? Its own command because История 58 disables the cherry-pick
/// menu item **before** it is clicked, with the reason stated.
#[tauri::command]
pub async fn commit_contains(state: State<'_, AppState>, hash: String) -> Result<bool> {
    ops::contains_commit(&state.repo_path()?, &hash)
}

/// Read-only: how many commits a reset to `hash` would discard. История 57 makes
/// the hard-reset confirmation name the number before the operation, not after.
#[tauri::command]
pub async fn commit_reset_lost_count(state: State<'_, AppState>, hash: String) -> Result<u32> {
    ops::commits_after(&state.repo_path()?, &hash)
}

/// Read-only: does the working tree or index carry anything uncommitted? The other
/// half of the hard-reset warning.
#[tauri::command]
pub async fn repo_local_changes(state: State<'_, AppState>) -> Result<bool> {
    ops::has_local_changes(&state.repo_path()?)
}

#[tauri::command]
pub async fn commit_checkout(state: State<'_, AppState>, hash: String) -> Result<RepoState> {
    exec::as_user("commit_checkout", || ops::checkout_rev(&state.repo_path()?, &hash))?;
    build_state(&state)
}

#[tauri::command]
pub async fn tag_create(
    state: State<'_, AppState>,
    hash: String,
    name: String,
    message: Option<String>,
) -> Result<RepoState> {
    exec::as_user("tag_create", || {
        ops::tag_create(&state.repo_path()?, &hash, &name, message.as_deref())
    })?;
    build_state(&state)
}

// ── conflict resolution (R05e) ───────────────────────────────────────────────

/// One conflicted path for the conflict editor: the three sides from the index, the
/// working file with git's markers, the marker size and whether it can be resolved
/// line by line at all. Read-only. A path with nothing to resolve is `Error::Rule`.
#[tauri::command]
pub async fn conflict_read(state: State<'_, AppState>, path: String) -> Result<ConflictFile> {
    conflict_engine::read(&state.repo_path()?, &path)
}

/// Mark a conflict resolved (`git add`), writing the resolution first when `text` is
/// given. `expect` is the digest of the file the editor last read or wrote — a file
/// changed underneath is `Error::Stale` and nothing is staged; `None` takes the file
/// as it lies (a binary one, resolved elsewhere).
#[tauri::command]
pub async fn conflict_resolve(
    state: State<'_, AppState>,
    path: String,
    text: Option<String>,
    eol: Eol,
    expect: Option<String>,
) -> Result<RepoState> {
    let repo = state.repo_path()?;
    exec::as_user("conflict_resolve", || {
        conflict_engine::resolve(&repo, &path, text.as_deref(), eol, expect.as_deref())
    })?;
    build_state(&state)
}

/// Resolve a conflict by taking one side whole — `ours` or `theirs`. A side on which
/// the file was deleted resolves to the deletion (`git rm`).
#[tauri::command]
pub async fn conflict_take(
    state: State<'_, AppState>,
    path: String,
    side: String,
) -> Result<RepoState> {
    let repo = state.repo_path()?;
    exec::as_user("conflict_take", || conflict_engine::take(&repo, &path, &side))?;
    build_state(&state)
}

#[tauri::command]
pub async fn op_continue(state: State<'_, AppState>) -> Result<RepoState> {
    exec::as_user("op_continue", || ops::op_continue(&state.repo_path()?))?;
    build_state(&state)
}

#[tauri::command]
pub async fn op_abort(state: State<'_, AppState>) -> Result<RepoState> {
    exec::as_user("op_abort", || ops::op_abort(&state.repo_path()?))?;
    build_state(&state)
}

#[tauri::command]
pub async fn op_skip(state: State<'_, AppState>) -> Result<RepoState> {
    exec::as_user("op_skip", || ops::op_skip(&state.repo_path()?))?;
    build_state(&state)
}

#[tauri::command]
pub async fn stash_list_app(state: State<'_, AppState>) -> Result<Vec<String>> {
    ops::stash_list_app(&state.repo_path()?)
}

#[tauri::command]
pub async fn stash_restore(state: State<'_, AppState>, name: String) -> Result<RepoState> {
    exec::as_user("stash_restore", || ops::stash_restore(&state.repo_path()?, &name))?;
    build_state(&state)
}

/// Every stash in the repository — the stash manager's list (`stash_list_app` is
/// the older, narrower view kept for the branch-switch dialog).
#[tauri::command]
pub async fn stash_list(state: State<'_, AppState>) -> Result<Vec<StashEntry>> {
    ops::stash_list(&state.repo_path()?)
}

/// Restore a stash and keep the entry. `hash` is the stash commit as it was listed:
/// `stash@{N}` renumbers after any pop or drop, and a stale index would otherwise
/// act on the neighbouring stash.
#[tauri::command]
pub async fn stash_apply(
    state: State<'_, AppState>,
    name: String,
    hash: Option<String>,
) -> Result<RepoState> {
    exec::as_user("stash_apply", || ops::stash_apply(&state.repo_path()?, &name, hash.as_deref()))?;
    build_state(&state)
}

/// Restore a stash and drop the entry.
#[tauri::command]
pub async fn stash_pop(
    state: State<'_, AppState>,
    name: String,
    hash: Option<String>,
) -> Result<RepoState> {
    exec::as_user("stash_pop", || ops::stash_pop(&state.repo_path()?, &name, hash.as_deref()))?;
    build_state(&state)
}

/// Discard a stash without applying it.
#[tauri::command]
pub async fn stash_drop(
    state: State<'_, AppState>,
    name: String,
    hash: Option<String>,
) -> Result<RepoState> {
    exec::as_user("stash_drop", || ops::stash_drop(&state.repo_path()?, &name, hash.as_deref()))?;
    build_state(&state)
}

/// Read-only: what a stash changes, in the shape of a commit's file list.
#[tauri::command]
pub async fn stash_files(
    state: State<'_, AppState>,
    name: String,
) -> Result<Vec<CommitFileEntry>> {
    ops::stash_files(&state.repo_path()?, &name)
}

/// Stash the current changes (untracked included) under a message.
#[tauri::command]
pub async fn stash_push(
    state: State<'_, AppState>,
    message: Option<String>,
) -> Result<RepoState> {
    exec::as_user("stash_push", || ops::stash_push(&state.repo_path()?, message.as_deref()))?;
    build_state(&state)
}

/// Bring a branch up to date with its upstream: `pull` for the current branch, a
/// fast-forward in place for any other. Divergence and a missing upstream are
/// domain refusals stating the reason.
#[tauri::command]
pub async fn branch_update(state: State<'_, AppState>, name: String) -> Result<RepoState> {
    exec::as_user("branch_update", || branches::update_from_upstream(&state.repo_path()?, &name))?;
    build_state(&state)
}

// ── history panel: UI state file (prd_02, task 01) ───────────────────────────

#[tauri::command]
pub async fn ui_state_get(state: State<'_, AppState>) -> Result<UiState> {
    uistate::get(&state.repo_path()?)
}

#[tauri::command]
pub async fn ui_state_set(state: State<'_, AppState>, ui: UiState) -> Result<UiState> {
    let repo = state.repo_path()?;
    uistate::set(&repo, &ui)?;
    uistate::get(&repo)
}

// ── command journal ──────────────────────────────────────────────────────────

/// Every git run of this process, oldest first, without outputs (`journal_output`).
///
/// `mine` reads the user ring (1000 actions) alone; otherwise the user and background
/// (2000 reads) rings come merged by id; `after` returns entries newer than that id, so a
/// client polling while its panel is open receives only what is new. Reads process
/// memory only: no git, no open repository needed — which is also why polling it can
/// never feed the journal it reads.
#[tauri::command]
pub async fn journal_list(mine: bool, after: Option<u64>) -> Result<Vec<JournalSummary>> {
    Ok(exec::journal_list(mine, after))
}

/// Both streams of one journal entry; `null` once the ring has dropped it.
#[tauri::command]
pub async fn journal_output(id: u64) -> Result<Option<JournalOutput>> {
    Ok(exec::journal_output(id))
}
