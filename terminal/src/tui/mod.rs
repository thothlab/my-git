//! TUI shell: the `App` orchestrator + event loop. Following the gwm-cli lesson,
//! `App` owns side effects and composes smaller concerns (theme, keymap, and —
//! in later waves — overlay state). Rendering lives in `ui`.

mod keymap;
mod logview;
mod theme;
mod ui;

use crate::changelists::{store_path, ChangelistStore, UNVERSIONED_ID};
use crate::engine::{BranchState, FileStatus, GitEngine, PushOpts, ResetMode};
use anyhow::Result;
use crossterm::event::{self, Event};
use keymap::{resolve, Action};
use logview::{LogAction, LogView};
use ratatui::layout::Rect;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Changes,
    Detail,
}

pub enum Overlay {
    None,
    /// Interactive help / command palette; the `usize` is the selected row.
    Help(usize),
    /// Static key reference for the Log browser (any key closes).
    LogHelp,
    Input(InputState),
    Picker(PickerState),
    Confirm(ConfirmState),
}

/// Destructive-action confirmation. Following the gwm-cli lesson, the default is
/// to cancel: only an explicit `y` proceeds; Esc / any other key cancels.
pub struct ConfirmState {
    pub title: String,
    pub body: String,
    pub purpose: ConfirmPurpose,
}

#[derive(Clone)]
pub enum ConfirmPurpose {
    ResetHard(String),
    RollbackFile(String),
    ForcePush(String),
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
    NewBranch,
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
    MoveFiles(Vec<String>),
    ResetMode(String),
    Checkout,
    RebaseOnto,
    RebaseControl,
}

/// A rendered row in the Changes panel: a changelist header or a file under it.
pub enum Row {
    Header {
        list: usize,
    },
    File {
        list: usize,
        path: String,
        status: FileStatus,
    },
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

    /// When `Some`, the full-screen Git Log browser is active.
    log: Option<LogView>,

    /// Left (Changes) panel width as a percent of the main area; the vertical
    /// divider drag adjusts it.
    split_pct: u16,
    dragging: bool,

    message: String,
    quit: bool,
}

