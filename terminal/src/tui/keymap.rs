//! Keymap — a single `Action` enum plus a default key→action resolver
//! (declarative-table spirit of gwm-cli's `define_actions!`). Bindings are
//! single keys (no chords) so the event loop stays a pure state machine with no
//! Vim-style timeout (gwm lesson). `Ctrl+C` is a hardcoded quit escape hatch.
//!
//! The base bindings follow ТЗ §4. A user-override layer is a future extension
//! documented in the design deliverable.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    // navigation / global
    Quit,
    Up,
    Down,
    ToggleFocus,
    Mark,
    Refresh,
    Help,
    // changelist operations
    NewList,
    RenameList,
    DeleteList,
    MoveFiles,
    // git operations
    Commit,
    Amend,
    Rollback,
    Push,
    Fetch,
    Branches,
    Log,
    Rebase,
    Stashes,
    Commands,
    // overlay control
    Confirm,
    Cancel,
}

impl Action {
    /// Short human label for the help overlay.
    pub fn label(self) -> &'static str {
        use Action::*;
        match self {
            Quit => "quit",
            Up => "up",
            Down => "down",
            ToggleFocus => "switch panel focus",
            Mark => "mark / unmark file",
            Refresh => "refresh",
            Help => "help",
            NewList => "new changelist",
            RenameList => "rename changelist",
            DeleteList => "delete changelist",
            MoveFiles => "move file(s) to changelist",
            Commit => "commit changelist",
            Amend => "amend last commit",
            Rollback => "rollback file to HEAD",
            Push => "push (new -u / update)",
            Fetch => "fetch / pull",
            Branches => "branches (checkout/create)",
            Log => "log browser",
            Rebase => "rebase onto",
            Stashes => "stashes (create / apply / pop / drop)",
            Commands => "git command log (errors / history)",
            Confirm => "confirm",
            Cancel => "cancel",
        }
    }
}

/// The base bindings, in help-display order: (key hint, action).
pub const BINDINGS: &[(&str, Action)] = &[
    ("j/↓", Action::Down),
    ("k/↑", Action::Up),
    ("Tab", Action::ToggleFocus),
    ("space", Action::Mark),
    ("n", Action::NewList),
    ("r", Action::RenameList),
    ("d", Action::DeleteList),
    ("m", Action::MoveFiles),
    ("c", Action::Commit),
    ("A", Action::Amend),
    ("u", Action::Rollback),
    ("P", Action::Push),
    ("F", Action::Fetch),
    ("B", Action::Branches),
    ("L", Action::Log),
    ("R", Action::Rebase),
    ("S", Action::Stashes),
    ("g", Action::Commands),
    ("F5 / ^R", Action::Refresh),
    ("?", Action::Help),
    ("q", Action::Quit),
];

/// Resolve a key press to an action. Returns `None` for unbound keys and for
/// non-press events (key repeat/release on some platforms).
pub fn resolve(key: KeyEvent) -> Option<Action> {
    use Action::*;
    if key.kind == KeyEventKind::Release {
        return None;
    }
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Some(Quit);
    }
    // Ctrl-R is a reliable refresh (F5 is intercepted by macOS by default).
    if key.code == KeyCode::Char('r') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Some(Refresh);
    }
    let action = match key.code {
        KeyCode::Char('q') => Quit,
        KeyCode::Char('j') | KeyCode::Down => Down,
        KeyCode::Char('k') | KeyCode::Up => Up,
        KeyCode::Tab => ToggleFocus,
        KeyCode::Char(' ') => Mark,
        KeyCode::F(5) => Refresh,
        KeyCode::Char('?') => Help,
        KeyCode::Char('n') => NewList,
        KeyCode::Char('r') => RenameList,
        KeyCode::Char('d') => DeleteList,
        KeyCode::Char('m') => MoveFiles,
        KeyCode::Char('c') => Commit,
        KeyCode::Char('A') => Amend,
        KeyCode::Char('u') => Rollback,
        KeyCode::Char('P') => Push,
        KeyCode::Char('F') => Fetch,
        KeyCode::Char('B') => Branches,
        KeyCode::Char('L') => Log,
        KeyCode::Char('R') => Rebase,
        KeyCode::Char('S') => Stashes,
        KeyCode::Char('g') => Commands,
        KeyCode::Enter => Confirm,
        KeyCode::Esc => Cancel,
        _ => return None,
    };
    Some(action)
}
