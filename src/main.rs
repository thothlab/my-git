//! mygit — TUI git manager with named changelists.
//!
//! Entry point: enforce the repository precondition (ТЗ §5 / AC#1), then launch
//! the TUI. Running outside a git repository prints a clear error and a hint and
//! exits non-zero without opening the TUI.

mod changelists;
mod engine;
mod tui;

use engine::GixEngine;
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

    // The App owns the startup pipeline (load store → sync against the real
    // working tree → persist) and the event loop.
    if let Err(err) = tui::run(&engine) {
        eprintln!("mygit: {err:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
