//! TUI shell: the `App` orchestrator + event loop. Following the gwm-cli lesson,
//! `App` owns side effects and composes smaller concerns (theme, keymap, and —
//! in later waves — overlay state). Rendering lives in `ui`.

mod keymap;
mod theme;
mod ui;

use crate::changelists::{store_path, ChangelistStore};
use crate::engine::{BranchState, FileStatus, GitEngine};
use anyhow::Result;
use crossterm::event::{self, Event};
use keymap::{resolve, Action};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use theme::Theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Changes,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help,
}

/// A rendered row in the Changes panel: a changelist header or a file under it.
pub enum Row {
    Header { list: usize },
    File { list: usize, path: String, status: FileStatus },
}

pub struct App<'e> {
    engine: &'e dyn GitEngine,
    store_path: PathBuf,
    store: ChangelistStore,
    status_map: HashMap<String, FileStatus>,
    branch: BranchState,
    theme: Theme,

    rows: Vec<Row>,
    cursor: usize,
    marked: BTreeSet<String>,
    focus: Focus,
    overlay: Overlay,

    diff: String,
    diff_path: Option<String>,
    diff_scroll: u16,

    message: String,
    quit: bool,
}

/// Build the app (runs the startup pipeline) and drive it until the user quits.
pub fn run(engine: &dyn GitEngine) -> Result<()> {
    let mut app = App::new(engine);
    let mut term = ratatui::init();
    let result = app.event_loop(&mut term);
    ratatui::restore();
    result
}

impl<'e> App<'e> {
    fn new(engine: &'e dyn GitEngine) -> Self {
        let store_path = store_path(engine.repo_root());
        let store = ChangelistStore::load(&store_path).unwrap_or_default();
        let mut app = App {
            engine,
            store_path,
            store,
            status_map: HashMap::new(),
            branch: BranchState::default(),
            theme: Theme::default(),
            rows: Vec::new(),
            cursor: 0,
            marked: BTreeSet::new(),
            focus: Focus::Changes,
            overlay: Overlay::None,
            diff: String::new(),
            diff_path: None,
            diff_scroll: 0,
            message: String::new(),
            quit: false,
        };
        app.refresh();
        app
    }

    /// Reconcile with the real working tree (ТЗ §6.2), refresh branch state, and
    /// rebuild the view. Called on start, on refresh, and after operations.
    fn refresh(&mut self) {
        match self.engine.status() {
            Ok(changed) => {
                self.store.sync(&changed);
                let _ = self.store.persist(&self.store_path);
                self.status_map = changed.into_iter().map(|f| (f.path, f.status)).collect();
            }
            Err(e) => self.message = format!("git status failed: {e}"),
        }
        self.branch = self.engine.branch_state().unwrap_or_default();
        self.rebuild_rows();
        self.diff_path = None; // force diff recompute for the current selection
        self.update_diff();
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        for (i, cl) in self.store.changelists.iter().enumerate() {
            rows.push(Row::Header { list: i });
            for path in &cl.files {
                if let Some(&status) = self.status_map.get(path) {
                    rows.push(Row::File { list: i, path: path.clone(), status });
                }
            }
        }
        self.rows = rows;
        if self.cursor >= self.rows.len() {
            self.cursor = self.rows.len().saturating_sub(1);
        }
        self.marked.retain(|p| self.status_map.contains_key(p));
    }

    fn selected_path(&self) -> Option<&str> {
        match self.rows.get(self.cursor) {
            Some(Row::File { path, .. }) => Some(path.as_str()),
            _ => None,
        }
    }

    fn update_diff(&mut self) {
        let path = self.selected_path().map(str::to_string);
        if path == self.diff_path {
            return;
        }
        self.diff = match &path {
            Some(p) => self
                .engine
                .diff(p)
                .unwrap_or_else(|e| format!("(diff unavailable: {e})")),
            None => String::new(),
        };
        self.diff_path = path;
        self.diff_scroll = 0;
    }

