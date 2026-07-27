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
use logview::{LogAction, LogHit, LogView};
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
    Input(InputState),
    Picker(PickerState),
    Confirm(ConfirmState),
    /// Stash manager: list named stashes; create / apply / pop / drop.
    Stashes(StashState),
    /// Git command log: what git commands ran and their detailed errors.
    Commands(CommandsState),
    /// A context menu / runnable command palette (right-click or `?` in the log).
    Menu(MenuState),
}

/// A list of runnable actions — used both as a right-click context menu (anchored
/// at the click) and as the log's `?` command palette (centred).
pub struct MenuState {
    pub title: String,
    pub items: Vec<MenuItem>,
    pub cursor: usize,
    /// Top-left anchor for a context menu; `None` centres it (palette).
    pub anchor: Option<(u16, u16)>,
    /// A dim hint line under the list (e.g. navigation keys for the palette).
    pub footer: Option<String>,
}

pub struct MenuItem {
    pub label: String,
    pub action: MenuAction,
}

/// A runnable action from a menu. Variants carry their target, captured when the
/// menu opens (not re-read on Enter).
#[derive(Clone)]
pub enum MenuAction {
    // log / history (target commit or branch)
    Checkout(String),
    RebaseOnto(String),
    FetchRebaseOnto(String),
    NewBranchFrom(String),
    Reword(String),
    SquashParent(String),
    SquashMarked,
    Drop(String),
    CherryPick(String),
    Revert(String),
    Reset(String),
    MarkToggle(String),
    Undo,
    Stashes,
    Commands,
    Push,
    // changes mode
    MoveFile,
    Rollback,
    CommitList,
    NewList,
    RenameList,
    DeleteList,
}

pub struct CommandsState {
    /// Snapshot of git invocations, newest first.
    pub entries: Vec<crate::engine::CmdEntry>,
    pub scroll: u16,
    /// Show only failed commands.
    pub failures_only: bool,
}

pub struct StashState {
    pub cursor: usize,
    pub items: Vec<StashItem>,
}

pub struct StashItem {
    /// `"__new__"` for the create row, else a stash selector like `stash@{0}`.
    pub id: String,
    pub label: String,
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
    /// Drop the given commit from the current branch's history.
    DropCommit(String),
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
    CommitMessage {
        files: Vec<String>,
        amend: bool,
    },
    NewBranch,
    /// Create a new branch starting at the given commit (from the log).
    NewBranchFrom(String),
    /// Reword (amend) the HEAD commit message.
    RewordHead,
    /// Reword an older commit via interactive rebase.
    RewordCommit(String),
    /// Squash the given commit into its parent with this combined message.
    SquashCommit(String),
    /// Squash the given marked commits (newest-first) with this combined message.
    SquashMarked(Vec<String>),
    /// Stash the working tree under this name, then checkout the given branch.
    StashName(String),
    /// Commit all changes with this message, then checkout the given branch.
    CommitAndSwitch(String),
    /// Stash the working tree under this name (from the stash manager).
    StashCreate,
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
    /// Drive whichever sequencer op (rebase/cherry-pick) is in progress, from the log.
    OpControl,
    /// Uncommitted changes exist — choose how to switch to the given branch.
    DirtyCheckout(String),
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

    /// A long-running git op queued to run after the next frame, so a busy
    /// indicator paints first (the event loop is otherwise blocked during it).
    pending: Option<PendingOp>,
    /// Label shown in the busy overlay while `pending` runs; `None` when idle.
    busy: Option<String>,
}

