use crate::error::Result;
use crate::model::RepoSnapshot;

pub mod branches;
pub mod cli;
pub mod commit;
pub mod log;
pub mod ops;

/// Abstraction over the git backend.
///
/// The MVP ships a single implementation — [`cli::CliEngine`], which shells out to
/// the system `git`. ТЗ §2.1 explicitly endorses shell-out (and warns against
/// assuming a Rust git library covers everything). The trait exists so the frontend
/// never depends on *which* implementation serves a call, leaving room for a
/// library-backed engine later. The trait grows one method group per feature task.
pub trait GitEngine {
    /// Working-tree snapshot: branch, upstream, ahead/behind, and changed files.
    fn snapshot(&self) -> Result<RepoSnapshot>;
}
