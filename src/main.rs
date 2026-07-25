//! mygit — TUI git manager with named changelists.
//!
//! Entry point: enforce the repository precondition (ТЗ §5 / AC#1), then launch
//! the TUI. Running outside a git repository prints a clear error and a hint and
//! exits non-zero without opening the TUI.

mod changelists;
mod engine;
mod tui;

use engine::{GitEngine, GixEngine};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("mygit: cannot determine current directory: {err}");
            return ExitCode::FAILURE;
        }
    };

    let engine = match GixEngine::discover(&cwd) {
        Ok(engine) => engine,
        Err(_) => {
            eprintln!("mygit: not a git repository (or any parent up to the filesystem root).");
            eprintln!("hint: run mygit from inside a git repository, or `git init` here first.");
            return ExitCode::FAILURE;
        }
    };

    // Live pipeline: load the changelist store, reconcile it with the real
    // working tree, and persist the reconciled assignments (ТЗ §6.2).
    let store_path = changelists::store_path(engine.repo_root());
    let mut store = changelists::ChangelistStore::load(&store_path).unwrap_or_default();
    if let Ok(changed) = engine.status() {
        store.sync(&changed);
        let _ = store.persist(&store_path); // best-effort; a write race is tolerated
    }

    if let Err(err) = tui::run(engine.repo_root(), &store) {
        eprintln!("mygit: {err:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