    fn event_loop(&mut self, term: &mut ratatui::DefaultTerminal) -> Result<()> {
        while !self.quit {
            term.draw(|f| ui::render(f, self))?;
            if let Event::Key(key) = event::read()? {
                self.handle_key(key);
            }
        }
        Ok(())
    }

    fn handle_key(&mut self, key: event::KeyEvent) {
        if self.overlay == Overlay::Help {
            if key.kind != event::KeyEventKind::Release {
                self.overlay = Overlay::None;
            }
            return;
        }
        if let Some(action) = resolve(key) {
            self.on_action(action);
        }
    }

    fn on_action(&mut self, action: Action) {
        use Action::*;
        self.message.clear();
        match action {
            Quit => self.quit = true,
            Down => self.move_down(),
            Up => self.move_up(),
            ToggleFocus => {
                self.focus = match self.focus {
                    Focus::Changes => Focus::Detail,
                    Focus::Detail => Focus::Changes,
                }
            }
            Mark => self.toggle_mark(),
            Refresh => {
                self.refresh();
                self.message = "refreshed".into();
            }
            Help => self.overlay = Overlay::Help,
            // Operations land in later waves; announce so the key is discoverable.
            other => self.message = format!("{}: следующая волна", other.label()),
        }
    }

    fn move_down(&mut self) {
        if self.focus == Focus::Detail {
            self.diff_scroll = self.diff_scroll.saturating_add(1);
            return;
        }
        if !self.rows.is_empty() && self.cursor + 1 < self.rows.len() {
            self.cursor += 1;
            self.update_diff();
        }
    }

    fn move_up(&mut self) {
        if self.focus == Focus::Detail {
            self.diff_scroll = self.diff_scroll.saturating_sub(1);
            return;
        }
        if self.cursor > 0 {
            self.cursor -= 1;
            self.update_diff();
        }
    }

