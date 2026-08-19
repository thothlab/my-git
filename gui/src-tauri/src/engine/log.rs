//! Reading history and laying out graph lanes.
//!
//! Owns the `git log` format, the parse of `%D` and the streaming lane algorithm;
//! the frontend sees only [`LogPage`]. Output is parsed **by NUL separators**
//! (`%x00` between fields, `%x01` between records) — a commit subject may contain
//! anything, spaces included.
//!
//! Filled in by task 03; the surface is fixed here so the rest of the panel can be
//! built against it.

use std::path::Path;

use crate::error::{Error, Result};
use crate::model::{LogCursor, LogFilter, LogPage};

fn todo_log() -> Error {
    Error::Rule("log: not implemented".into())
}

/// One page of history, continuing the graph from `cursor` when given.
pub fn page(_repo: &Path, _filter: &LogFilter, _cursor: Option<&LogCursor>, _limit: u32) -> Result<LogPage> {
    // TODO(prd): task 03 — git log --format=…%x00…%x01, lane layout, cursor.
    Err(todo_log())
}

/// Distinct commit authors, for the author filter.
pub fn authors(_repo: &Path) -> Result<Vec<String>> {
    // TODO(prd): task 03.
    Err(todo_log())
}
