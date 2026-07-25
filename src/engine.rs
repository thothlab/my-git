//! Git engine abstraction (ТЗ §2.1).
//!
//! Simple operations (status, diff, stage, commit, log, branch, push, fetch,
//! checkout, reset) go through `gix`/`git2`; `rebase`/`merge`/conflict handling
//! shell out to the system `git` CLI. This module defines the trait boundary and
//! a `gix`-backed implementation; per-operation bodies are filled in over Task 01.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub path: String,
    pub status: FileStatus,
}

#[derive(Debug, Clone)]
pub struct Commit {
    pub hash: String,
    pub summary: String,
    pub author: String,
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

#[derive(Debug, Clone, Default)]
pub struct PushOpts {
    pub set_upstream: bool,
    pub force: bool,
    pub force_with_lease: bool,
}

#[derive(Debug, Clone)]
pub struct RebaseState {
    pub current: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Default)]
pub struct BranchState {
    pub current_branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub rebase: Option<RebaseState>,
    pub detached: bool,
}

/// The operation surface listed in the PRD "API list". The trait boundary is an
/// implementation detail — callers depend on observable behaviour, not on which
/// backend (gix/git2 vs shell-out) services a given call.
pub trait GitEngine {
    fn status(&self) -> Result<Vec<ChangedFile>>;
    fn diff(&self, path: &str) -> Result<String>;
    fn stage(&self, paths: &[String]) -> Result<()>;
    fn commit(&self, paths: &[String], message: &str, amend: bool) -> Result<String>;
    fn log(&self, limit: usize) -> Result<Vec<Commit>>;
    fn revert(&self, hash: &str) -> Result<()>;
    fn reset(&self, hash: &str, mode: ResetMode) -> Result<()>;
    fn checkout_file(&self, path: &str) -> Result<()>;
    fn branch_state(&self) -> Result<BranchState>;
    fn checkout_branch(&self, name: &str) -> Result<()>;
    fn create_branch(&self, name: &str, from: &str) -> Result<()>;
    fn push(&self, branch: &str, opts: &PushOpts) -> Result<()>;
    fn fetch(&self) -> Result<()>;
    fn pull(&self) -> Result<()>;
    fn rebase_onto(&self, target: &str) -> Result<()>;
    fn rebase_continue(&self) -> Result<()>;
    fn rebase_skip(&self) -> Result<()>;
    fn rebase_abort(&self) -> Result<()>;
    fn repo_root(&self) -> &Path;
}

/// `gix`-backed engine. Holds the discovered repository; simple ops will be
/// implemented against it, rebase/merge against shell-out, over Task 01.
pub struct GixEngine {
    #[allow(dead_code)]
    repo: gix::Repository,
    root: PathBuf,
}

impl GixEngine {
    /// Discover the repository containing `dir`. Returns an error when `dir` is
    /// not inside a git repository — the caller turns this into the AC#1
    /// non-repo message.
    pub fn discover(dir: &Path) -> Result<Self> {
        let repo = gix::discover(dir)?;
        let root = repo
            .work_dir()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| repo.git_dir().to_path_buf());
        Ok(Self { repo, root })
    }
}

macro_rules! not_yet {
    ($op:literal) => {
        anyhow::bail!(concat!("not yet implemented: ", $op))
    };
}

impl GitEngine for GixEngine {
    fn status(&self) -> Result<Vec<ChangedFile>> {
        let out = std::process::Command::new("git")
            .current_dir(&self.root)
            .args(["status", "--porcelain", "-z", "--untracked-files=all"])
            .output()
            .context("running `git status`")?;
        anyhow::ensure!(
            out.status.success(),
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(parse_porcelain_z(&out.stdout))
    }
    fn diff(&self, _path: &str) -> Result<String> {
        not_yet!("diff")
    }
    fn stage(&self, _paths: &[String]) -> Result<()> {
        not_yet!("stage")
    }
    fn commit(&self, _paths: &[String], _message: &str, _amend: bool) -> Result<String> {
        not_yet!("commit")
    }
    fn log(&self, _limit: usize) -> Result<Vec<Commit>> {
        not_yet!("log")
    }
    fn revert(&self, _hash: &str) -> Result<()> {
        not_yet!("revert")
    }
    fn reset(&self, _hash: &str, _mode: ResetMode) -> Result<()> {
        not_yet!("reset")
    }
    fn checkout_file(&self, _path: &str) -> Result<()> {
        not_yet!("checkout_file")
    }
    fn branch_state(&self) -> Result<BranchState> {
        not_yet!("branch_state")
    }
    fn checkout_branch(&self, _name: &str) -> Result<()> {
        not_yet!("checkout_branch")
    }
    fn create_branch(&self, _name: &str, _from: &str) -> Result<()> {
        not_yet!("create_branch")
    }
    fn push(&self, _branch: &str, _opts: &PushOpts) -> Result<()> {
        not_yet!("push")
    }
    fn fetch(&self) -> Result<()> {
        not_yet!("fetch")
    }
    fn pull(&self) -> Result<()> {
        not_yet!("pull")
    }
    fn rebase_onto(&self, _target: &str) -> Result<()> {
        not_yet!("rebase_onto")
    }
    fn rebase_continue(&self) -> Result<()> {
        not_yet!("rebase_continue")
    }
    fn rebase_skip(&self) -> Result<()> {
        not_yet!("rebase_skip")
    }
    fn rebase_abort(&self) -> Result<()> {
        not_yet!("rebase_abort")
    }
    fn repo_root(&self) -> &Path {
        &self.root
    }
}

/// Parse `git status --porcelain -z` output into `ChangedFile`s. Paths are
/// repo-relative with `/` separators on every platform (git's own convention),
/// and `-z` leaves them unquoted. Rename/copy entries carry an extra
/// NUL-terminated original path, which is consumed and discarded.
fn parse_porcelain_z(bytes: &[u8]) -> Vec<ChangedFile> {
    let tokens: Vec<&[u8]> = bytes.split(|&b| b == 0).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];
        i += 1;
        if tok.len() < 3 {
            continue; // trailing empty token
        }
        let x = tok[0] as char;
        let y = tok[1] as char;
        let path = String::from_utf8_lossy(&tok[3..]).into_owned();
        if x == 'R' || x == 'C' || y == 'R' || y == 'C' {
            i += 1; // skip the original path of a rename/copy
        }
        out.push(ChangedFile {
            path,
            status: map_status(x, y),
        });
    }
    out
}