/// Build the app (runs the startup pipeline) and drive it until the user quits.
pub fn run(engine: &dyn GitEngine) -> Result<()> {
    let mut app = App::new(engine);
    let mut term = ratatui::init();
    // Enable mouse reporting so panels respond to clicks and the divider drags.
    let _ = crossterm::execute!(std::io::stdout(), event::EnableMouseCapture);
    let result = app.event_loop(&mut term);
    let _ = crossterm::execute!(std::io::stdout(), event::DisableMouseCapture);
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
            log: None,
            split_pct: 50,
            dragging: false,
            message: String::new(),
            quit: false,
        };
        app.refresh();
        app
    }

    /// Re-scan the working tree (ТЗ §6.2) and branch state, and rebuild the rows.
    /// Persists the store only when reconciliation actually changed it, so idle
    /// auto-refresh ticks don't churn `.git/changelists.json`.
    fn reload_status(&mut self) {
        match self.engine.status() {
            Ok(changed) => {
                if self.store.sync(&changed) {
                    if let Err(e) = self.store.persist(&self.store_path) {
                        self.message = format!("⚠ save failed: {e}");
                    }
                }
                self.status_map = changed.into_iter().map(|f| (f.path, f.status)).collect();
            }
            Err(e) => self.message = format!("git status failed: {e}"),
        }
        self.branch = self.engine.branch_state().unwrap_or_default();
        self.rebuild_rows();
    }

    /// Full manual refresh (F5 / Ctrl-R): re-scan and recompute the diff.
    fn refresh(&mut self) {
        self.reload_status();
        self.diff_path = None; // force diff recompute for the current selection
        self.update_diff();
    }

    /// Periodic background refresh: reflect on-disk edits without disturbing an
    /// open overlay, the log browser, or the diff scroll position.
    fn auto_refresh(&mut self) {
        if !matches!(self.overlay, Overlay::None) || self.log.is_some() {
            return;
        }
        self.reload_status();
        // Recompute the diff for the current selection, keeping the scroll offset.
        if let Some(p) = self.selected_path().map(str::to_string) {
            self.diff = self
                .engine
                .diff(&p)
                .unwrap_or_else(|e| format!("(diff unavailable: {e})"));
            self.diff_path = Some(p);
        } else {
            self.diff.clear();
            self.diff_path = None;
        }
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        for (i, cl) in self.store.changelists.iter().enumerate() {
            rows.push(Row::Header { list: i });
            for path in &cl.files {
                if let Some(&status) = self.status_map.get(path) {
                    rows.push(Row::File {
                        list: i,
                        path: path.clone(),
                        status,
                    });
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
        let auto_every = Duration::from_millis(1500);
        let mut last_auto = Instant::now();
        while !self.quit {
            term.draw(|f| ui::render(f, self))?;
            // Poll with a timeout so the panel can auto-refresh while idle
            // (F5 is unreliable on macOS; this keeps the view live regardless).
            if event::poll(Duration::from_millis(250))? {
                match event::read()? {
                    Event::Key(key) => self.handle_key(key),
                    Event::Mouse(m) => {
                        let size = term.size()?;
                        self.handle_mouse(m, Rect::new(0, 0, size.width, size.height));
                    }
                    _ => {}
                }
            }
            if last_auto.elapsed() >= auto_every {
                self.auto_refresh();
                last_auto = Instant::now();
            }
        }
        Ok(())
    }

    fn handle_key(&mut self, key: event::KeyEvent) {
        if key.kind == event::KeyEventKind::Release {
            return;
        }
        match self.overlay {
            Overlay::Help(_) => self.handle_help_key(key),
            Overlay::LogHelp => self.overlay = Overlay::None, // any key closes
            Overlay::Input(_) => self.handle_input_key(key),
            Overlay::Picker(_) => self.handle_picker_key(key),
            Overlay::Confirm(_) => self.handle_confirm_key(key),
            Overlay::None => {
                if self.log.is_some() {
                    self.handle_log_key_event(key);
                } else if let Some(action) = resolve(key) {
                    self.on_action(action);
                }
            }
        }
    }

    /// Mouse: a click focuses (and, in Changes, selects) a panel, dragging the
    /// divider resizes them, and the wheel scrolls the panel under the pointer.
    fn handle_mouse(&mut self, m: event::MouseEvent, area: Rect) {
        use event::{MouseButton, MouseEventKind};
        if !matches!(self.overlay, Overlay::None) {
            return; // let overlays own the interaction
        }
        if let Some(l) = self.log.as_mut() {
            let body = Rect::new(
                area.x,
                area.y + 1,
                area.width,
                area.height.saturating_sub(2),
            );
            l.handle_mouse(m, body, self.engine);
            return;
        }
        let [_, changes, detail, _] = ui::regions(area, self.split_pct);
        let divider = detail.x as i32;
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if (m.column as i32 - divider).abs() <= 1 {
                    self.dragging = true;
                } else if m.column < detail.x {
                    self.focus = Focus::Changes;
                    self.select_row_at(m.row, changes);
                } else {
                    self.focus = Focus::Detail;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                let total = changes.width + detail.width;
                if total > 0 {
                    let rel = m.column.saturating_sub(changes.x);
                    let pct = (rel as u32 * 100 / total as u32) as u16;
                    self.split_pct = pct.clamp(20, 80);
                }
            }
            MouseEventKind::Up(_) => self.dragging = false,
            MouseEventKind::ScrollDown => self.scroll_at(m.column, detail.x, true),
            MouseEventKind::ScrollUp => self.scroll_at(m.column, detail.x, false),
            _ => {}
        }
    }

    /// Select the Changes row under a click, matching the render viewport.
    fn select_row_at(&mut self, row: u16, changes: Rect) {
        let inner_top = changes.y + 1; // skip the panel's top border
        if row < inner_top {
            return;
        }
        let inner_h = changes.height.saturating_sub(2) as usize;
        let start = self.cursor.saturating_sub(inner_h.saturating_sub(1));
        let idx = start + (row - inner_top) as usize;
        if idx < self.rows.len() {
            self.cursor = idx;
            self.update_diff();
        }
    }

    fn scroll_at(&mut self, col: u16, detail_x: u16, down: bool) {
        if col < detail_x {
            if down {
                if self.cursor + 1 < self.rows.len() {
                    self.cursor += 1;
                    self.update_diff();
                }
            } else if self.cursor > 0 {
                self.cursor -= 1;
                self.update_diff();
            }
        } else if down {
            self.diff_scroll = self.diff_scroll.saturating_add(1);
        } else {
            self.diff_scroll = self.diff_scroll.saturating_sub(1);
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
            Help => self.overlay = Overlay::Help(0),
            NewList => self.open_new_list(),
            RenameList => self.open_rename_list(),
            DeleteList => self.delete_current_list(),
            MoveFiles => self.open_move_files(),
            Commit => self.open_commit(false),
            Amend => self.open_commit(true),
            Log => self.toggle_log_mode(),
            Rollback => self.open_rollback(),
            Push => self.push_action(),
            Fetch => self.fetch_action(),
            Branches => self.open_branches(),
            Rebase => self.open_rebase(),
            Confirm | Cancel => {} // only meaningful inside overlays
        }
    }

    // ----- rebase (Task 09) ----------------------------------------------------

    fn open_rebase(&mut self) {
        if let Some(rb) = self.branch.rebase.clone() {
            // In progress: list conflicts (informational) + control actions.
            let conflicts = self.engine.conflicts().unwrap_or_default();
            let mut items: Vec<PickerItem> = conflicts
                .iter()
                .map(|p| PickerItem {
                    label: format!("⚠ {p}"),
                    id: "noop".into(),
                })
                .collect();
            let first_control = items.len();
            items.push(PickerItem {
                label: "Continue".into(),
                id: "continue".into(),
            });
            items.push(PickerItem {
                label: "Skip".into(),
                id: "skip".into(),
            });
            items.push(PickerItem {
                label: "Abort".into(),
                id: "abort".into(),
            });
            self.overlay = Overlay::Picker(PickerState {
                title: format!("Rebase {}/{} in progress", rb.current, rb.total),
                items,
                cursor: first_control,
                purpose: PickerPurpose::RebaseControl,
            });
        } else {
            let cur = self.branch.current_branch.clone().unwrap_or_default();
            let items: Vec<PickerItem> = self
                .engine
                .branches()
                .unwrap_or_default()
                .into_iter()
                .filter(|b| *b != cur)
                .map(|b| PickerItem {
                    label: b.clone(),
                    id: b,
                })
                .collect();
            if items.is_empty() {
                self.message = "no other branch to rebase onto".into();
                return;
            }
            self.overlay = Overlay::Picker(PickerState {
                title: "Rebase current branch onto…".into(),
                items,
                cursor: 0,
                purpose: PickerPurpose::RebaseOnto,
            });
        }
    }

    fn after_rebase_step(&mut self, result: Result<()>, ok: &str) {
        self.refresh();
        match result {
            Ok(()) => {
                self.message = if self.branch.rebase.is_some() {
                    "still in progress — resolve conflicts, then Continue".into()
                } else {
                    format!("rebase {ok}")
                };
            }
            Err(_) => {
                self.message = if self.branch.rebase.is_some() {
                    "rebase step failed — unresolved conflicts remain".into()
                } else {
                    "rebase step failed".into()
                };
            }
        }
    }

    // ----- branches (Task 08) --------------------------------------------------

    fn push_action(&mut self) {
        let Some(branch) = self.branch.current_branch.clone() else {
            self.message = "no branch to push".into();
            return;
        };
        if self.branch.detached {
            self.message = "detached HEAD; cannot push".into();
            return;
        }
        if self.branch.upstream.is_none() {
            self.do_push(
                &branch,
                PushOpts {
                    set_upstream: true,
                    ..Default::default()
                },
                "pushed (upstream set)",
            );
        } else if self.branch.behind > 0 {
            // Diverged: offer --force-with-lease behind an explicit confirm.
            self.overlay = Overlay::Confirm(ConfirmState {
                title: "Diverged from upstream".into(),
                body: format!(
                    "Local/remote diverged (↑{} ↓{}). Push with --force-with-lease?",
                    self.branch.ahead, self.branch.behind
                ),
                purpose: ConfirmPurpose::ForcePush(branch),
            });
        } else {
            self.do_push(&branch, PushOpts::default(), "pushed");
        }
    }

    fn do_push(&mut self, branch: &str, opts: PushOpts, ok: &str) {
        match self.engine.push(branch, &opts) {
            Ok(()) => {
                self.refresh();
                self.message = ok.into();
            }
            Err(e) => self.message = e.to_string(),
        }
    }

    fn fetch_action(&mut self) {
        match self.engine.fetch() {
            Ok(()) => {
                self.refresh();
                self.message = "fetched".into();
            }
            Err(e) => self.message = e.to_string(),
        }
    }

    fn open_branches(&mut self) {
        let mut items = vec![PickerItem {
            label: "＋ new branch…".into(),
            id: "__new__".into(),
        }];
        for b in self.engine.branches().unwrap_or_default() {
            items.push(PickerItem {
                label: b.clone(),
                id: b,
            });
        }
        self.overlay = Overlay::Picker(PickerState {
            title: "Branches — checkout / create".into(),
            items,
            cursor: 0,
            purpose: PickerPurpose::Checkout,
        });
    }

    // ----- log browser + revert/reset -----------------------------------------

    fn toggle_log_mode(&mut self) {
        if self.log.is_some() {
            self.log = None;
        } else {
            self.log = Some(LogView::new(self.engine));
        }
    }

    /// Route a key to the log browser and act on what it returns.
    fn handle_log_key_event(&mut self, key: event::KeyEvent) {
        let engine = self.engine;
        let action = match self.log.as_mut() {
            Some(l) => l.handle_key(key, engine),
            None => return,
        };
        match action {
            LogAction::None => {}
            LogAction::Exit => self.log = None,
            LogAction::Help => self.overlay = Overlay::LogHelp,
            LogAction::Revert(hash) => match engine.revert(&hash) {
                Ok(()) => {
                    self.reload_log_and_state();
                    self.message = "reverted".into();
                }
                Err(e) => self.message = e.to_string(),
            },
            LogAction::Reset(hash) => self.open_reset_picker(hash),
        }
    }

    /// Refresh changes-mode state and, if the log browser is open, its commits.
    fn reload_log_and_state(&mut self) {
        self.refresh();
        let engine = self.engine;
        if let Some(l) = self.log.as_mut() {
            l.reload_commits(engine);
        }
    }

    fn open_reset_picker(&mut self, hash: String) {
        let items = vec![
            PickerItem {
                label: "soft  (keep index + worktree)".into(),
                id: "soft".into(),
            },
            PickerItem {
                label: "mixed (keep worktree)".into(),
                id: "mixed".into(),
            },
            PickerItem {
                label: "hard  (DISCARD changes)".into(),
                id: "hard".into(),
            },
        ];
        self.overlay = Overlay::Picker(PickerState {
            title: format!("Reset to {}…", &hash[..hash.len().min(8)]),
            items,
            cursor: 0,
            purpose: PickerPurpose::ResetMode(hash),
        });
    }

    fn do_reset(&mut self, hash: &str, mode: ResetMode) {
        match self.engine.reset(hash, mode) {
            Ok(()) => {
                self.reload_log_and_state();
                self.message = "reset done".into();
            }
            Err(e) => self.message = e.to_string(),
        }
    }

    fn open_rollback(&mut self) {
        if let Some(p) = self.selected_path().map(str::to_string) {
            self.overlay = Overlay::Confirm(ConfirmState {
                title: "Rollback file to HEAD".into(),
                body: format!("Discard local changes to {p}? This cannot be undone."),
                purpose: ConfirmPurpose::RollbackFile(p),
            });
        } else {
            self.message = "no file selected".into();
        }
    }

    /// Interactive help / command palette: ↑↓ (or j/k) move, Enter runs the
    /// selected action, Esc/q/? close.
    fn handle_help_key(&mut self, key: event::KeyEvent) {
        use event::KeyCode;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Overlay::Help(c) = &mut self.overlay {
                    *c = c.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Overlay::Help(c) = &mut self.overlay {
                    if *c + 1 < keymap::BINDINGS.len() {
                        *c += 1;
                    }
                }
            }
            KeyCode::Enter => {
                let action = match self.overlay {
                    Overlay::Help(c) => keymap::BINDINGS.get(c).map(|(_, a)| *a),
                    _ => None,
                };
                self.overlay = Overlay::None;
                // Don't let the palette re-open itself.
                if let Some(a) = action {
                    if !matches!(a, Action::Help) {
                        self.on_action(a);
                    }
                }
            }
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                self.overlay = Overlay::None;
            }
            _ => {}
        }
    }

    fn handle_confirm_key(&mut self, key: event::KeyEvent) {
        // Default-cancel: only 'y' proceeds.
        if key.code == event::KeyCode::Char('y') {
            self.execute_confirm();
        } else {
            self.overlay = Overlay::None;
        }
    }

    fn execute_confirm(&mut self) {
        let Overlay::Confirm(c) = &self.overlay else {
            return;
        };
        let purpose = c.purpose.clone();
        self.overlay = Overlay::None;
        match purpose {
            ConfirmPurpose::ResetHard(hash) => self.do_reset(&hash, ResetMode::Hard),
            ConfirmPurpose::RollbackFile(path) => match self.engine.checkout_file(&path) {
                Ok(()) => {
                    self.refresh();
                    self.message = "file rolled back".into();
                }
                Err(e) => self.message = e.to_string(),
            },
            ConfirmPurpose::ForcePush(branch) => self.do_push(
                &branch,
                PushOpts {
                    force_with_lease: true,
                    ..Default::default()
                },
                "force-pushed (with lease)",
            ),
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
            self.selected_path()
                .map(|p| vec![p.to_string()])
                .unwrap_or_default()
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
            if cl.id == UNVERSIONED_ID {
                self.message = "Unversioned Files is managed automatically".into();
                return;
            }
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

    /// Real (movable) lists — excludes the auto-managed Unversioned Files list.
    fn picker_items_all_lists(&self) -> Vec<PickerItem> {
        self.store
            .changelists
            .iter()
            .filter(|c| c.id != UNVERSIONED_ID)
            .map(|c| PickerItem {
                label: c.name.clone(),
                id: c.id.clone(),
            })
            .collect()
    }

    /// Persist the store and rebuild the view (working tree is unchanged). A
    /// write failure is surfaced rather than swallowed (ТЗ §5 resilience).
    fn commit_store_change(&mut self, msg: &str) {
        let persisted = self.store.persist(&self.store_path);
        self.rebuild_rows();
        self.update_diff();
        self.message = match persisted {
            Ok(()) => msg.into(),
            Err(e) => format!("⚠ save failed: {e}"),
        };
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
        let Overlay::Input(s) = &self.overlay else {
            return;
        };
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
                    Ok(_) => {
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
            InputPurpose::NewBranch => {
                if value.is_empty() {
                    self.message = "branch name must not be empty".into();
                    return;
                }
                match self.engine.create_branch(&value, "HEAD") {
                    Ok(()) => {
                        self.overlay = Overlay::None;
                        self.refresh();
                        self.message = format!("created & switched to {value}");
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
                        self.message = if amend {
                            "amended".into()
                        } else {
                            "committed".into()
                        };
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
        let Overlay::Picker(p) = &self.overlay else {
            return;
        };
        let Some(item) = p.items.get(p.cursor) else {
            self.overlay = Overlay::None;
            return;
        };
        let id = item.id.clone();
        let purpose = p.purpose.clone();
        self.overlay = Overlay::None;
        match purpose {
            PickerPurpose::MoveFiles(paths) => match self.store.move_files(&paths, &id) {
                Ok(()) => {
                    self.marked.clear();
                    self.commit_store_change("files moved");
                }
                Err(e) => self.message = e.to_string(),
            },
            PickerPurpose::ResetMode(hash) => match id.as_str() {
                "hard" => {
                    self.overlay = Overlay::Confirm(ConfirmState {
                        title: "reset --hard".into(),
                        body: format!(
                            "Hard-reset to {} and DISCARD all uncommitted changes?",
                            &hash[..hash.len().min(8)]
                        ),
                        purpose: ConfirmPurpose::ResetHard(hash),
                    });
                }
                "soft" => self.do_reset(&hash, ResetMode::Soft),
                _ => self.do_reset(&hash, ResetMode::Mixed),
            },
            PickerPurpose::Checkout => {
                if id == "__new__" {
                    self.overlay = Overlay::Input(InputState {
                        title: "New branch (from HEAD)".into(),
                        value: String::new(),
                        purpose: InputPurpose::NewBranch,
                    });
                } else {
                    match self.engine.checkout_branch(&id) {
                        Ok(()) => {
                            self.refresh();
                            self.message = format!("switched to {id}");
                        }
                        Err(e) => self.message = e.to_string(),
                    }
                }
            }
            PickerPurpose::RebaseOnto => match self.engine.rebase_onto(&id) {
                Ok(()) => {
                    self.refresh();
                    self.message = format!("rebased onto {id}");
                }
                Err(_) => {
                    // git rebase exits non-zero on conflict but leaves the rebase
                    // in progress; reflect that so the user can drive it with R.
                    self.refresh();
                    self.message = if self.branch.rebase.is_some() {
                        "rebase stopped — resolve conflicts, then press R".into()
                    } else {
                        "rebase failed".into()
                    };
                }
            },
            PickerPurpose::RebaseControl => match id.as_str() {
                "continue" => {
                    let r = self.engine.rebase_continue();
                    self.after_rebase_step(r, "continued");
                }
                "skip" => {
                    let r = self.engine.rebase_skip();
                    self.after_rebase_step(r, "skipped");
                }
                "abort" => {
                    let r = self.engine.rebase_abort();
                    self.after_rebase_step(r, "aborted");
                }
                _ => self.message = "resolve conflicts in your editor, then Continue".into(),
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
    use crate::engine::{ChangedFile, Commit, CommitFile, PushOpts, ResetMode};
    use std::path::Path;

    /// A canned engine so the App logic can be tested without a terminal.
    struct Mock {
        root: std::path::PathBuf,
    }
    impl GitEngine for Mock {
        fn status(&self) -> Result<Vec<ChangedFile>> {
            Ok(vec![
                ChangedFile {
                    path: "a.rs".into(),
                    status: FileStatus::Modified,
                },
                ChangedFile {
                    path: "b.rs".into(),
                    status: FileStatus::Untracked,
                },
            ])
        }
        fn diff(&self, path: &str) -> Result<String> {
            Ok(format!("+++ {path}\n@@ -1 +1 @@\n+changed"))
        }
        fn branch_state(&self) -> Result<BranchState> {
            Ok(BranchState {
                current_branch: Some("main".into()),
                ..Default::default()
            })
        }
        fn stage(&self, _: &[String]) -> Result<()> {
            Ok(())
        }
        fn commit(&self, _: &[String], _: &str, _: bool) -> Result<String> {
            Ok("x".into())
        }
        fn log(&self, _: usize) -> Result<Vec<Commit>> {
            Ok(vec![])
        }
        fn remote_branches(&self) -> Result<Vec<String>> {
            Ok(vec![])
        }
        fn log_for(&self, _: &str, _: usize) -> Result<Vec<Commit>> {
            Ok(vec![])
        }
        fn commit_files(&self, _: &str) -> Result<Vec<CommitFile>> {
            Ok(vec![])
        }
        fn commit_body(&self, _: &str) -> Result<String> {
            Ok(String::new())
        }
        fn commit_file_diff(&self, _: &str, _: &str) -> Result<String> {
            Ok(String::new())
        }
        fn revert(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn reset(&self, _: &str, _: ResetMode) -> Result<()> {
            Ok(())
        }
        fn checkout_file(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn branches(&self) -> Result<Vec<String>> {
            Ok(vec![])
        }
        fn checkout_branch(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn create_branch(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        fn push(&self, _: &str, _: &PushOpts) -> Result<()> {
            Ok(())
        }
        fn fetch(&self) -> Result<()> {
            Ok(())
        }
        fn pull(&self) -> Result<()> {
            Ok(())
        }
        fn rebase_onto(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn rebase_continue(&self) -> Result<()> {
            Ok(())
        }
        fn rebase_skip(&self) -> Result<()> {
            Ok(())
        }
        fn rebase_abort(&self) -> Result<()> {
            Ok(())
        }
        fn conflicts(&self) -> Result<Vec<String>> {
            Ok(vec![])
        }
        fn repo_root(&self) -> &Path {
            &self.root
        }
    }

    #[test]
    fn builds_grouped_rows_navigates_and_marks() {
        let mock = Mock {
            root: std::env::temp_dir().join("mygit-app-test-norepo"),
        };
        let mut app = App::new(&mock);

        // Default header + a.rs (modified), then Unversioned Files header + b.rs
        // (untracked): 4 rows.
        assert_eq!(app.rows.len(), 4);
        assert!(matches!(app.rows[0], Row::Header { .. }));

        app.on_action(Action::Down); // onto a.rs
        assert_eq!(app.selected_path(), Some("a.rs"));
        assert!(app.diff.contains("a.rs"));

        app.on_action(Action::Mark);
        assert!(app.marked.contains("a.rs"));

        app.on_action(Action::Help);
        assert!(matches!(app.overlay, Overlay::Help(_)));
    }

    #[test]
    fn help_palette_runs_selected_action() {
        let mock = Mock {
            root: std::env::temp_dir().join("mygit-app-test-help"),
        };
        let mut app = App::new(&mock);
        app.on_action(Action::Help);
        assert!(matches!(app.overlay, Overlay::Help(0)));

        // Move down to the "new changelist" row and run it with Enter.
        let target = keymap::BINDINGS
            .iter()
            .position(|(_, a)| *a == Action::NewList)
            .unwrap();
        for _ in 0..target {
            app.handle_key(key(event::KeyCode::Down));
        }
        assert!(matches!(app.overlay, Overlay::Help(c) if c == target));
        app.handle_key(key(event::KeyCode::Enter));
        assert!(
            matches!(app.overlay, Overlay::Input(_)),
            "Enter on 'new changelist' opens the name input"
        );
    }

    #[test]
    fn changelist_ops_create_move_and_persist() {
        let mock = Mock {
            root: std::env::temp_dir().join("mygit-app-test-ops"),
        };
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
        let wip = app
            .store
            .changelists
            .iter()
            .find(|c| c.name == "WIP")
            .unwrap();
        assert!(
            wip.files.iter().any(|f| f == "a.rs"),
            "a.rs should be in WIP"
        );
    }

    fn key(code: event::KeyCode) -> event::KeyEvent {
        event::KeyEvent::new(code, event::KeyModifiers::NONE)
    }
    fn key_char(c: char) -> event::KeyEvent {
        key(event::KeyCode::Char(c))
    }

    fn mev(kind: event::MouseEventKind, column: u16, row: u16) -> event::MouseEvent {
        event::MouseEvent {
            kind,
            column,
            row,
            modifiers: event::KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_click_focuses_and_drag_resizes() {
        use event::{MouseButton, MouseEventKind};
        let mock = Mock {
            root: std::env::temp_dir().join("mygit-app-test-mouse"),
        };
        let mut app = App::new(&mock);
        let area = Rect::new(0, 0, 100, 30);

        // Click the right half -> Detail focus; left half -> Changes focus.
        app.handle_mouse(mev(MouseEventKind::Down(MouseButton::Left), 80, 10), area);
        assert_eq!(app.focus, Focus::Detail);
        app.handle_mouse(mev(MouseEventKind::Down(MouseButton::Left), 5, 10), area);
        assert_eq!(app.focus, Focus::Changes);

        // Drag the divider left -> the Changes panel gets narrower.
        let [_, _changes, detail, _] = super::ui::regions(area, app.split_pct);
        app.handle_mouse(
            mev(MouseEventKind::Down(MouseButton::Left), detail.x, 10),
            area,
        );
        assert!(app.dragging);
        app.handle_mouse(mev(MouseEventKind::Drag(MouseButton::Left), 30, 10), area);
        assert!(
            (20..=40).contains(&app.split_pct),
            "split now {}",
            app.split_pct
        );
        app.handle_mouse(mev(MouseEventKind::Up(MouseButton::Left), 30, 10), area);
        assert!(!app.dragging);
    }

    #[test]
    fn renders_log_browser_panes() {
        use ratatui::{backend::TestBackend, Terminal};
        let mock = Mock {
            root: std::env::temp_dir().join("mygit-app-test-logui"),
        };
        let mut app = App::new(&mock);
        app.on_action(Action::Log);
        assert!(app.log.is_some());

        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| super::ui::render(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        for needle in ["BRANCHES", "COMMITS", "COMMIT", "DIFF"] {
            assert!(text.contains(needle), "log frame missing {needle:?}");
        }
    }

    #[test]
    fn log_help_opens_and_closes() {
        let mock = Mock {
            root: std::env::temp_dir().join("mygit-app-test-loghelp"),
        };
        let mut app = App::new(&mock);
        app.on_action(Action::Log);
        app.handle_key(key_char('?'));
        assert!(matches!(app.overlay, Overlay::LogHelp));
        app.handle_key(key_char('x')); // any key closes
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.log.is_some(), "closing help stays in the log browser");
    }

    fn init_repo(tag: &str) -> std::path::PathBuf {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("mygit-tui-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |a: &[&str]| {
            assert!(Command::new("git")
                .current_dir(&dir)
                .args(a)
                .output()
                .unwrap()
                .status
                .success());
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
        // Seed both files as tracked, then modify them so they land in Default
        // (modified/tracked), not Unversioned (which is only for untracked).
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        let engine = GixEngine::discover(&dir).unwrap();
        engine
            .commit(&["a.txt".to_string(), "b.txt".to_string()], "seed", false)
            .unwrap();
        std::fs::write(dir.join("a.txt"), "a2").unwrap();
        std::fs::write(dir.join("b.txt"), "b2").unwrap();

        let mut app = App::new(&engine); // a.txt, b.txt (modified) -> Default

        let nfc = app.store.create("Not for commit").unwrap();
        app.store.move_files(&["b.txt".to_string()], &nfc).unwrap();
        let _ = app.store.persist(&app.store_path);
        app.rebuild_rows();

        // Commit the Default list (cursor on its header, no marks).
        app.cursor = 0;
        app.on_action(Action::Commit);
        assert!(matches!(app.overlay, Overlay::Input(_)));
        for c in "edit a".chars() {
            app.handle_key(key_char(c));
        }
        app.handle_key(key(event::KeyCode::Enter));

        let log = engine.log(10).unwrap();
        assert_eq!(log.len(), 2, "seed + the Default commit");
        let changed = engine.status().unwrap();
        assert!(
            !changed.iter().any(|f| f.path == "a.txt"),
            "a.txt should be committed"
        );
        assert!(
            changed.iter().any(|f| f.path == "b.txt"),
            "b.txt must remain changed"
        );
        let nfc_list = app
            .store
            .changelists
            .iter()
            .find(|c| c.name == "Not for commit")
            .unwrap();
        assert!(nfc_list.files.iter().any(|f| f == "b.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn revert_and_reset_from_log() {
        use crate::engine::GixEngine;
        let dir = init_repo("log");
        let engine = GixEngine::discover(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "1").unwrap();
        engine.commit(&["a.txt".to_string()], "c1", false).unwrap();
        std::fs::write(dir.join("a.txt"), "2").unwrap();
        engine.commit(&["a.txt".to_string()], "c2", false).unwrap();

        let mut app = App::new(&engine);
        app.on_action(Action::Log); // enter the Log browser
        assert!(app.log.is_some());

        // Revert the newest commit (log defaults to the current branch, newest
        // first) -> inverse commit added, history preserved.
        app.handle_key(key_char('v'));
        assert_eq!(engine.log(10).unwrap().len(), 3);

        // Move down to the oldest commit (c1) and hard-reset to it.
        app.handle_key(key(event::KeyCode::Down));
        app.handle_key(key(event::KeyCode::Down));
        app.handle_key(key_char('x'));
        assert!(matches!(app.overlay, Overlay::Picker(_)));
        app.handle_key(key(event::KeyCode::Down)); // mixed
        app.handle_key(key(event::KeyCode::Down)); // hard
        app.handle_key(key(event::KeyCode::Enter)); // -> confirm
        assert!(matches!(app.overlay, Overlay::Confirm(_)));
        app.handle_key(key_char('y')); // confirm hard reset
        assert_eq!(
            engine.log(10).unwrap().len(),
            1,
            "hard reset to oldest -> one commit"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollback_confirm_defaults_to_cancel() {
        use crate::engine::GixEngine;
        let dir = init_repo("rollback");
        let engine = GixEngine::discover(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "1").unwrap();
        engine.commit(&["a.txt".to_string()], "c1", false).unwrap();
        std::fs::write(dir.join("a.txt"), "changed").unwrap();
        let mut app = App::new(&engine);
        app.cursor = 1; // a.txt
        app.update_diff();
        app.on_action(Action::Rollback);
        assert!(matches!(app.overlay, Overlay::Confirm(_)));
        // Esc cancels -> file keeps changes
        app.handle_key(key(event::KeyCode::Esc));
        assert!(engine.status().unwrap().iter().any(|f| f.path == "a.txt"));
        // Now confirm with 'y' -> file restored to HEAD
        app.on_action(Action::Rollback);
        app.handle_key(key_char('y'));
        assert!(
            !engine.status().unwrap().iter().any(|f| f.path == "a.txt"),
            "rollback restores HEAD"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn init_bare(tag: &str) -> std::path::PathBuf {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("mygit-bare-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(Command::new("git")
            .current_dir(&dir)
            .args(["init", "--bare", "-q"])
            .output()
            .unwrap()
            .status
            .success());
        dir
    }

    #[test]
    fn push_new_branch_sets_upstream_and_create_checkout() {
        use crate::engine::GixEngine;
        use std::process::Command;
        let remote = init_bare("push-remote");
        let dir = init_repo("push");
        let run = |a: &[&str]| {
            assert!(Command::new("git")
                .current_dir(&dir)
                .args(a)
                .output()
                .unwrap()
                .status
                .success());
        };
        run(&["remote", "add", "origin", remote.to_str().unwrap()]);
        std::fs::write(dir.join("a.txt"), "1").unwrap();
        let engine = GixEngine::discover(&dir).unwrap();
        engine.commit(&["a.txt".to_string()], "c1", false).unwrap();

        let mut app = App::new(&engine);
        assert!(
            app.branch.upstream.is_none(),
            "no upstream before first push"
        );
        app.on_action(Action::Push); // no upstream -> push -u
        assert!(
            engine.branch_state().unwrap().upstream.is_some(),
            "upstream set after push -u"
        );

        // A new local commit makes the branch ahead by 1.
        std::fs::write(dir.join("a.txt"), "2").unwrap();
        engine.commit(&["a.txt".to_string()], "c2", false).unwrap();
        assert_eq!(
            engine.branch_state().unwrap().ahead,
            1,
            "ahead reflects the new commit"
        );

        // Create + checkout a new branch via the branches picker.
        app.on_action(Action::Branches);
        assert!(matches!(app.overlay, Overlay::Picker(_)));
        app.handle_key(key(event::KeyCode::Enter)); // "＋ new branch…" is first
        assert!(matches!(app.overlay, Overlay::Input(_)));
        for c in "feature-x".chars() {
            app.handle_key(key_char(c));
        }
        app.handle_key(key(event::KeyCode::Enter));
        assert!(engine.branches().unwrap().iter().any(|b| b == "feature-x"));
        assert_eq!(
            engine.branch_state().unwrap().current_branch.as_deref(),
            Some("feature-x")
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&remote);
    }

    fn select_picker(app: &mut App, label: &str) {
        if let Overlay::Picker(p) = &mut app.overlay {
            p.cursor = p
                .items
                .iter()
                .position(|i| i.label == label)
                .expect("picker item");
        }
        app.handle_key(key(event::KeyCode::Enter));
    }

    #[test]
    fn rebase_onto_completes_cleanly() {
        use crate::engine::GixEngine;
        use std::process::Command;
        let dir = init_repo("rebase-clean");
        let run = |a: &[&str]| {
            assert!(Command::new("git")
                .current_dir(&dir)
                .args(a)
                .output()
                .unwrap()
                .status
                .success());
        };
        let engine = GixEngine::discover(&dir).unwrap();
        std::fs::write(dir.join("base.txt"), "base").unwrap();
        engine
            .commit(&["base.txt".to_string()], "base", false)
            .unwrap();
        let base = engine.branch_state().unwrap().current_branch.unwrap();

        run(&["checkout", "-q", "-b", "feature"]);
        std::fs::write(dir.join("feat.txt"), "feat").unwrap();
        engine
            .commit(&["feat.txt".to_string()], "feat", false)
            .unwrap();
        run(&["checkout", "-q", &base]);
        std::fs::write(dir.join("other.txt"), "other").unwrap();
        engine
            .commit(&["other.txt".to_string()], "other", false)
            .unwrap();
        run(&["checkout", "-q", "feature"]);

        let mut app = App::new(&engine);
        app.on_action(Action::Rebase);
        select_picker(&mut app, &base);
        assert!(app.branch.rebase.is_none(), "clean rebase should complete");
        assert!(
            dir.join("other.txt").exists(),
            "base commit replayed under feature"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebase_conflict_then_abort_restores() {
        use crate::engine::GixEngine;
        use std::process::Command;
        let dir = init_repo("rebase-conflict");
        let run = |a: &[&str]| {
            assert!(Command::new("git")
                .current_dir(&dir)
                .args(a)
                .output()
                .unwrap()
                .status
                .success());
        };
        let engine = GixEngine::discover(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), "base\n").unwrap();
        engine
            .commit(&["f.txt".to_string()], "base", false)
            .unwrap();
        let base = engine.branch_state().unwrap().current_branch.unwrap();

        run(&["checkout", "-q", "-b", "feature"]);
        std::fs::write(dir.join("f.txt"), "feature version\n").unwrap();
        engine
            .commit(&["f.txt".to_string()], "feat", false)
            .unwrap();
        run(&["checkout", "-q", &base]);
        std::fs::write(dir.join("f.txt"), "base version\n").unwrap();
        engine
            .commit(&["f.txt".to_string()], "base2", false)
            .unwrap();
        run(&["checkout", "-q", "feature"]);

        let mut app = App::new(&engine);
        app.on_action(Action::Rebase);
        select_picker(&mut app, &base); // conflicts -> rebase stops
        assert!(
            app.branch.rebase.is_some(),
            "conflicting rebase stops in progress"
        );

        // Abort via the in-progress control picker.
        app.on_action(Action::Rebase);
        select_picker(&mut app, "Abort");
        assert!(app.branch.rebase.is_none(), "abort ends the rebase");
        assert_eq!(
            std::fs::read_to_string(dir.join("f.txt")).unwrap(),
            "feature version\n",
            "abort restores the pre-rebase feature state"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_refresh_picks_up_new_file() {
        use crate::engine::GixEngine;
        let dir = init_repo("autorefresh");
        std::fs::write(dir.join("committed.txt"), "x").unwrap();
        let engine = GixEngine::discover(&dir).unwrap();
        engine
            .commit(&["committed.txt".to_string()], "init", false)
            .unwrap();

        let mut app = App::new(&engine);
        assert!(app.status_map.is_empty(), "clean tree at start");

        // User edits a file on disk; no key pressed — the background tick catches it.
        std::fs::write(dir.join("new.txt"), "hello").unwrap();
        app.auto_refresh();
        assert!(
            app.status_map.contains_key("new.txt"),
            "auto-refresh picks up new.txt"
        );
        assert!(app
            .rows
            .iter()
            .any(|r| matches!(r, Row::File { path, .. } if path == "new.txt")));

        // Auto-refresh must not run while an overlay is open.
        app.overlay = Overlay::Help(0);
        std::fs::write(dir.join("another.txt"), "y").unwrap();
        app.auto_refresh();
        assert!(
            !app.status_map.contains_key("another.txt"),
            "no refresh while an overlay is open"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_requires_nonempty_message() {
        use crate::engine::GixEngine;
        let dir = init_repo("emptymsg");
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        let engine = GixEngine::discover(&dir).unwrap();
        engine
            .commit(&["a.txt".to_string()], "seed", false)
            .unwrap();
        std::fs::write(dir.join("a.txt"), "a2").unwrap(); // modified -> Default

        let mut app = App::new(&engine);
        app.cursor = 0;
        app.on_action(Action::Commit);
        app.handle_key(key(event::KeyCode::Enter)); // empty message
        assert!(
            matches!(app.overlay, Overlay::Input(_)),
            "overlay stays open on empty message"
        );
        assert_eq!(
            engine.log(10).unwrap().len(),
            1,
            "no new commit created (only seed)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn renders_frame_with_panels_and_content() {
        use ratatui::{backend::TestBackend, Terminal};
        let mock = Mock {
            root: std::env::temp_dir().join("mygit-app-test-render"),
        };
        let mut app = App::new(&mock);
        app.on_action(Action::Down); // select a.rs so the diff title shows it

        let mut terminal = Terminal::new(TestBackend::new(90, 20)).unwrap();
        terminal.draw(|f| super::ui::render(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();

        for needle in [
            "CHANGES",
            "DIFF",
            "main",
            "a.rs",
            "Default",
            "Unversioned Files",
        ] {
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
            fn status(&self) -> Result<Vec<ChangedFile>> {
                Ok(vec![])
            }
            fn diff(&self, _: &str) -> Result<String> {
                Ok(String::new())
            }
            fn branch_state(&self) -> Result<BranchState> {
                Ok(BranchState {
                    current_branch: Some("main".into()),
                    ..Default::default()
                })
            }
            fn stage(&self, _: &[String]) -> Result<()> {
                Ok(())
            }
            fn commit(&self, _: &[String], _: &str, _: bool) -> Result<String> {
                Ok("x".into())
            }
            fn log(&self, _: usize) -> Result<Vec<Commit>> {
                Ok(vec![])
            }
            fn remote_branches(&self) -> Result<Vec<String>> {
                Ok(vec![])
            }
            fn log_for(&self, _: &str, _: usize) -> Result<Vec<Commit>> {
                Ok(vec![])
            }
            fn commit_files(&self, _: &str) -> Result<Vec<CommitFile>> {
                Ok(vec![])
            }
            fn commit_body(&self, _: &str) -> Result<String> {
                Ok(String::new())
            }
            fn commit_file_diff(&self, _: &str, _: &str) -> Result<String> {
                Ok(String::new())
            }
            fn revert(&self, _: &str) -> Result<()> {
                Ok(())
            }
            fn reset(&self, _: &str, _: ResetMode) -> Result<()> {
                Ok(())
            }
            fn checkout_file(&self, _: &str) -> Result<()> {
                Ok(())
            }
            fn branches(&self) -> Result<Vec<String>> {
                Ok(vec![])
            }
            fn checkout_branch(&self, _: &str) -> Result<()> {
                Ok(())
            }
            fn create_branch(&self, _: &str, _: &str) -> Result<()> {
                Ok(())
            }
            fn push(&self, _: &str, _: &PushOpts) -> Result<()> {
                Ok(())
            }
            fn fetch(&self) -> Result<()> {
                Ok(())
            }
            fn pull(&self) -> Result<()> {
                Ok(())
            }
            fn rebase_onto(&self, _: &str) -> Result<()> {
                Ok(())
            }
            fn rebase_continue(&self) -> Result<()> {
                Ok(())
            }
            fn rebase_skip(&self) -> Result<()> {
                Ok(())
            }
            fn rebase_abort(&self) -> Result<()> {
                Ok(())
            }
            fn conflicts(&self) -> Result<Vec<String>> {
                Ok(vec![])
            }
            fn repo_root(&self) -> &std::path::Path {
                &self.root
            }
        }
        let clean = Clean {
            root: std::env::temp_dir().join("mygit-app-test-clean"),
        };
        let app = App::new(&clean);
        let mut terminal = Terminal::new(TestBackend::new(90, 12)).unwrap();
        terminal.draw(|f| super::ui::render(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(
            text.contains("No changes"),
            "expected empty state, got: {text}"
        );
    }
}
