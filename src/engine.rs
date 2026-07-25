//! Git engine abstraction (ТЗ §2.1).
//!
//! Simple operations (status, diff, stage, commit, log, branch, push, fetch,
//! checkout, reset) go through `gix`/`git2`; `rebase`/`merge`/conflict handling
//! shell out to the system `git` CLI. This module defines the trait boundary and
//! a `gix`-backed implementation; per-operation bodies are filled in over Task 01.

use anyhow::Result;
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
        not_yet!("status")
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
