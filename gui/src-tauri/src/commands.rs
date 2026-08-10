use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::State;

use crate::changelists::{self, Store};
use crate::engine::cli::CliEngine;
use crate::engine::GitEngine;
use crate::error::{Error, Result};
use crate::model::RepoState;

/// Holds the currently open repository root. Commands are `async` at the Tauri layer
/// (see lib.rs) so long git work never blocks the UI thread.
#[derive(Default)]
pub struct AppState {
    pub repo: Mutex<Option<PathBuf>>,
}

impl AppState {
    pub fn repo_path(&self) -> Result<PathBuf> {
        self.repo
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Error::Rule("репозиторий не открыт".into()))
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
    let views = changelists::build_views(&store, &snap);

    Ok(RepoState {
        repo_path: repo.display().to_string(),
        branch: snap.branch,
        upstream: snap.upstream,
        ahead: snap.ahead,
        behind: snap.behind,
        detached: snap.detached,
        active_changelist_id: store.active_changelist_id.clone(),
        changelists: views,
    })
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
