//! The branch tree and operations on branches.
//!
//! Hides `for-each-ref`, the parse of `%(upstream:track)` and the grouping order.
//! Filled in by task 05.

use std::path::Path;

use crate::error::{Error, Result};
use crate::model::BranchNode;

fn todo_branches() -> Error {
    Error::Rule("branches: not implemented".into())
}

/// Local and remote-tracking branches with ahead/behind, ready for the tree.
pub fn tree(_repo: &Path) -> Result<Vec<BranchNode>> {
    // TODO(prd): task 05 — for-each-ref with %00-separated fields.
    Err(todo_branches())
}

pub fn rename(_repo: &Path, _from: &str, _to: &str) -> Result<()> {
    // TODO(prd): task 05.
    Err(todo_branches())
}

pub fn delete(_repo: &Path, _name: &str, _remote: bool, _force: bool) -> Result<()> {
    // TODO(prd): task 05.
    Err(todo_branches())
}

pub fn merge(_repo: &Path, _name: &str) -> Result<()> {
    // TODO(prd): task 05.
    Err(todo_branches())
}

pub fn rebase_onto(_repo: &Path, _name: &str) -> Result<()> {
    // TODO(prd): task 05.
    Err(todo_branches())
}