/// A git operation slow enough to warrant a "working…" frame before it blocks
/// the event loop. Run by the event loop, not inline in the key handler.
enum PendingOp {
    /// Fetch the remote ref, then (as a second busy phase) rebase onto it.
    FetchThenRebase(String),
    /// Rebase the current branch onto `target`; `fetched` tunes the done message.
    Rebase { target: String, fetched: bool },
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
            pending: None,
            busy: None,
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
            // A queued long op runs here — AFTER the busy frame was painted above,
            // so the user sees "working…" instead of a frozen screen. A phase may
            // queue the next phase (fetch → rebase), keeping `busy` set.
            if let Some(op) = self.pending.take() {
                self.run_pending(op);
                if self.pending.is_none() {
                    self.busy = None;
                }
                continue;
            }
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
            Overlay::Input(_) => self.handle_input_key(key),
            Overlay::Picker(_) => self.handle_picker_key(key),
            Overlay::Confirm(_) => self.handle_confirm_key(key),
            Overlay::Stashes(_) => self.handle_stashes_key(key),
            Overlay::Commands(_) => self.handle_commands_key(key),
            Overlay::Menu(_) => self.handle_menu_key(key),
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
        // The context menu owns the mouse while it is open.
        if let Overlay::Menu(_) = self.overlay {
            self.handle_menu_mouse(m, area);
            return;
        }
        if !matches!(self.overlay, Overlay::None) {
            return; // let other overlays own the interaction
        }
        // Right-click anywhere opens a context menu for the element under it.
        if let MouseEventKind::Down(MouseButton::Right) = m.kind {
            self.open_context_menu(m.column, m.row, area);
            return;
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
            Stashes => self.open_stashes(),
            Commands => self.open_commands(),
            Confirm | Cancel => {} // only meaningful inside overlays
        }
    }

    // ----- git command log ----------------------------------------------------

    /// Snapshot the engine's git-command log (newest first) into an overlay.
    fn open_commands(&mut self) {
        let mut entries = self.engine.command_log();
        entries.reverse(); // newest first
        self.overlay = Overlay::Commands(CommandsState {
            entries,
            scroll: 0,
            failures_only: false,
        });
    }

    fn handle_commands_key(&mut self, key: event::KeyEvent) {
        use event::KeyCode;
        match key.code {
            KeyCode::Esc | KeyCode::Char('g') | KeyCode::Char('q') => self.overlay = Overlay::None,
            KeyCode::Char('f') => {
                if let Overlay::Commands(c) = &mut self.overlay {
                    c.failures_only = !c.failures_only;
                    c.scroll = 0;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Overlay::Commands(c) = &mut self.overlay {
                    c.scroll = c.scroll.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Overlay::Commands(c) = &mut self.overlay {
                    c.scroll = c.scroll.saturating_add(1);
                }
            }
            _ => {}
        }
    }

    // ----- context menu / command palette -------------------------------------

    /// Open a context menu for the element under a right-click.
    fn open_context_menu(&mut self, col: u16, row: u16, area: Rect) {
        if self.log.is_some() {
            let body = Rect::new(
                area.x,
                area.y + 1,
                area.width,
                area.height.saturating_sub(2),
            );
            let engine = self.engine;
            let hit = self
                .log
                .as_mut()
                .and_then(|l| l.context_at(col, row, body, engine));
            let (title, items) = match hit {
                Some(LogHit::Branch {
                    target,
                    refname,
                    remote,
                }) => (
                    format!("Branch: {target}"),
                    branch_menu_items(&target, &refname, remote),
                ),
                Some(LogHit::Commit { hash }) => {
                    let marked = self.log.as_ref().map(|l| l.marked_count()).unwrap_or(0);
                    (
                        format!("Commit {}", &hash[..hash.len().min(8)]),
                        commit_menu_items(&hash, marked),
                    )
                }
                None => return,
            };
            self.overlay = Overlay::Menu(MenuState {
                title,
                items,
                cursor: 0,
                anchor: Some((col, row)),
                footer: None,
            });
        } else {
            let [_, changes, _, _] = ui::regions(area, self.split_pct);
            if col < changes.x || col >= changes.x + changes.width {
                return;
            }
            self.focus = Focus::Changes;
            self.select_row_at(row, changes);
            let items = self.changes_menu_items();
            if items.is_empty() {
                return;
            }
            self.overlay = Overlay::Menu(MenuState {
                title: "Actions".into(),
                items,
                cursor: 0,
                anchor: Some((col, row)),
                footer: None,
            });
        }
    }

    /// The log's `?` — a runnable command palette for the current selection.
    fn open_log_palette(&mut self) {
        let Some(l) = self.log.as_ref() else { return };
        let mut items = Vec::new();
        if let Some(hash) = l.selected_commit_hash() {
            items.extend(commit_menu_items(&hash, l.marked_count()));
        }
        if let Some((target, refname, remote)) = l.selected_branch_pair() {
            items.extend(branch_menu_items(&target, &refname, remote));
        }
        for (label, action) in [
            ("u  undo last reword/squash/drop", MenuAction::Undo),
            ("P  push current branch", MenuAction::Push),
            ("S  stashes", MenuAction::Stashes),
            ("g  git command log", MenuAction::Commands),
        ] {
            items.push(MenuItem {
                label: label.into(),
                action,
            });
        }
        self.overlay = Overlay::Menu(MenuState {
            title: "Log — run a command".into(),
            items,
            cursor: 0,
            anchor: None,
            footer: Some("↑↓ select · Enter run · Esc close   ·   Tab/drag/mouse: navigate".into()),
        });
    }

    fn changes_menu_items(&self) -> Vec<MenuItem> {
        match self.rows.get(self.cursor) {
            Some(Row::File { .. }) => vec![
                MenuItem {
                    label: "Move to changelist…".into(),
                    action: MenuAction::MoveFile,
                },
                MenuItem {
                    label: "Rollback file to HEAD".into(),
                    action: MenuAction::Rollback,
                },
                MenuItem {
                    label: "Commit this list…".into(),
                    action: MenuAction::CommitList,
                },
            ],
            Some(Row::Header { .. }) => vec![
                MenuItem {
                    label: "Commit this list…".into(),
                    action: MenuAction::CommitList,
                },
                MenuItem {
                    label: "New changelist…".into(),
                    action: MenuAction::NewList,
                },
                MenuItem {
                    label: "Rename changelist…".into(),
                    action: MenuAction::RenameList,
                },
                MenuItem {
                    label: "Delete changelist".into(),
                    action: MenuAction::DeleteList,
                },
            ],
            None => Vec::new(),
        }
    }

    fn handle_menu_key(&mut self, key: event::KeyEvent) {
        use event::KeyCode;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.overlay = Overlay::None,
            KeyCode::Up | KeyCode::Char('k') => {
                if let Overlay::Menu(mn) = &mut self.overlay {
                    mn.cursor = mn.cursor.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Overlay::Menu(mn) = &mut self.overlay {
                    if mn.cursor + 1 < mn.items.len() {
                        mn.cursor += 1;
                    }
                }
            }
            KeyCode::Enter => self.run_menu_cursor(),
            _ => {}
        }
    }

    fn handle_menu_mouse(&mut self, m: event::MouseEvent, area: Rect) {
        use event::{MouseButton, MouseEventKind};
        let MouseEventKind::Down(button) = m.kind else {
            return;
        };
        let rect = match &self.overlay {
            Overlay::Menu(mn) => ui::menu_rect(area, mn),
            _ => return,
        };
        let inside = m.column >= rect.x
            && m.column < rect.x + rect.width
            && m.row >= rect.y
            && m.row < rect.y + rect.height;
        if !inside || button == MouseButton::Right {
            self.overlay = Overlay::None; // click outside closes
            return;
        }
        // Map the click row to an item (list starts one row below the top border).
        let idx = (m.row.saturating_sub(rect.y + 1)) as usize;
        if let Overlay::Menu(mn) = &mut self.overlay {
            if idx < mn.items.len() {
                mn.cursor = idx;
            } else {
                return;
            }
        }
        self.run_menu_cursor();
    }

    /// Run the selected menu item. Clears the overlay *before* dispatching so an
    /// action that opens its own overlay (input/picker) isn't wiped.
    fn run_menu_cursor(&mut self) {
        let action = match &self.overlay {
            Overlay::Menu(mn) => mn.items.get(mn.cursor).map(|i| i.action.clone()),
            _ => None,
        };
        self.overlay = Overlay::None;
        if let Some(a) = action {
            self.dispatch_menu_action(a);
        }
    }

    fn dispatch_menu_action(&mut self, action: MenuAction) {
        use MenuAction::*;
        self.message.clear();
        match action {
            Checkout(target) => self.log_checkout(target),
            RebaseOnto(refname) => self.log_rebase_onto(&refname),
            FetchRebaseOnto(refname) => self.log_fetch_rebase_onto(&refname),
            NewBranchFrom(hash) => self.open_new_branch_from(hash),
            Reword(hash) => self.open_reword(hash),
            SquashParent(hash) => self.open_squash(hash),
            SquashMarked => {
                if let Some(l) = self.log.as_ref() {
                    let m = l.marked_hashes();
                    self.open_squash_marked(m);
                }
            }
            Drop(hash) => self.confirm_drop(hash),
            CherryPick(hash) => self.do_cherry_pick(&hash),
            Revert(hash) => self.do_revert(hash),
            Reset(hash) => self.open_reset_picker(hash),
            MarkToggle(hash) => {
                if let Some(l) = self.log.as_mut() {
                    l.toggle_mark_hash(&hash);
                }
            }
            Undo => self.do_undo(),
            Stashes => self.open_stashes(),
            Commands => self.open_commands(),
            Push => self.push_action(),
            MoveFile => self.open_move_files(),
            Rollback => self.open_rollback(),
            CommitList => self.open_commit(false),
            NewList => self.open_new_list(),
            RenameList => self.open_rename_list(),
            DeleteList => self.delete_current_list(),
        }
    }

    fn do_revert(&mut self, hash: String) {
        match self.engine.revert(&hash) {
            Ok(()) => {
                self.reload_log_and_state();
                self.message = "reverted".into();
            }
            Err(e) => self.message = e.to_string(),
        }
    }

    fn open_new_branch_from(&mut self, hash: String) {
        self.overlay = Overlay::Input(InputState {
            title: format!("New branch from {}", &hash[..hash.len().min(8)]),
            value: String::new(),
            purpose: InputPurpose::NewBranchFrom(hash),
        });
    }

    // ----- stash manager ------------------------------------------------------

    fn open_stashes(&mut self) {
        let mut items = vec![StashItem {
            id: "__new__".into(),
            label: "＋ Stash current changes…".into(),
        }];
        for (sel, msg) in self.engine.stash_list().unwrap_or_default() {
            items.push(StashItem {
                id: sel.clone(),
                label: format!("{sel}  {msg}"),
            });
        }
        self.overlay = Overlay::Stashes(StashState { cursor: 0, items });
    }

    fn selected_stash(&self) -> Option<String> {
        match &self.overlay {
            Overlay::Stashes(s) => s.items.get(s.cursor).map(|i| i.id.clone()),
            _ => None,
        }
    }

    fn handle_stashes_key(&mut self, key: event::KeyEvent) {
        use event::KeyCode;
        match key.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Up | KeyCode::Char('k') => {
                if let Overlay::Stashes(s) = &mut self.overlay {
                    s.cursor = s.cursor.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Overlay::Stashes(s) = &mut self.overlay {
                    if s.cursor + 1 < s.items.len() {
                        s.cursor += 1;
                    }
                }
            }
            KeyCode::Enter => match self.selected_stash().as_deref() {
                Some("__new__") => {
                    self.overlay = Overlay::Input(InputState {
                        title: "Stash name".into(),
                        value: String::new(),
                        purpose: InputPurpose::StashCreate,
                    });
                }
                Some(_) => self.stash_op("pop"),
                None => {}
            },
            KeyCode::Char('p') => self.stash_op("pop"),
            KeyCode::Char('a') => self.stash_op("apply"),
            KeyCode::Char('d') => self.stash_op("drop"),
            _ => {}
        }
    }

    fn stash_op(&mut self, kind: &str) {
        let Some(id) = self.selected_stash() else {
            return;
        };
        if id == "__new__" {
            return; // create is via Enter only
        }
        let r = match kind {
            "pop" => self.engine.stash_pop(&id),
            "apply" => self.engine.stash_apply(&id),
            _ => self.engine.stash_drop(&id),
        };
        match r {
            Ok(()) => {
                self.refresh();
                self.open_stashes(); // re-list, stay in the manager
                self.message = format!("stash {kind}");
            }
            Err(e) => {
                self.overlay = Overlay::None;
                self.message = e.to_string();
            }
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
            Err(e) => {
                self.message = if self.branch.rebase.is_some() {
                    "rebase step failed — unresolved conflicts remain".into()
                } else {
                    format!("{e} (press g for the git log)")
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
            LogAction::Help => self.open_log_palette(),
            LogAction::Revert(hash) => self.do_revert(hash),
            LogAction::Reset(hash) => self.open_reset_picker(hash),
            LogAction::Checkout(target) => self.log_checkout(target),
            LogAction::Push => self.push_action(),
            LogAction::NewBranchFrom(hash) => self.open_new_branch_from(hash),
            LogAction::RebaseOnto(target) => self.log_rebase_onto(&target),
            LogAction::FetchRebaseOnto(refname) => self.log_fetch_rebase_onto(&refname),
            LogAction::Reword(hash) => self.open_reword(hash),
            LogAction::Squash(hash) => self.open_squash(hash),
            LogAction::SquashMarked(hashes) => self.open_squash_marked(hashes),
            LogAction::Drop(hash) => self.confirm_drop(hash),
            LogAction::CherryPick(hash) => self.do_cherry_pick(&hash),
            LogAction::OpControl => self.open_op_control(),
            LogAction::Undo => self.do_undo(),
            LogAction::Stashes => self.open_stashes(),
            LogAction::ShowCommands => self.open_commands(),
        }
    }

    /// Ask for the combined message, then squash the marked set into one commit.
    fn open_squash_marked(&mut self, hashes: Vec<String>) {
        if hashes.len() < 2 {
            self.message = "mark at least two commits (space) to squash".into();
            return;
        }
        // Seed with the messages of the marked commits (oldest first).
        let value = hashes
            .iter()
            .rev()
            .filter_map(|h| self.engine.commit_body(h).ok())
            .filter_map(|b| b.lines().next().map(str::to_string))
            .collect::<Vec<_>>()
            .join("; ");
        self.overlay = Overlay::Input(InputState {
            title: format!("Squash {} commits — combined message", hashes.len()),
            value,
            purpose: InputPurpose::SquashMarked(hashes),
        });
    }

    fn confirm_drop(&mut self, hash: String) {
        if !self.engine.is_on_head(&hash) {
            self.message = "checkout this branch to edit its history".into();
            return;
        }
        let subject = self
            .engine
            .commit_body(&hash)
            .ok()
            .and_then(|b| b.lines().next().map(str::to_string))
            .unwrap_or_default();
        self.overlay = Overlay::Confirm(ConfirmState {
            title: "Drop commit".into(),
            body: format!(
                "Drop {} \"{}\"? (u to undo afterwards)",
                &hash[..hash.len().min(8)],
                subject
            ),
            purpose: ConfirmPurpose::DropCommit(hash),
        });
    }

    fn do_drop(&mut self, hash: &str) {
        match self.engine.drop_commit(hash) {
            Ok(()) => {
                self.rebuild_log_view();
                self.message = "commit dropped (u to undo)".into();
            }
            Err(e) => self.message = e.to_string(),
        }
    }

    fn do_cherry_pick(&mut self, hash: &str) {
        match self.engine.cherry_pick(hash) {
            Ok(()) => {
                self.rebuild_log_view();
                self.message = "cherry-picked (u to undo)".into();
            }
            Err(e) => {
                // On conflict the pick is left in progress — R drives it.
                self.rebuild_log_view();
                self.message = if self.engine.op_in_progress().is_some() {
                    "cherry-pick stopped on conflict — press R to resolve".into()
                } else {
                    e.to_string()
                };
            }
        }
    }

    /// Drive a stopped sequencer op (rebase/cherry-pick) from the log: list the
    /// conflicted files and offer continue / skip / abort.
    fn open_op_control(&mut self) {
        let Some(op) = self.engine.op_in_progress() else {
            self.message = "no operation in progress".into();
            return;
        };
        let conflicts = self.engine.conflicts().unwrap_or_default();
        let mut items: Vec<PickerItem> = conflicts
            .iter()
            .map(|p| PickerItem {
                label: format!("⚠ {p}"),
                id: "noop".into(),
            })
            .collect();
        let first_control = items.len();
        for (label, id) in [
            ("Continue", "continue"),
            ("Skip", "skip"),
            ("Abort", "abort"),
        ] {
            items.push(PickerItem {
                label: label.into(),
                id: id.into(),
            });
        }
        self.overlay = Overlay::Picker(PickerState {
            title: format!("{op} in progress"),
            items,
            cursor: first_control,
            purpose: PickerPurpose::OpControl,
        });
    }

    fn after_op_step(&mut self, result: Result<()>, ok: &str) {
        self.rebuild_log_view();
        match result {
            Ok(()) => {
                self.message = if self.engine.op_in_progress().is_some() {
                    "still in progress — resolve conflicts, then Continue".into()
                } else {
                    ok.to_string()
                };
            }
            Err(e) => {
                self.message = if self.engine.op_in_progress().is_some() {
                    "step failed — unresolved conflicts remain".into()
                } else {
                    format!("{e} (press g for the git log)")
                };
            }
        }
    }

    fn open_squash(&mut self, hash: String) {
        if !self.engine.is_on_head(&hash) {
            self.message = "checkout this branch to edit its history".into();
            return;
        }
        let this = self.engine.commit_body(&hash).unwrap_or_default();
        let parent = self
            .engine
            .commit_body(&format!("{hash}^"))
            .unwrap_or_default();
        let value = format!(
            "{} {}",
            parent.lines().next().unwrap_or(""),
            this.lines().next().unwrap_or("")
        )
        .trim()
        .to_string();
        self.overlay = Overlay::Input(InputState {
            title: "Squash into parent — combined message".into(),
            value,
            purpose: InputPurpose::SquashCommit(hash),
        });
    }

    fn do_undo(&mut self) {
        if !self.engine.has_backup() {
            self.message = "nothing to undo".into();
            return;
        }
        match self.engine.restore_backup() {
            Ok(()) => {
                self.rebuild_log_view();
                self.message = "undone".into();
            }
            Err(e) => self.message = e.to_string(),
        }
    }

    /// Checkout a branch from the log; if the working tree has tracked changes,
    /// ask how to handle them first (commit / stash / switch anyway).
    fn log_checkout(&mut self, target: String) {
        let dirty = self
            .engine
            .status()
            .map(|s| s.iter().any(|f| f.status != FileStatus::Untracked))
            .unwrap_or(false);
        if !dirty {
            self.do_checkout(&target);
            return;
        }
        let items = vec![
            PickerItem {
                label: "Stash (shelve) & switch".into(),
                id: "stash".into(),
            },
            PickerItem {
                label: "Commit & switch".into(),
                id: "commit".into(),
            },
            PickerItem {
                label: "Switch anyway (keep changes)".into(),
                id: "switch".into(),
            },
            PickerItem {
                label: "Cancel".into(),
                id: "cancel".into(),
            },
        ];
        self.overlay = Overlay::Picker(PickerState {
            title: format!("Uncommitted changes — switch to {target}?"),
            items,
            cursor: 0,
            purpose: PickerPurpose::DirtyCheckout(target),
        });
    }

    fn do_checkout(&mut self, target: &str) {
        match self.engine.checkout_branch(target) {
            Ok(()) => {
                self.rebuild_log_view();
                self.message = format!("switched to {target}");
            }
            Err(e) => self.message = e.to_string(),
        }
    }

    /// Queue a rebase onto a local branch (runs behind a busy frame).
    fn log_rebase_onto(&mut self, target: &str) {
        self.begin_busy(
            PendingOp::Rebase {
                target: target.to_string(),
                fetched: false,
            },
            format!("Rebasing onto {target}…"),
        );
    }

    /// Queue a fetch + rebase onto a remote branch (two busy phases).
    fn log_fetch_rebase_onto(&mut self, remote_ref: &str) {
        self.begin_busy(
            PendingOp::FetchThenRebase(remote_ref.to_string()),
            format!("Fetching {remote_ref}…"),
        );
    }

    /// Queue `op` to run after the next frame and show `label` in the busy overlay.
    fn begin_busy(&mut self, op: PendingOp, label: String) {
        self.message.clear();
        self.busy = Some(label);
        self.pending = Some(op);
    }

    /// Execute a queued long op (called by the event loop, after the busy frame).
    fn run_pending(&mut self, op: PendingOp) {
        match op {
            PendingOp::FetchThenRebase(remote_ref) => match self.engine.fetch_ref(&remote_ref) {
                Ok(()) => self.begin_busy(
                    PendingOp::Rebase {
                        target: remote_ref.clone(),
                        fetched: true,
                    },
                    format!("Rebasing onto {remote_ref}…"),
                ),
                Err(e) => self.after_failed_rebase(e),
            },
            PendingOp::Rebase { target, fetched } => match self.engine.rebase_onto(&target) {
                Ok(()) => {
                    self.rebuild_log_view();
                    self.message = if fetched {
                        format!("fetched + rebased onto {target}")
                    } else {
                        format!("rebased onto {target}")
                    };
                }
                Err(e) => self.after_failed_rebase(e),
            },
        }
    }

    /// Shared message handling when a rebase-onto returns an error: a conflict
    /// leaves the rebase in progress (drive it with R), else surface the reason.
    fn after_failed_rebase(&mut self, e: anyhow::Error) {
        self.refresh();
        self.message = if self.branch.rebase.is_some() {
            "rebase stopped — resolve, then press R".into()
        } else {
            format!("{e} (press g for the git log)")
        };
    }

    /// Reword only the HEAD commit in this phase (amend); older commits need an
    /// interactive rebase (a later phase).
    fn open_reword(&mut self, hash: String) {
        let head = self
            .engine
            .log_for("HEAD", 1)
            .ok()
            .and_then(|c| c.into_iter().next())
            .map(|c| c.hash);
        let is_head = head.as_deref() == Some(hash.as_str());
        // HEAD is a fast amend; older commits go through interactive rebase.
        if !is_head && !self.engine.is_on_head(&hash) {
            self.message = "checkout this branch to edit its history".into();
            return;
        }
        let value = self
            .engine
            .commit_body(&hash)
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
        let purpose = if is_head {
            InputPurpose::RewordHead
        } else {
            InputPurpose::RewordCommit(hash)
        };
        self.overlay = Overlay::Input(InputState {
            title: "Reword commit".into(),
            value,
            purpose,
        });
    }

    /// Rebuild the whole log browser (branch list, current, commits) after an
    /// operation that changed the repo structure.
    fn rebuild_log_view(&mut self) {
        self.refresh();
        if self.log.is_some() {
            self.log = Some(LogView::new(self.engine));
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
            ConfirmPurpose::DropCommit(hash) => self.do_drop(&hash),
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
            InputPurpose::NewBranchFrom(start) => {
                if value.is_empty() {
                    self.message = "branch name must not be empty".into();
                    return;
                }
                match self.engine.create_branch(&value, &start) {
                    Ok(()) => {
                        self.overlay = Overlay::None;
                        self.rebuild_log_view();
                        self.message = format!("created & switched to {value}");
                    }
                    Err(e) => self.message = e.to_string(),
                }
            }
            InputPurpose::RewordHead => {
                if value.is_empty() {
                    self.message = "commit message required".into();
                    return;
                }
                match self.engine.commit(&[], &value, true) {
                    Ok(_) => {
                        self.overlay = Overlay::None;
                        self.reload_log_and_state();
                        self.message = "reworded".into();
                    }
                    Err(e) => self.message = e.to_string(),
                }
            }
            InputPurpose::RewordCommit(hash) => {
                if value.is_empty() {
                    self.message = "commit message required".into();
                    return;
                }
                match self.engine.reword_commit(&hash, &value) {
                    Ok(()) => {
                        self.overlay = Overlay::None;
                        self.rebuild_log_view();
                        self.message = "reworded (u to undo)".into();
                    }
                    Err(e) => {
                        self.overlay = Overlay::None;
                        self.message = e.to_string();
                    }
                }
            }
            InputPurpose::SquashCommit(hash) => {
                if value.is_empty() {
                    self.message = "commit message required".into();
                    return;
                }
                match self.engine.squash_into_parent(&hash, &value) {
                    Ok(()) => {
                        self.overlay = Overlay::None;
                        self.rebuild_log_view();
                        self.message = "squashed (u to undo)".into();
                    }
                    Err(e) => {
                        self.overlay = Overlay::None;
                        self.message = e.to_string();
                    }
                }
            }
            InputPurpose::SquashMarked(hashes) => {
                if value.is_empty() {
                    self.message = "commit message required".into();
                    return;
                }
                match self.engine.squash_commits(&hashes, &value) {
                    Ok(()) => {
                        self.overlay = Overlay::None;
                        self.rebuild_log_view();
                        self.message = format!("squashed {} commits (u to undo)", hashes.len());
                    }
                    Err(e) => {
                        self.overlay = Overlay::None;
                        self.message = e.to_string();
                    }
                }
            }
            InputPurpose::StashName(target) => {
                if value.is_empty() {
                    self.message = "stash name required".into();
                    return;
                }
                match self.engine.stash_push(&value) {
                    Ok(()) => {
                        self.overlay = Overlay::None;
                        self.do_checkout(&target);
                    }
                    Err(e) => self.message = e.to_string(),
                }
            }
            InputPurpose::CommitAndSwitch(target) => {
                if value.is_empty() {
                    self.message = "commit message required".into();
                    return;
                }
                let files: Vec<String> = self
                    .status_map
                    .iter()
                    .filter(|(_, s)| **s != FileStatus::Untracked)
                    .map(|(p, _)| p.clone())
                    .collect();
                match self.engine.commit(&files, &value, false) {
                    Ok(_) => {
                        self.overlay = Overlay::None;
                        self.do_checkout(&target);
                    }
                    Err(e) => self.message = e.to_string(),
                }
            }
            InputPurpose::StashCreate => {
                if value.is_empty() {
                    self.message = "stash name required".into();
                    return;
                }
                match self.engine.stash_push(&value) {
                    Ok(()) => {
                        self.refresh();
                        self.open_stashes();
                        self.message = "stashed".into();
                    }
                    Err(e) => {
                        self.overlay = Overlay::None;
                        self.message = e.to_string();
                    }
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
            // Runs behind a busy frame like the log-mode rebase.
            PickerPurpose::RebaseOnto => self.log_rebase_onto(&id),
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
            PickerPurpose::OpControl => match id.as_str() {
                "continue" => {
                    let r = self.engine.op_continue();
                    self.after_op_step(r, "continued");
                }
                "skip" => {
                    let r = self.engine.op_skip();
                    self.after_op_step(r, "skipped");
                }
                "abort" => {
                    let r = self.engine.op_abort();
                    self.after_op_step(r, "aborted");
                }
                _ => self.message = "resolve conflicts in your editor, then Continue".into(),
            },
            PickerPurpose::DirtyCheckout(target) => match id.as_str() {
                "stash" => {
                    self.overlay = Overlay::Input(InputState {
                        title: format!("Stash name (then switch to {target})"),
                        value: String::new(),
                        purpose: InputPurpose::StashName(target),
                    });
                }
                "commit" => {
                    self.overlay = Overlay::Input(InputState {
                        title: format!("Commit message (then switch to {target})"),
                        value: String::new(),
                        purpose: InputPurpose::CommitAndSwitch(target),
                    });
                }
                "switch" => self.do_checkout(&target),
                _ => {} // cancel
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

/// Context-menu / palette items for a branch. `target` is the checkout name
/// (remote-stripped); `refname` is the full ref used as a rebase base. A remote
/// branch fetches before rebasing so it lands on the latest state.
fn branch_menu_items(target: &str, refname: &str, remote: bool) -> Vec<MenuItem> {
    let mut items = vec![MenuItem {
        label: format!("c  Checkout {target}"),
        action: MenuAction::Checkout(target.to_string()),
    }];
    if remote {
        items.push(MenuItem {
            label: format!("R  Fetch & rebase current onto {refname}"),
            action: MenuAction::FetchRebaseOnto(refname.to_string()),
        });
    } else {
        items.push(MenuItem {
            label: format!("R  Rebase current onto {refname}"),
            action: MenuAction::RebaseOnto(refname.to_string()),
        });
    }
    items
}

/// Context-menu / palette items for a commit on the current branch.
fn commit_menu_items(hash: &str, marked: usize) -> Vec<MenuItem> {
    let h = hash.to_string();
    let mut items = vec![MenuItem {
        label: "r  Reword".into(),
        action: MenuAction::Reword(h.clone()),
    }];
    if marked >= 2 {
        items.push(MenuItem {
            label: format!("s  Squash {marked} marked"),
            action: MenuAction::SquashMarked,
        });
    } else {
        items.push(MenuItem {
            label: "s  Squash into parent".into(),
            action: MenuAction::SquashParent(h.clone()),
        });
    }
    items.push(MenuItem {
        label: "space  Mark / unmark for squash".into(),
        action: MenuAction::MarkToggle(h.clone()),
    });
    items.push(MenuItem {
        label: "d  Drop".into(),
        action: MenuAction::Drop(h.clone()),
    });
    items.push(MenuItem {
        label: "C  Cherry-pick onto current".into(),
        action: MenuAction::CherryPick(h.clone()),
    });
    items.push(MenuItem {
        label: "v  Revert".into(),
        action: MenuAction::Revert(h.clone()),
    });
    items.push(MenuItem {
        label: "x  Reset to here".into(),
        action: MenuAction::Reset(h.clone()),
    });
    items.push(MenuItem {
        label: "b  New branch from here".into(),
        action: MenuAction::NewBranchFrom(h),
    });
    items
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
        fn stash_push(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn is_on_head(&self, _: &str) -> bool {
            true
        }
        fn reword_commit(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        fn squash_into_parent(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        fn squash_commits(&self, _: &[String], _: &str) -> Result<()> {
            Ok(())
        }
        fn drop_commit(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn cherry_pick(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn backup_head(&self) -> Result<()> {
            Ok(())
        }
        fn has_backup(&self) -> bool {
            false
        }
        fn restore_backup(&self) -> Result<()> {
            Ok(())
        }
        fn stash_list(&self) -> Result<Vec<(String, String)>> {
            Ok(vec![])
        }
        fn stash_apply(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn stash_pop(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn stash_drop(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn rebase_onto(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn fetch_ref(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn fetch_and_rebase_onto(&self, _: &str) -> Result<()> {
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
        fn op_in_progress(&self) -> Option<&'static str> {
            None
        }
        fn op_continue(&self) -> Result<()> {
            Ok(())
        }
        fn op_skip(&self) -> Result<()> {
            Ok(())
        }
        fn op_abort(&self) -> Result<()> {
            Ok(())
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
    fn log_palette_opens_navigates_and_closes() {
        let mock = Mock {
            root: std::env::temp_dir().join("mygit-app-test-loghelp"),
        };
        let mut app = App::new(&mock);
        app.on_action(Action::Log);
        app.handle_key(key_char('?')); // runnable command palette
        match &app.overlay {
            Overlay::Menu(mn) => assert!(!mn.items.is_empty(), "palette has commands"),
            _ => panic!("expected the command palette"),
        }
        app.handle_key(key(event::KeyCode::Down)); // navigate
        app.handle_key(key(event::KeyCode::Esc)); // close
        assert!(matches!(app.overlay, Overlay::None));
        assert!(
            app.log.is_some(),
            "closing the palette stays in the log browser"
        );
    }

    #[test]
    fn log_checkout_stashes_dirty_then_switches() {
        use crate::engine::GixEngine;
        let dir = init_repo("logco");
        let engine = GixEngine::discover(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "1").unwrap();
        engine.commit(&["a.txt".to_string()], "c1", false).unwrap();
        let base = engine.branch_state().unwrap().current_branch.unwrap();
        engine.create_branch("feature", "HEAD").unwrap(); // switches to feature
        engine.checkout_branch(&base).unwrap(); // back to base
        std::fs::write(dir.join("a.txt"), "dirty").unwrap(); // tracked change

        let mut app = App::new(&engine);
        app.on_action(Action::Log);
        app.log_checkout("feature".to_string());
        // dirty -> the how-to-switch picker opens
        assert!(matches!(app.overlay, Overlay::Picker(_)));
        app.handle_key(key(event::KeyCode::Enter)); // "Stash (shelve) & switch"
        assert!(matches!(app.overlay, Overlay::Input(_)));
        for c in "wip".chars() {
            app.handle_key(key_char(c));
        }
        app.handle_key(key(event::KeyCode::Enter)); // stash + checkout

        assert_eq!(
            engine.branch_state().unwrap().current_branch.as_deref(),
            Some("feature")
        );
        assert!(
            engine.status().unwrap().is_empty(),
            "changes were stashed away"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stashes_overlay_open_and_create() {
        let mock = Mock {
            root: std::env::temp_dir().join("mygit-app-test-stash"),
        };
        let mut app = App::new(&mock);
        app.on_action(Action::Stashes);
        assert!(matches!(app.overlay, Overlay::Stashes(_)));
        // Enter on the "＋ create" row opens the name input.
        app.handle_key(key(event::KeyCode::Enter));
        assert!(matches!(app.overlay, Overlay::Input(_)));
        for c in "wip".chars() {
            app.handle_key(key_char(c));
        }
        app.handle_key(key(event::KeyCode::Enter)); // stash_push (mock) -> re-list
        assert!(matches!(app.overlay, Overlay::Stashes(_)));
    }

    #[test]
    fn log_reword_older_commit_then_undo() {
        use crate::engine::GixEngine;
        let dir = init_repo("logreword");
        let engine = GixEngine::discover(&dir).unwrap();
        for (f, m) in [("a.txt", "c1"), ("b.txt", "c2"), ("c.txt", "c3")] {
            std::fs::write(dir.join(f), m).unwrap();
            engine.commit(&[f.to_string()], m, false).unwrap();
        }
        let mut app = App::new(&engine);
        app.on_action(Action::Log); // Commits focus, newest (c3) selected
        app.handle_key(key(event::KeyCode::Down)); // -> c2 (older)
        app.handle_key(key_char('r')); // reword -> interactive-rebase input
        assert!(matches!(app.overlay, Overlay::Input(_)));
        for _ in 0..8 {
            app.handle_key(key(event::KeyCode::Backspace)); // clear prefill
        }
        for c in "c2 new".chars() {
            app.handle_key(key_char(c));
        }
        app.handle_key(key(event::KeyCode::Enter));
        assert!(engine
            .log(10)
            .unwrap()
            .iter()
            .any(|c| c.summary == "c2 new"));

        app.handle_key(key_char('u')); // undo
        let log = engine.log(10).unwrap();
        assert!(log.iter().any(|c| c.summary == "c2"));
        assert!(!log.iter().any(|c| c.summary == "c2 new"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_multi_squash_marked() {
        use crate::engine::GixEngine;
        let dir = init_repo("logmsquash");
        let engine = GixEngine::discover(&dir).unwrap();
        for (f, m) in [("a.txt", "c1"), ("b.txt", "c2"), ("c.txt", "c3")] {
            std::fs::write(dir.join(f), m).unwrap();
            engine.commit(&[f.to_string()], m, false).unwrap();
        }
        let mut app = App::new(&engine);
        app.on_action(Action::Log); // Commits focus, c3 (newest) selected
        app.handle_key(key_char(' ')); // mark c3
        app.handle_key(key(event::KeyCode::Down)); // -> c2
        app.handle_key(key_char(' ')); // mark c2
        app.handle_key(key_char('s')); // squash marked -> combined-message input
        assert!(matches!(app.overlay, Overlay::Input(_)));
        for _ in 0..40 {
            app.handle_key(key(event::KeyCode::Backspace)); // clear prefill
        }
        for c in "c2c3".chars() {
            app.handle_key(key_char(c));
        }
        app.handle_key(key(event::KeyCode::Enter));
        let log = engine.log(10).unwrap();
        assert_eq!(log.len(), 2, "c2 + c3 merged into one");
        assert!(log.iter().any(|c| c.summary == "c2c3"));
        assert!(log.iter().any(|c| c.summary == "c1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_drop_commit_confirmed() {
        use crate::engine::GixEngine;
        let dir = init_repo("logdrop");
        let engine = GixEngine::discover(&dir).unwrap();
        for (f, m) in [("a.txt", "c1"), ("b.txt", "c2"), ("c.txt", "c3")] {
            std::fs::write(dir.join(f), m).unwrap();
            engine.commit(&[f.to_string()], m, false).unwrap();
        }
        let mut app = App::new(&engine);
        app.on_action(Action::Log);
        app.handle_key(key(event::KeyCode::Down)); // -> c2
        app.handle_key(key_char('d')); // drop -> confirm
        assert!(matches!(app.overlay, Overlay::Confirm(_)));
        app.handle_key(key_char('y')); // confirm
        let log = engine.log(10).unwrap();
        assert_eq!(log.len(), 2);
        assert!(!log.iter().any(|c| c.summary == "c2"));
        assert!(log.iter().any(|c| c.summary == "c1"));
        assert!(log.iter().any(|c| c.summary == "c3"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_cherry_pick_conflict_then_resolve() {
        use crate::engine::GixEngine;
        use std::process::Command;
        let dir = init_repo("logpick");
        let engine = GixEngine::discover(&dir).unwrap();
        let git = |a: &[&str]| {
            assert!(Command::new("git")
                .current_dir(&dir)
                .args(a)
                .output()
                .unwrap()
                .status
                .success());
        };
        std::fs::write(dir.join("f.txt"), "base\n").unwrap();
        engine
            .commit(&["f.txt".to_string()], "base", false)
            .unwrap();
        let main = engine.branch_state().unwrap().current_branch.unwrap();
        git(&["checkout", "-q", "-b", "other"]);
        std::fs::write(dir.join("f.txt"), "other\n").unwrap();
        let other_c = engine
            .commit(&["f.txt".to_string()], "on-other", false)
            .unwrap();
        git(&["checkout", "-q", &main]);
        std::fs::write(dir.join("f.txt"), "main\n").unwrap();
        engine
            .commit(&["f.txt".to_string()], "on-main", false)
            .unwrap();

        let mut app = App::new(&engine);
        app.on_action(Action::Log);
        app.do_cherry_pick(&other_c); // conflict -> left in progress
        assert_eq!(engine.op_in_progress(), Some("cherry-pick"));
        // R while a pick is stopped opens the op-control picker.
        app.handle_key(key_char('R'));
        assert!(matches!(app.overlay, Overlay::Picker(_)));
        app.handle_key(key(event::KeyCode::Down)); // Continue -> Skip
        app.handle_key(key(event::KeyCode::Down)); // Skip -> Abort
        app.handle_key(key(event::KeyCode::Enter)); // abort
        assert!(engine.op_in_progress().is_none());
        assert_eq!(engine.log(10).unwrap()[0].summary, "on-main");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_log_overlay_opens_and_filters() {
        use crate::engine::GixEngine;
        let dir = init_repo("cmdlog");
        let engine = GixEngine::discover(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        engine.commit(&["a.txt".to_string()], "c1", false).unwrap();
        let mut app = App::new(&engine); // status/branch_state calls get recorded
        app.handle_key(key_char('g')); // open the git command log
        match &app.overlay {
            Overlay::Commands(c) => {
                assert!(!c.entries.is_empty(), "some git commands recorded");
                assert!(!c.failures_only);
            }
            _ => panic!("expected the commands overlay"),
        }
        // Render the overlay to catch layout panics before they reach users.
        {
            use ratatui::{backend::TestBackend, Terminal};
            let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
            term.draw(|f| super::ui::render(f, &app)).unwrap();
            let text: String = term
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(text.contains("Git command log"));
        }
        app.handle_key(key_char('f')); // toggle failures-only
        assert!(matches!(&app.overlay, Overlay::Commands(c) if c.failures_only));
        app.handle_key(key(event::KeyCode::Esc)); // close
        assert!(matches!(app.overlay, Overlay::None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_palette_reword_opens_input() {
        use crate::engine::GixEngine;
        let dir = init_repo("logpal");
        let engine = GixEngine::discover(&dir).unwrap();
        for (f, m) in [("a.txt", "c1"), ("b.txt", "c2")] {
            std::fs::write(dir.join(f), m).unwrap();
            engine.commit(&[f.to_string()], m, false).unwrap();
        }
        let mut app = App::new(&engine);
        app.on_action(Action::Log); // newest commit selected
        app.handle_key(key_char('?')); // runnable palette
        assert!(matches!(app.overlay, Overlay::Menu(_)));
        // First item is "r Reword"; Enter runs it -> the Input overlay opens. This
        // only survives if the menu is cleared BEFORE dispatch, not after.
        app.handle_key(key(event::KeyCode::Enter));
        assert!(matches!(app.overlay, Overlay::Input(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_right_click_opens_commit_menu() {
        use crate::engine::GixEngine;
        use event::{MouseButton, MouseEventKind};
        let dir = init_repo("logctx");
        let engine = GixEngine::discover(&dir).unwrap();
        for (f, m) in [("a.txt", "c1"), ("b.txt", "c2")] {
            std::fs::write(dir.join(f), m).unwrap();
            engine.commit(&[f.to_string()], m, false).unwrap();
        }
        let mut app = App::new(&engine);
        app.on_action(Action::Log);
        let area = Rect::new(0, 0, 100, 30);
        // Right-click inside the COMMITS pane (x in [~22,62), a commit row).
        app.handle_mouse(mev(MouseEventKind::Down(MouseButton::Right), 40, 2), area);
        match &app.overlay {
            Overlay::Menu(mn) => {
                assert!(mn.anchor.is_some(), "context menu anchors at the click");
                assert!(mn.items.iter().any(|i| i.label.contains("Reword")));
                assert!(mn.items.iter().any(|i| i.label.contains("Cherry-pick")));
            }
            _ => panic!("expected a context menu"),
        }
        // Render the anchored menu (near the edge exercises clamping).
        {
            use ratatui::{backend::TestBackend, Terminal};
            let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
            term.draw(|f| super::ui::render(f, &app)).unwrap();
        }
        app.handle_key(key(event::KeyCode::Esc));
        assert!(matches!(app.overlay, Overlay::None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebase_onto_defers_behind_a_busy_frame() {
        use ratatui::{backend::TestBackend, Terminal};
        let mock = Mock {
            root: std::env::temp_dir().join("mygit-busy-rebase"),
        };
        let mut app = App::new(&mock);
        app.log_rebase_onto("develop");
        // Queued, NOT run synchronously — the loop runs it after painting busy.
        assert!(app.busy.is_some(), "busy label set");
        assert!(app.pending.is_some(), "op queued");
        assert!(app.message.is_empty(), "op did not run synchronously");
        // The busy overlay paints.
        let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
        term.draw(|f| super::ui::render(f, &app)).unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("Working"), "busy overlay painted");
        // Now the loop's step runs it and clears busy.
        let op = app.pending.take().unwrap();
        app.run_pending(op);
        if app.pending.is_none() {
            app.busy = None;
        }
        assert!(app.busy.is_none());
        assert!(
            app.message.contains("rebased onto develop"),
            "msg: {}",
            app.message
        );
    }

    #[test]
    fn fetch_rebase_runs_in_two_busy_phases() {
        let mock = Mock {
            root: std::env::temp_dir().join("mygit-busy-fetch"),
        };
        let mut app = App::new(&mock);
        app.log_fetch_rebase_onto("origin/develop");
        assert!(app.message.is_empty(), "nothing ran yet");
        assert!(app.busy.as_deref().unwrap().contains("Fetching"));
        // phase 1: fetch -> queues the rebase phase, busy switches label
        let op = app.pending.take().unwrap();
        app.run_pending(op);
        assert!(app.pending.is_some(), "rebase phase queued after the fetch");
        assert!(app.busy.as_deref().unwrap().contains("Rebasing"));
        // phase 2: rebase -> done
        let op2 = app.pending.take().unwrap();
        app.run_pending(op2);
        if app.pending.is_none() {
            app.busy = None;
        }
        assert!(app.busy.is_none());
        assert!(
            app.message
                .contains("fetched + rebased onto origin/develop"),
            "msg: {}",
            app.message
        );
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

    /// Run any queued long op(s) to completion, as the event loop would.
    fn drain_pending(app: &mut App) {
        while let Some(op) = app.pending.take() {
            app.run_pending(op);
        }
        app.busy = None;
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
        drain_pending(&mut app); // the rebase runs behind the busy frame
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
        drain_pending(&mut app); // the rebase runs behind the busy frame
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
            fn stash_push(&self, _: &str) -> Result<()> {
                Ok(())
            }
            fn is_on_head(&self, _: &str) -> bool {
                true
            }
            fn reword_commit(&self, _: &str, _: &str) -> Result<()> {
                Ok(())
            }
            fn squash_into_parent(&self, _: &str, _: &str) -> Result<()> {
                Ok(())
            }
            fn squash_commits(&self, _: &[String], _: &str) -> Result<()> {
                Ok(())
            }
            fn drop_commit(&self, _: &str) -> Result<()> {
                Ok(())
            }
            fn cherry_pick(&self, _: &str) -> Result<()> {
                Ok(())
            }
            fn backup_head(&self) -> Result<()> {
                Ok(())
            }
            fn has_backup(&self) -> bool {
                false
            }
            fn restore_backup(&self) -> Result<()> {
                Ok(())
            }
            fn stash_list(&self) -> Result<Vec<(String, String)>> {
                Ok(vec![])
            }
            fn stash_apply(&self, _: &str) -> Result<()> {
                Ok(())
            }
            fn stash_pop(&self, _: &str) -> Result<()> {
                Ok(())
            }
            fn stash_drop(&self, _: &str) -> Result<()> {
                Ok(())
            }
            fn rebase_onto(&self, _: &str) -> Result<()> {
                Ok(())
            }
            fn fetch_ref(&self, _: &str) -> Result<()> {
                Ok(())
            }
            fn fetch_and_rebase_onto(&self, _: &str) -> Result<()> {
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
            fn op_in_progress(&self) -> Option<&'static str> {
                None
            }
            fn op_continue(&self) -> Result<()> {
                Ok(())
            }
            fn op_skip(&self) -> Result<()> {
                Ok(())
            }
            fn op_abort(&self) -> Result<()> {
                Ok(())
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
