//! Operations that rewrite history, and the state of an unfinished one.
//!
//! Hides the sequence of git commands and the recognition of an in-progress
//! operation from files under `.git/`. Filled in by task 06 — except
//! [`detect_state`], which already answers "no operation" for a calm repository so
//! `RepoState.operation` is honest from the first task.

use std::path::Path;

use crate::engine::cli::CliEngine;
use crate::error::{Error, Result};
use crate::model::{OperationKind, OperationState};

fn todo_ops() -> Error {
    Error::Rule("ops: not implemented".into())
}

/// Which multi-step operation, if any, is in progress.
///
/// Recognised by the marker files git itself writes. Their location is resolved by
/// git (`rev-parse --git-path`), not by joining `.git` onto the worktree root: in a
/// linked worktree and in a submodule `.git` is a file and the markers live under
/// `.git/worktrees/<name>/`, so a concatenated path would quietly report "no
/// operation" and the banner would never appear. A path git cannot resolve is an
/// error, not silence.
///
/// `current` / `total` / `conflicted` are filled by task 06; a calm repository is
/// already reported exactly — `OperationKind::None`.
pub fn detect_state(repo: &Path) -> Result<OperationState> {
    const MARKERS: [&str; 5] = [
        "MERGE_HEAD",
        "rebase-merge",
        "rebase-apply",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
    ];
    let paths = CliEngine::new(repo).git_paths(&MARKERS)?;
    let present: Vec<bool> = paths.iter().map(|p| p.exists()).collect();

    let kind = if present[0] {
        OperationKind::Merge
    } else if present[1] || present[2] {
        OperationKind::Rebase
    } else if present[3] {
        OperationKind::CherryPick
    } else if present[4] {
        OperationKind::Revert
    } else {
        OperationKind::None
    };
    // TODO(prd): task 06 — msgnum/end for "N of M" and the conflicted paths.
    Ok(OperationState {
        kind,
        current: None,
        total: None,
        conflicted: Vec::new(),
    })
}

/// git flag for a reset mode: `soft` | `mixed` | `hard` | `keep` (манифест G02,
/// История 57). Unknown values are rejected rather than folded into a default —
/// guessing here would discard the user's work under a different mode than asked.
pub fn reset_mode_flag(mode: &str) -> Result<&'static str> {
    match mode {
        "soft" => Ok("--soft"),
        "mixed" => Ok("--mixed"),
        "hard" => Ok("--hard"),
        "keep" => Ok("--keep"),
        other => Err(Error::Rule(format!(
            "unknown reset mode: {other} (expected soft, mixed, hard or keep)"
        ))),
    }
}

pub fn revert(_repo: &Path, _hash: &str) -> Result<()> {
    // TODO(prd): task 06.
    Err(todo_ops())
}

/// Reset to `hash` in one of git's modes: `soft` | `mixed` | `hard` | `keep`.
/// The mode is validated at the boundary before any git call.
pub fn reset(_repo: &Path, _hash: &str, mode: &str) -> Result<()> {
    let _flag = reset_mode_flag(mode)?;
    // TODO(prd): task 06.
    Err(todo_ops())
}

pub fn cherry_pick(_repo: &Path, _hash: &str) -> Result<()> {
    // TODO(prd): task 06.
    Err(todo_ops())
}

pub fn checkout_rev(_repo: &Path, _hash: &str) -> Result<()> {
    // TODO(prd): task 06.
    Err(todo_ops())
}

pub fn tag_create(_repo: &Path, _hash: &str, _name: &str, _message: Option<&str>) -> Result<()> {
    // TODO(prd): task 06.
    Err(todo_ops())
}

pub fn op_continue(_repo: &Path) -> Result<()> {
    // TODO(prd): task 06.
    Err(todo_ops())
}

pub fn op_abort(_repo: &Path) -> Result<()> {
    // TODO(prd): task 06.
    Err(todo_ops())
}

pub fn op_skip(_repo: &Path) -> Result<()> {
    // TODO(prd): task 06.
    Err(todo_ops())
}

/// Stashes created by the application, newest first.
pub fn stash_list_app(_repo: &Path) -> Result<Vec<String>> {
    // TODO(prd): task 06.
    Err(todo_ops())
}