/// Collapse a porcelain XY status pair into a single displayable status.
/// Precedence: conflict > untracked > renamed > added > deleted > modified.
fn map_status(x: char, y: char) -> FileStatus {
    let unmerged = matches!(
        (x, y),
        ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D')
    );
    if unmerged {
        FileStatus::Conflicted
    } else if x == '?' && y == '?' {
        FileStatus::Untracked
    } else if x == 'R' || y == 'R' {
        FileStatus::Renamed
    } else if x == 'A' || y == 'A' {
        FileStatus::Added
    } else if x == 'D' || y == 'D' {
        FileStatus::Deleted
    } else {
        FileStatus::Modified
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_statuses() {
        // ` M a.rs\0?? b.txt\0A  c.rs\0 D d.rs\0UU e.rs\0`
        let raw = b" M a.rs\0?? b.txt\0A  c.rs\0 D d.rs\0UU e.rs\0";
        let files = parse_porcelain_z(raw);
        let got: Vec<(&str, FileStatus)> =
            files.iter().map(|f| (f.path.as_str(), f.status)).collect();
        assert_eq!(
            got,
            vec![
                ("a.rs", FileStatus::Modified),
                ("b.txt", FileStatus::Untracked),
                ("c.rs", FileStatus::Added),
                ("d.rs", FileStatus::Deleted),
                ("e.rs", FileStatus::Conflicted),
            ]
        );
    }

    #[test]
    fn rename_consumes_original_path() {
        // `R  new.rs\0old.rs\0 M keep.rs\0` — the old path must not become an entry.
        let raw = b"R  new.rs\0old.rs\0 M keep.rs\0";
        let files = parse_porcelain_z(raw);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "new.rs");
        assert_eq!(files[0].status, FileStatus::Renamed);
        assert_eq!(files[1].path, "keep.rs");
    }

    #[test]
    fn nested_paths_use_forward_slash() {
        let raw = b" M src/ui/panel.rs\0";
        let files = parse_porcelain_z(raw);
        assert_eq!(files[0].path, "src/ui/panel.rs");
    }

    #[test]
    fn status_reports_a_new_file_on_a_real_repo() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("mygit-status-it-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| Command::new("git").current_dir(&dir).args(args).output().unwrap();
        assert!(git(&["init", "-q"]).status.success());
        std::fs::write(dir.join("hello.txt"), b"hi").unwrap();

        let engine = GixEngine::discover(&dir).unwrap();
        let files = engine.status().unwrap();
        assert!(
            files.iter().any(|f| f.path == "hello.txt" && f.status == FileStatus::Untracked),
            "expected hello.txt as Untracked, got {files:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn startup_pipeline_persists_assignments() {
        use crate::changelists::{store_path, ChangelistStore};
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("mygit-pipe-it-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| Command::new("git").current_dir(&dir).args(args).output().unwrap();
        assert!(git(&["init", "-q"]).status.success());
        std::fs::write(dir.join("hello.txt"), b"hi").unwrap();

        // Exactly what main() does at startup.
        let engine = GixEngine::discover(&dir).unwrap();
        let sp = store_path(engine.repo_root());
        let mut store = ChangelistStore::load(&sp).unwrap();
        store.sync(&engine.status().unwrap());
        store.persist(&sp).unwrap();

        assert!(sp.exists(), ".git/changelists.json must be written");
        let reloaded = ChangelistStore::load(&sp).unwrap();
        assert!(
            reloaded.changelists.iter().any(|c| c.files.iter().any(|f| f == "hello.txt")),
            "hello.txt must be assigned and persisted"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