    fn toggle_mark(&mut self) {
        if let Some(p) = self.selected_path().map(str::to_string) {
            if !self.marked.remove(&p) {
                self.marked.insert(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{ChangedFile, Commit, PushOpts, ResetMode};
    use std::path::Path;

    /// A canned engine so the App logic can be tested without a terminal.
    struct Mock {
        root: std::path::PathBuf,
    }
    impl GitEngine for Mock {
        fn status(&self) -> Result<Vec<ChangedFile>> {
            Ok(vec![
                ChangedFile { path: "a.rs".into(), status: FileStatus::Modified },
                ChangedFile { path: "b.rs".into(), status: FileStatus::Untracked },
            ])
        }
        fn diff(&self, path: &str) -> Result<String> {
            Ok(format!("+++ {path}\n@@ -1 +1 @@\n+changed"))
        }
        fn branch_state(&self) -> Result<BranchState> {
            Ok(BranchState { current_branch: Some("main".into()), ..Default::default() })
        }
        fn stage(&self, _: &[String]) -> Result<()> { Ok(()) }
        fn commit(&self, _: &[String], _: &str, _: bool) -> Result<String> { Ok("x".into()) }
        fn log(&self, _: usize) -> Result<Vec<Commit>> { Ok(vec![]) }
        fn revert(&self, _: &str) -> Result<()> { Ok(()) }
        fn reset(&self, _: &str, _: ResetMode) -> Result<()> { Ok(()) }
        fn checkout_file(&self, _: &str) -> Result<()> { Ok(()) }
        fn branches(&self) -> Result<Vec<String>> { Ok(vec![]) }
        fn checkout_branch(&self, _: &str) -> Result<()> { Ok(()) }
        fn create_branch(&self, _: &str, _: &str) -> Result<()> { Ok(()) }
        fn push(&self, _: &str, _: &PushOpts) -> Result<()> { Ok(()) }
        fn fetch(&self) -> Result<()> { Ok(()) }
        fn pull(&self) -> Result<()> { Ok(()) }
        fn rebase_onto(&self, _: &str) -> Result<()> { Ok(()) }
        fn rebase_continue(&self) -> Result<()> { Ok(()) }
        fn rebase_skip(&self) -> Result<()> { Ok(()) }
        fn rebase_abort(&self) -> Result<()> { Ok(()) }
        fn conflicts(&self) -> Result<Vec<String>> { Ok(vec![]) }
        fn repo_root(&self) -> &Path { &self.root }
    }

    #[test]
    fn builds_grouped_rows_navigates_and_marks() {
        let mock = Mock { root: std::env::temp_dir().join("mygit-app-test-norepo") };
        let mut app = App::new(&mock);

        // Default header + a.rs + b.rs, both synced into the active Default list.
        assert_eq!(app.rows.len(), 3);
        assert!(matches!(app.rows[0], Row::Header { .. }));

        app.on_action(Action::Down); // onto a.rs
        assert_eq!(app.selected_path(), Some("a.rs"));
        assert!(app.diff.contains("a.rs"));

        app.on_action(Action::Mark);
        assert!(app.marked.contains("a.rs"));

        app.on_action(Action::Help);
        assert_eq!(app.overlay, Overlay::Help);
    }

    #[test]
    fn renders_frame_with_panels_and_content() {
        use ratatui::{backend::TestBackend, Terminal};
        let mock = Mock { root: std::env::temp_dir().join("mygit-app-test-render") };
        let mut app = App::new(&mock);
        app.on_action(Action::Down); // select a.rs so the diff title shows it

        let mut terminal = Terminal::new(TestBackend::new(90, 20)).unwrap();
        terminal.draw(|f| super::ui::render(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();

        for needle in ["CHANGES", "DIFF", "main", "a.rs", "active:", "Default"] {
            assert!(text.contains(needle), "rendered frame missing {needle:?}");
        }
    }

    #[test]
    fn renders_empty_state_when_no_changes() {
        use ratatui::{backend::TestBackend, Terminal};
        struct Clean {
            root: std::path::PathBuf,
        }
        impl GitEngine for Clean {
            fn status(&self) -> Result<Vec<ChangedFile>> { Ok(vec![]) }
            fn diff(&self, _: &str) -> Result<String> { Ok(String::new()) }
            fn branch_state(&self) -> Result<BranchState> {
                Ok(BranchState { current_branch: Some("main".into()), ..Default::default() })
            }
            fn stage(&self, _: &[String]) -> Result<()> { Ok(()) }
            fn commit(&self, _: &[String], _: &str, _: bool) -> Result<String> { Ok("x".into()) }
            fn log(&self, _: usize) -> Result<Vec<Commit>> { Ok(vec![]) }
            fn revert(&self, _: &str) -> Result<()> { Ok(()) }
            fn reset(&self, _: &str, _: ResetMode) -> Result<()> { Ok(()) }
            fn checkout_file(&self, _: &str) -> Result<()> { Ok(()) }
            fn branches(&self) -> Result<Vec<String>> { Ok(vec![]) }
            fn checkout_branch(&self, _: &str) -> Result<()> { Ok(()) }
            fn create_branch(&self, _: &str, _: &str) -> Result<()> { Ok(()) }
            fn push(&self, _: &str, _: &PushOpts) -> Result<()> { Ok(()) }
            fn fetch(&self) -> Result<()> { Ok(()) }
            fn pull(&self) -> Result<()> { Ok(()) }
            fn rebase_onto(&self, _: &str) -> Result<()> { Ok(()) }
            fn rebase_continue(&self) -> Result<()> { Ok(()) }
            fn rebase_skip(&self) -> Result<()> { Ok(()) }
            fn rebase_abort(&self) -> Result<()> { Ok(()) }
            fn conflicts(&self) -> Result<Vec<String>> { Ok(vec![]) }
            fn repo_root(&self) -> &std::path::Path { &self.root }
        }
        let clean = Clean { root: std::env::temp_dir().join("mygit-app-test-clean") };
        let app = App::new(&clean);
        let mut terminal = Terminal::new(TestBackend::new(90, 12)).unwrap();
        terminal.draw(|f| super::ui::render(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("No changes"), "expected empty state, got: {text}");
    }
}
