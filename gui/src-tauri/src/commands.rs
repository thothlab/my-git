use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::State;

use crate::changelists::{self, Store};
use crate::engine::cli::CliEngine;
use crate::engine::GitEngine;
use crate::error::{Error, Result};
use crate::engine::{branches, commit as commit_engine, log as log_engine, ops};
use crate::model::{
    BranchInfo, BranchNode, ChangelistView, CommitDetails, CommitFileEntry, Eol, FileDiff,
    FileState, FileStatus, FileWritten, LogCursor, LogFilter, LogPage, RepoState, StashEntry,
    TextFile, UiState,
};
use crate::uistate;

/// Holds the currently open repository root. Commands are `async` at the Tauri layer
/// (see lib.rs) so long git work never blocks the UI thread.
#[derive(Default)]
pub struct AppState {
    pub repo: Mutex<Option<PathBuf>>,
    /// Whether the synthetic "Ignored Files" list is included in the state.
    pub show_ignored: AtomicBool,
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
pub async fn repo_open(state: State<'_, AppState>, path: Option<String>) -> Result<RepoState> {
    let start = path.unwrap_or_else(|| ".".to_string());
    let root = CliEngine::resolve_root(Path::new(&start))?;
    *state.repo.lock().unwrap() = Some(root);
    build_state(&state)
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

#[tauri::command]
pub async fn file_rollback(state: State<'_, AppState>, paths: Vec<String>) -> Result<RepoState> {
    let repo = state.repo_path()?;
    CliEngine::new(&repo).rollback(&paths)?;
    // reverted files are no longer changed; build_state's sync prunes them
    build_state(&state)
}

#[tauri::command]
pub async fn list_rollback(state: State<'_, AppState>, id: String) -> Result<RepoState> {
    let repo = state.repo_path()?;
    let store = changelists::load(&repo)?;
    let paths = changelists::list_paths(&store, &id);
    CliEngine::new(&repo).rollback(&paths)?;
    build_state(&state)
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

#[tauri::command]
pub async fn hunk_stage(state: State<'_, AppState>, patch: String) -> Result<RepoState> {
    CliEngine::new(state.repo_path()?).apply_patch(&patch, true, false)?;
    build_state(&state)
}

#[tauri::command]
pub async fn hunk_unstage(state: State<'_, AppState>, patch: String) -> Result<RepoState> {
    CliEngine::new(state.repo_path()?).apply_patch(&patch, true, true)?;
    build_state(&state)
}

#[tauri::command]
pub async fn hunk_revert(state: State<'_, AppState>, patch: String) -> Result<RepoState> {
    CliEngine::new(state.repo_path()?).apply_patch(&patch, false, true)?;
    build_state(&state)
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
    CliEngine::new(&repo).commit_paths(&paths, &message, amend)?;
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
    CliEngine::new(state.repo_path()?).create_branch(&name, from.as_deref())?;
    build_state(&state)
}

#[tauri::command]
pub async fn branch_checkout(
    state: State<'_, AppState>,
    name: String,
    stash: bool,
) -> Result<RepoState> {
    CliEngine::new(state.repo_path()?).checkout(&name, stash)?;
    build_state(&state)
}

#[tauri::command]
pub async fn push(state: State<'_, AppState>, mode: String) -> Result<RepoState> {
    CliEngine::new(state.repo_path()?).push(&mode)?;
    build_state(&state)
}

#[tauri::command]
pub async fn fetch(state: State<'_, AppState>) -> Result<RepoState> {
    CliEngine::new(state.repo_path()?).fetch()?;
    build_state(&state)
}

#[tauri::command]
pub async fn pull(state: State<'_, AppState>) -> Result<RepoState> {
    CliEngine::new(state.repo_path()?).pull()?;
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
) -> Result<FileDiff> {
    commit_engine::file_diff(&state.repo_path()?, &hash, &path, &whitespace, context)
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
    branches::rename(&state.repo_path()?, &from, &to)?;
    build_state(&state)
}

#[tauri::command]
pub async fn branch_delete(
    state: State<'_, AppState>,
    name: String,
    remote: bool,
    force: bool,
) -> Result<RepoState> {
    branches::delete(&state.repo_path()?, &name, remote, force)?;
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
    branches::merge(&state.repo_path()?, &name)?;
    build_state(&state)
}

#[tauri::command]
pub async fn branch_rebase_onto(state: State<'_, AppState>, name: String) -> Result<RepoState> {
    branches::rebase_onto(&state.repo_path()?, &name)?;
    build_state(&state)
}

// ── history panel: operations on commits (prd_02, task 06) ───────────────────

#[tauri::command]
pub async fn commit_revert(state: State<'_, AppState>, hash: String) -> Result<RepoState> {
    ops::revert(&state.repo_path()?, &hash)?;
    build_state(&state)
}

#[tauri::command]
pub async fn commit_reset(
    state: State<'_, AppState>,
    hash: String,
    mode: String,
) -> Result<RepoState> {
    ops::reset(&state.repo_path()?, &hash, &mode)?;
    build_state(&state)
}

#[tauri::command]
pub async fn commit_cherry_pick(state: State<'_, AppState>, hash: String) -> Result<RepoState> {
    ops::cherry_pick(&state.repo_path()?, &hash)?;
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
    ops::checkout_rev(&state.repo_path()?, &hash)?;
    build_state(&state)
}

#[tauri::command]
pub async fn tag_create(
    state: State<'_, AppState>,
    hash: String,
    name: String,
    message: Option<String>,
) -> Result<RepoState> {
    ops::tag_create(&state.repo_path()?, &hash, &name, message.as_deref())?;
    build_state(&state)
}

#[tauri::command]
pub async fn op_continue(state: State<'_, AppState>) -> Result<RepoState> {
    ops::op_continue(&state.repo_path()?)?;
    build_state(&state)
}

#[tauri::command]
pub async fn op_abort(state: State<'_, AppState>) -> Result<RepoState> {
    ops::op_abort(&state.repo_path()?)?;
    build_state(&state)
}

#[tauri::command]
pub async fn op_skip(state: State<'_, AppState>) -> Result<RepoState> {
    ops::op_skip(&state.repo_path()?)?;
    build_state(&state)
}

#[tauri::command]
pub async fn stash_list_app(state: State<'_, AppState>) -> Result<Vec<String>> {
    ops::stash_list_app(&state.repo_path()?)
}

#[tauri::command]
pub async fn stash_restore(state: State<'_, AppState>, name: String) -> Result<RepoState> {
    ops::stash_restore(&state.repo_path()?, &name)?;
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
    ops::stash_apply(&state.repo_path()?, &name, hash.as_deref())?;
    build_state(&state)
}

/// Restore a stash and drop the entry.
#[tauri::command]
pub async fn stash_pop(
    state: State<'_, AppState>,
    name: String,
    hash: Option<String>,
) -> Result<RepoState> {
    ops::stash_pop(&state.repo_path()?, &name, hash.as_deref())?;
    build_state(&state)
}

/// Discard a stash without applying it.
#[tauri::command]
pub async fn stash_drop(
    state: State<'_, AppState>,
    name: String,
    hash: Option<String>,
) -> Result<RepoState> {
    ops::stash_drop(&state.repo_path()?, &name, hash.as_deref())?;
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
    ops::stash_push(&state.repo_path()?, message.as_deref())?;
    build_state(&state)
}

/// Bring a branch up to date with its upstream: `pull` for the current branch, a
/// fast-forward in place for any other. Divergence and a missing upstream are
/// domain refusals stating the reason.
#[tauri::command]
pub async fn branch_update(state: State<'_, AppState>, name: String) -> Result<RepoState> {
    branches::update_from_upstream(&state.repo_path()?, &name)?;
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
