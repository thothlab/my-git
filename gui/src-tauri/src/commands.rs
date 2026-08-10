use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::State;

use crate::engine::cli::CliEngine;
use crate::engine::GitEngine;
use crate::error::{Error, Result};
use crate::model::{ChangelistView, FileState, RepoState};

/// Holds the currently open repository root. Commands are `async` at the Tauri
/// layer (see lib.rs registration) so long git work never blocks the UI thread.
#[derive(Default)]
pub struct AppState {
    pub repo: Mutex<Option<PathBuf>>,
}

impl AppState {
    fn repo_path(&self) -> Result<PathBuf> {
        self.repo
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Error::Rule("no repository open".into()))
    }
}

fn engine(state: &State<AppState>) -> Result<CliEngine> {
    Ok(CliEngine::new(state.repo_path()?))
}

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

/// Compute the full repo state for the UI.
///
/// task_01 grouping is intentionally minimal — tracked → Default, untracked →
/// Unversioned Files — with no persistence. task_02 replaces this with the real
/// changelist store (`.git/changelists.json`, byte-compatible with the TUI).
fn build_state(state: &State<AppState>) -> Result<RepoState> {
    let eng = engine(state)?;
    let snap = eng.snapshot()?;
    let repo_path = state.repo_path()?.display().to_string();

    let (mut tracked, mut untracked) = (Vec::new(), Vec::new());
    for f in snap.files {
        if f.status == FileState::Untracked {
            untracked.push(f);
        } else {
            tracked.push(f);
        }
    }

    let mut changelists = vec![ChangelistView {
        id: "default".into(),
        name: "Default".into(),
        comment: String::new(),
        is_default: true,
        is_unversioned: false,
        files: tracked,
    }];
    if !untracked.is_empty() {
        changelists.push(ChangelistView {
            id: "unversioned".into(),
            name: "Unversioned Files".into(),
            comment: String::new(),
            is_default: false,
            is_unversioned: true,
            files: untracked,
        });
    }

    Ok(RepoState {
        repo_path,
        branch: snap.branch,
        upstream: snap.upstream,
        ahead: snap.ahead,
        behind: snap.behind,
        detached: snap.detached,
        active_changelist_id: "default".into(),
        changelists,
    })
}
