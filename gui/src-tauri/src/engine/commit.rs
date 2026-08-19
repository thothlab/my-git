//! One commit and comparison of two revisions.
//!
//! Hides `git show`, `git diff <a>..<b>` and `git branch --contains`.
//! Filled in by task 04.

use std::path::Path;

use crate::error::{Error, Result};
use crate::model::{CommitDetails, CommitFileEntry, FileDiff};

fn todo_commit() -> Error {
    Error::Rule("commit: not implemented".into())
}

/// Full commit card: author, committer, body, refs, containing branches.
pub fn details(_repo: &Path, _hash: &str) -> Result<CommitDetails> {
    // TODO(prd): task 04.
    Err(todo_commit())
}

/// Files touched by a commit.
pub fn files(_repo: &Path, _hash: &str) -> Result<Vec<CommitFileEntry>> {
    // TODO(prd): task 04.
    Err(todo_commit())
}

/// Diff of one file inside a commit. `ws` is the whitespace mode, same vocabulary
/// as [`crate::engine::cli::CliEngine::diff_file`].
pub fn file_diff(_repo: &Path, _hash: &str, _path: &str, _ws: &str) -> Result<FileDiff> {
    // TODO(prd): task 04.
    Err(todo_commit())
}

/// Files differing between two revisions.
pub fn compare(_repo: &Path, _from: &str, _to: &str) -> Result<Vec<CommitFileEntry>> {
    // TODO(prd): task 04.
    Err(todo_commit())
}

/// Diff of one file between two revisions.
pub fn compare_diff(_repo: &Path, _from: &str, _to: &str, _path: &str, _ws: &str) -> Result<FileDiff> {
    // TODO(prd): task 04.
    Err(todo_commit())
}
