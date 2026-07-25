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

pub enum Overlay {
    None,
    Help,
    Input(InputState),
    Picker(PickerState),
}

/// Single-line text entry (new/rename changelist; commit message in a later wave).
pub struct InputState {
    pub title: String,
    pub value: String,
    pub purpose: InputPurpose,
}

#[derive(Clone)]
pub enum InputPurpose {
    NewList,
    RenameList(String),
    CommitMessage { files: Vec<String>, amend: bool },
}

/// A choose-one list overlay (move target, active list; branch/reset in later waves).
pub struct PickerState {
    pub title: String,
    pub items: Vec<PickerItem>,
    pub cursor: usize,
    pub purpose: PickerPurpose,
}

pub struct PickerItem {
    pub label: String,
    pub id: String,
}

#[derive(Clone)]
pub enum PickerPurpose {
    SetActive,
    MoveFiles(Vec<String>),
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
        if key.kind == event::KeyEventKind::Release {
            return;
        }
        match self.overlay {
            Overlay::Help => self.overlay = Overlay::None,
            Overlay::Input(_) => self.handle_input_key(key),
            Overlay::Picker(_) => self.handle_picker_key(key),
            Overlay::None => {
                if let Some(action) = resolve(key) {
                    self.on_action(action);
                }
            }
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
            NewList => self.open_new_list(),
            RenameList => self.open_rename_list(),
            DeleteList => self.delete_current_list(),
            SetActive => self.open_set_active(),
            MoveFiles => self.open_move_files(),
            Commit => self.open_commit(false),
            Amend => self.open_commit(true),
            // Remaining git operations land in later waves.
            other => self.message = format!("{}: следующая волна", other.label()),
        }
    }

    // ----- commit by changelist (Task 06) --------------------------------------

    /// Files to commit: the marked subset, else all currently-changed files of
    /// the changelist under the cursor. Staging only these guarantees other
    /// lists (including "Not for commit") are never included.
    fn commit_files(&self) -> Vec<String> {
        if !self.marked.is_empty() {
            return self.marked.iter().cloned().collect();
        }
        match self.current_list() {
            Some(li) => self.store.changelists[li]
                .files
                .iter()
                .filter(|p| self.status_map.contains_key(*p))
                .cloned()
                .collect(),
            None => Vec::new(),
        }
    }

    fn open_commit(&mut self, amend: bool) {
        let files = self.commit_files();
        if files.is_empty() {
            self.message = "nothing to commit in this changelist".into();
            return;
        }
        let mut value = String::new();
        if amend {
            if let Ok(log) = self.engine.log(1) {
                if let Some(c) = log.first() {
                    value = c.summary.clone();
                }
            }
        }
        let title = if amend {
            format!("Amend last commit — {} file(s)", files.len())
        } else {
            format!("Commit — {} file(s)", files.len())
        };
        self.overlay = Overlay::Input(InputState {
            title,
            value,
            purpose: InputPurpose::CommitMessage { files, amend },
        });
    }

    // ----- changelist operations (Task 05) -------------------------------------

    /// The changelist index for the row under the cursor (header or file).
    fn current_list(&self) -> Option<usize> {
        match self.rows.get(self.cursor) {
            Some(Row::Header { list }) | Some(Row::File { list, .. }) => Some(*list),
            None => None,
        }
    }

    /// Files an operation applies to: the marked set, or the file under the cursor.
    fn action_files(&self) -> Vec<String> {
        if !self.marked.is_empty() {
            self.marked.iter().cloned().collect()
        } else {
            self.selected_path().map(|p| vec![p.to_string()]).unwrap_or_default()
        }
    }

    fn open_new_list(&mut self) {
        self.overlay = Overlay::Input(InputState {
            title: "New changelist".into(),
            value: String::new(),
            purpose: InputPurpose::NewList,
        });
    }

    fn open_rename_list(&mut self) {
        if let Some(li) = self.current_list() {
            let cl = &self.store.changelists[li];
            self.overlay = Overlay::Input(InputState {
                title: format!("Rename '{}'", cl.name),
                value: cl.name.clone(),
                purpose: InputPurpose::RenameList(cl.id.clone()),
            });
        }
    }

    fn delete_current_list(&mut self) {
        if let Some(li) = self.current_list() {
            let id = self.store.changelists[li].id.clone();
            match self.store.delete(&id) {
                Ok(()) => self.commit_store_change("changelist deleted"),
                Err(e) => self.message = e.to_string(),
            }
        }
    }

    fn open_set_active(&mut self) {
        let items = self.picker_items_all_lists();
        self.overlay = Overlay::Picker(PickerState {
            title: "Set active changelist".into(),
            items,
            cursor: 0,
            purpose: PickerPurpose::SetActive,
        });
    }

    fn open_move_files(&mut self) {
        let files = self.action_files();
        if files.is_empty() {
            self.message = "no file selected".into();
            return;
        }
        let items = self.picker_items_all_lists();
        self.overlay = Overlay::Picker(PickerState {
            title: format!("Move {} file(s) to…", files.len()),
            items,
            cursor: 0,
            purpose: PickerPurpose::MoveFiles(files),
        });
    }

    fn picker_items_all_lists(&self) -> Vec<PickerItem> {
        self.store
            .changelists
            .iter()
            .map(|c| PickerItem { label: c.name.clone(), id: c.id.clone() })
            .collect()
    }

    /// Persist the store and rebuild the view (working tree is unchanged).
    fn commit_store_change(&mut self, msg: &str) {
        let _ = self.store.persist(&self.store_path);
        self.rebuild_rows();
        self.update_diff();
        self.message = msg.into();
    }

    fn handle_input_key(&mut self, key: event::KeyEvent) {
        use event::KeyCode;
        match key.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Enter => self.submit_input(),
            KeyCode::Backspace => {
                if let Overlay::Input(s) = &mut self.overlay {
                    s.value.pop();
                }
            }
            KeyCode::Char(c) => {
                if let Overlay::Input(s) = &mut self.overlay {
                    s.value.push(c);
                }
            }
            _ => {}
        }
    }

    fn submit_input(&mut self) {
        let Overlay::Input(s) = &self.overlay else { return };
        let value = s.value.trim().to_string();
        let purpose = s.purpose.clone();
        // On rejection the overlay stays open so the user can correct the value.
        match purpose {
            InputPurpose::NewList => {
                if value.is_empty() {
                    self.message = "name must not be empty".into();
                    return;
                }
                match self.store.create(&value) {
                    Ok(id) => {
                        let _ = self.store.set_active(&id);
                        self.overlay = Overlay::None;
                        self.commit_store_change("changelist created");
                    }
                    Err(e) => self.message = e.to_string(),
                }
            }
            InputPurpose::RenameList(id) => {
                if value.is_empty() {
                    self.message = "name must not be empty".into();
                    return;
                }
                match self.store.rename(&id, &value) {
                    Ok(()) => {
                        self.overlay = Overlay::None;
                        self.commit_store_change("changelist renamed");
                    }
                    Err(e) => self.message = e.to_string(),
                }
            }
            InputPurpose::CommitMessage { files, amend } => {
                if value.is_empty() {
                    self.message = "commit message required".into();
                    return;
                }
                match self.engine.commit(&files, &value, amend) {
                    Ok(_hash) => {
                        // Only the committed files leave their list; files in
                        // other lists (incl. "Not for commit") are untouched.
                        self.store.remove_files(&files);
                        self.overlay = Overlay::None;
                        self.marked.clear();
                        self.refresh();
                        self.message = if amend { "amended".into() } else { "committed".into() };
                    }
                    Err(e) => self.message = e.to_string(),
                }
            }
        }
    }

    fn handle_picker_key(&mut self, key: event::KeyEvent) {
        use event::KeyCode;
        match key.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Up | KeyCode::Char('k') => {
                if let Overlay::Picker(p) = &mut self.overlay {
                    p.cursor = p.cursor.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Overlay::Picker(p) = &mut self.overlay {
                    if p.cursor + 1 < p.items.len() {
                        p.cursor += 1;
                    }
                }
            }
            KeyCode::Enter => self.submit_picker(),
            _ => {}
        }
    }

    fn submit_picker(&mut self) {
        let Overlay::Picker(p) = &self.overlay else { return };
        let Some(item) = p.items.get(p.cursor) else {
            self.overlay = Overlay::None;
            return;
        };
        let id = item.id.clone();
        let purpose = p.purpose.clone();
        self.overlay = Overlay::None;
        match purpose {
            PickerPurpose::SetActive => {
                let _ = self.store.set_active(&id);
                self.commit_store_change("active changelist set");
            }
            PickerPurpose::MoveFiles(paths) => match self.store.move_files(&paths, &id) {
                Ok(()) => {
                    self.marked.clear();
                    self.commit_store_change("files moved");
                }
                Err(e) => self.message = e.to_string(),
            },
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
        assert!(matches!(app.overlay, Overlay::Help));
    }

    #[test]
    fn changelist_ops_create_move_and_persist() {
        let mock = Mock { root: std::env::temp_dir().join("mygit-app-test-ops") };
        let mut app = App::new(&mock);
        // create "WIP" (becomes active)
        app.on_action(Action::NewList);
        for c in "WIP".chars() {
            app.handle_key(key_char(c));
        }
        app.handle_key(key(event::KeyCode::Enter));
        assert!(app.store.changelists.iter().any(|c| c.name == "WIP"));
        assert!(matches!(app.overlay, Overlay::None));

        // move a.rs into WIP: select the file, mark it, open move picker, choose WIP
        app.cursor = 1; // a.rs under Default
        app.update_diff();
        app.on_action(Action::Mark);
        app.on_action(Action::MoveFiles);
        assert!(matches!(app.overlay, Overlay::Picker(_)));
        // cursor to WIP entry
        if let Overlay::Picker(p) = &app.overlay {
            let wip_idx = p.items.iter().position(|i| i.label == "WIP").unwrap();
            for _ in 0..wip_idx {
                app.handle_key(key(event::KeyCode::Down));
            }
        }
        app.handle_key(key(event::KeyCode::Enter));
        let wip = app.store.changelists.iter().find(|c| c.name == "WIP").unwrap();
        assert!(wip.files.iter().any(|f| f == "a.rs"), "a.rs should be in WIP");
    }

    fn key(code: event::KeyCode) -> event::KeyEvent {
        event::KeyEvent::new(code, event::KeyModifiers::NONE)
    }
    fn key_char(c: char) -> event::KeyEvent {
        key(event::KeyCode::Char(c))
    }

    fn init_repo(tag: &str) -> std::path::PathBuf {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("mygit-tui-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |a: &[&str]| {
            assert!(Command::new("git").current_dir(&dir).args(a).output().unwrap().status.success());
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        dir
    }

    #[test]
    fn commit_default_excludes_not_for_commit() {
        use crate::engine::GixEngine;
        let dir = init_repo("commit");
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        let engine = GixEngine::discover(&dir).unwrap();
        let mut app = App::new(&engine); // a.txt, b.txt -> Default

        let nfc = app.store.create("Not for commit").unwrap();
        app.store.move_files(&["b.txt".to_string()], &nfc).unwrap();
        let _ = app.store.persist(&app.store_path);
        app.rebuild_rows();

        // Commit the Default list (cursor on its header, no marks).
        app.cursor = 0;
        app.on_action(Action::Commit);
        assert!(matches!(app.overlay, Overlay::Input(_)));
        for c in "add a".chars() {
            app.handle_key(key_char(c));
        }
        app.handle_key(key(event::KeyCode::Enter));

        let log = engine.log(10).unwrap();
        assert_eq!(log.len(), 1, "exactly one commit expected");
        let changed = engine.status().unwrap();
        assert!(!changed.iter().any(|f| f.path == "a.txt"), "a.txt should be committed");
        assert!(changed.iter().any(|f| f.path == "b.txt"), "b.txt must remain changed");
        let nfc_list = app.store.changelists.iter().find(|c| c.name == "Not for commit").unwrap();
        assert!(nfc_list.files.iter().any(|f| f == "b.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_requires_nonempty_message() {
        use crate::engine::GixEngine;
        let dir = init_repo("emptymsg");
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        let engine = GixEngine::discover(&dir).unwrap();
        let mut app = App::new(&engine);
        app.cursor = 0;
        app.on_action(Action::Commit);
        app.handle_key(key(event::KeyCode::Enter)); // empty message
        assert!(matches!(app.overlay, Overlay::Input(_)), "overlay stays open on empty message");
        assert_eq!(engine.log(10).unwrap().len(), 0, "no commit created");
        let _ = std::fs::remove_dir_all(&dir);
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