pub fn stash_restore(_repo: &Path, _name: &str) -> Result<()> {
    // TODO(prd): task 06.
    Err(todo_ops())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::scratch_repo;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Where git itself keeps a marker for this worktree.
    fn marker_path(dir: &Path, name: &str) -> std::path::PathBuf {
        let p = std::path::PathBuf::from(git(dir, &["rev-parse", "--git-path", name]));
        if p.is_absolute() {
            p
        } else {
            dir.join(p)
        }
    }

    #[test]
    fn detect_state_calm_repository_reports_none() {
        let dir = scratch_repo();
        assert_eq!(detect_state(dir.path()).unwrap().kind, OperationKind::None);
    }

    #[test]
    fn detect_state_recognises_operation_markers() {
        let dir = scratch_repo();
        let p = dir.path();

        std::fs::write(marker_path(p, "MERGE_HEAD"), b"deadbeef\n").unwrap();
        assert_eq!(detect_state(p).unwrap().kind, OperationKind::Merge);
        std::fs::remove_file(marker_path(p, "MERGE_HEAD")).unwrap();

        std::fs::create_dir_all(marker_path(p, "rebase-merge")).unwrap();
        assert_eq!(detect_state(p).unwrap().kind, OperationKind::Rebase);
        std::fs::remove_dir_all(marker_path(p, "rebase-merge")).unwrap();

        std::fs::write(marker_path(p, "CHERRY_PICK_HEAD"), b"deadbeef\n").unwrap();
        assert_eq!(detect_state(p).unwrap().kind, OperationKind::CherryPick);
        std::fs::remove_file(marker_path(p, "CHERRY_PICK_HEAD")).unwrap();

        std::fs::write(marker_path(p, "REVERT_HEAD"), b"deadbeef\n").unwrap();
        assert_eq!(detect_state(p).unwrap().kind, OperationKind::Revert);
    }

    /// In a linked worktree `.git` is a file and the markers live under
    /// `.git/worktrees/<name>/` — a concatenated `repo/.git/MERGE_HEAD` would miss
    /// them and report a calm repository.
    #[test]
    fn detect_state_sees_markers_in_a_linked_worktree() {
        let dir = scratch_repo();
        let outer = tempfile::tempdir().unwrap();
        let linked = outer.path().join("linked");
        git(
            dir.path(),
            &["worktree", "add", "-b", "wt", linked.to_str().unwrap()],
        );
        assert!(linked.join(".git").is_file(), "linked worktree has a .git file");

        assert_eq!(detect_state(&linked).unwrap().kind, OperationKind::None);

        let marker = marker_path(&linked, "MERGE_HEAD");
        assert!(
            marker.starts_with(
                dir.path()
                    .canonicalize()
                    .unwrap()
                    .join(".git")
                    .join("worktrees")
            ),
            "marker lives in the linked worktree's git dir: {}",
            marker.display()
        );
        std::fs::write(&marker, b"deadbeef\n").unwrap();
        assert_eq!(detect_state(&linked).unwrap().kind, OperationKind::Merge);
    }

    /// Outside a repository the state is unknown — that is an error, not "calm".
    #[test]
    fn detect_state_outside_a_repository_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(detect_state(dir.path()).is_err());
    }

    /// Manifest G02 / История 57: four reset modes, unknown ones rejected.
    #[test]
    fn reset_accepts_four_modes_and_rejects_others() {
        for mode in ["soft", "mixed", "hard", "keep"] {
            assert!(reset_mode_flag(mode).is_ok(), "{mode} is a git reset mode");
        }
        match reset_mode_flag("nuke") {
            Err(Error::Rule(m)) => assert!(m.contains("nuke"), "{m}"),
            other => panic!("expected a rule error, got {other:?}"),
        }
        // the mode is checked before anything else, even while reset is a stub
        let dir = scratch_repo();
        match reset(dir.path(), "HEAD", "nuke") {
            Err(Error::Rule(m)) => assert!(m.contains("unknown reset mode"), "{m}"),
            other => panic!("expected the mode to be rejected, got {other:?}"),
        }
    }
}
