//! UI state of the Git panel, stored in `<git dir>/graft-ui.json` (`.git/graft-ui.json`
//! of the main worktree, `.git/worktrees/<name>/graft-ui.json` of a linked one).
//!
//! A **separate** file from `.git/changelists.json` on purpose: the changelist store
//! is byte-compatible with the TUI (see `changelists::byte_compat_with_tui_fixture`)
//! and nothing in the Git panel is allowed to read or write it. Nothing here ever
//! touches that path.
//!
//! Missing file is not an error — it is the default state. A corrupt file is not an
//! error either: UI preferences are not worth an unusable application, so a broken
//! JSON falls back to defaults and the next `set` rewrites it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{Error, Result};
use crate::model::UiState;

/// `<git dir>/graft-ui.json` — per worktree, like the changelist store
/// (`cli::private_git_file`): a linked worktree's `.git` is a file.
pub fn state_path(repo: &Path) -> Result<PathBuf> {
    crate::engine::cli::private_git_file(repo, "graft-ui.json")
}

/// Read the panel's UI state. Missing or corrupt file ⇒ defaults.
pub fn get(repo: &Path) -> Result<UiState> {
    let bytes = match std::fs::read(state_path(repo)?) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(UiState::default()),
        Err(e) => return Err(Error::Io(e.to_string())),
    };
    Ok(serde_json::from_slice(&bytes).unwrap_or_default())
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Persist atomically: unique temp name per call (pid + counter, as in
/// `changelists::save` — Правка `716da3a`), then rename over the target.
pub fn set(repo: &Path, state: &UiState) -> Result<()> {
    let path = state_path(repo)?;
    let dir = path
        .parent()
        .ok_or_else(|| Error::Io("no .git directory".into()))?;
    let mut json = serde_json::to_string_pretty(state).map_err(|e| Error::Parse(e.to_string()))?;
    json.push('\n');

    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!("graft-ui.json.tmp.{}.{}", std::process::id(), n));
    std::fs::write(&tmp, json).map_err(|e| Error::Io(e.to_string()))?;
    std::fs::rename(&tmp, &path).map_err(|e| Error::Io(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        dir
    }

    #[test]
    fn ui_state_missing_file_is_default() {
        let dir = repo();
        let st = get(dir.path()).unwrap();
        assert_eq!(st, UiState::default());
        assert_eq!(st.version, 1);
        assert!(!state_path(dir.path()).unwrap().exists(), "get must not create the file");
    }

    #[test]
    fn ui_state_roundtrip() {
        let dir = repo();
        let mut st = UiState::default();
        st.favorites = vec!["main".into(), "feature/x".into()];
        st.collapsed_folders = vec!["origin".into()];
        st.column_widths.insert("author".into(), 180);
        st.log_highlight = true;
        set(dir.path(), &st).unwrap();

        assert!(state_path(dir.path()).unwrap().exists());
        assert_eq!(get(dir.path()).unwrap(), st);
    }

    #[test]
    fn ui_state_corrupt_file_falls_back_to_default() {
        let dir = repo();
        std::fs::write(state_path(dir.path()).unwrap(), b"{ not json at all ").unwrap();
        assert_eq!(get(dir.path()).unwrap(), UiState::default());
    }

    #[test]
    fn ui_state_never_touches_changelists_json() {
        let dir = repo();
        let cl = dir.path().join(".git").join("changelists.json");
        std::fs::write(&cl, b"{\"changelists\":[],\"activeChangelistId\":\"default\"}").unwrap();
        let before = std::fs::read(&cl).unwrap();

        set(dir.path(), &UiState::default()).unwrap();
        get(dir.path()).unwrap();

        assert_eq!(std::fs::read(&cl).unwrap(), before);
    }

    /// A linked worktree's `.git` is a file; the state file goes to its own git dir,
    /// not through that file (which failed every read and write with "Not a
    /// directory").
    #[test]
    fn a_linked_worktree_reads_and_writes_its_own_state() {
        use crate::engine::cli::tests::{run_git, scratch_repo};
        let main = scratch_repo();
        let outer = tempfile::tempdir().unwrap();
        let linked = outer.path().join("linked");
        let wt = linked.to_str().unwrap();
        run_git(main.path(), &["worktree", "add", "-q", "-b", "side", wt]);

        assert_eq!(get(&linked).unwrap(), UiState::default());
        let mut st = UiState::default();
        st.favorites = vec!["side".into()];
        set(&linked, &st).unwrap();
        assert_eq!(get(&linked).unwrap(), st);
        assert!(state_path(&linked)
            .unwrap()
            .starts_with(main.path().canonicalize().unwrap().join(".git/worktrees")));
        assert_eq!(get(main.path()).unwrap(), UiState::default());
    }

    /// The file is JSON on the Tauri boundary shape: camelCase keys, version 1.
    #[test]
    fn ui_state_file_is_camel_case() {
        let dir = repo();
        set(dir.path(), &UiState::default()).unwrap();
        let text = std::fs::read_to_string(state_path(dir.path()).unwrap()).unwrap();
        assert!(text.contains("\"collapsedFolders\""), "{text}");
        assert!(text.contains("\"columnWidths\""), "{text}");
        assert!(text.contains("\"logHighlight\""), "{text}");
        assert!(text.contains("\"version\": 1"), "{text}");
    }
}
