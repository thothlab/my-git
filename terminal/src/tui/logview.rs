//! Git Log browser (JetBrains-style): a branch tree (local + remote), the
//! commits of the selected branch, the files of the selected commit + its
//! message, and the diff of the selected file in a bottom panel. Branches,
//! commits and files are selectable by keyboard and mouse; the vertical
//! dividers between columns and the horizontal divider above the diff are
//! draggable to resize.

use super::theme::Theme;
use crate::engine::{Commit, CommitFile, GitEngine};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::collections::{BTreeMap, HashSet};

const LOG_LIMIT: usize = 300;

/// What the caller (the App) should do after a key press in the log browser.
pub enum LogAction {
    None,
    Exit,
    Help,
    Revert(String),
    Reset(String),
    /// Checkout the given (already remote-stripped) branch name.
    Checkout(String),
    /// Push the current branch.
    Push,
    /// Create a new branch starting at the given commit.
    NewBranchFrom(String),
    /// Rebase the current branch onto the given branch ref.
    RebaseOnto(String),
    /// Reword (amend the message of) the given commit.
    Reword(String),
    /// Squash the given commit into its parent.
    Squash(String),
    /// Squash the marked set of commits (given newest-first) into one.
    SquashMarked(Vec<String>),
    /// Drop the given commit from the current branch.
    Drop(String),
    /// Cherry-pick the given commit onto the current branch.
    CherryPick(String),
    /// A sequencer op (rebase/cherry-pick) is in progress — drive it (continue/skip/abort).
    OpControl,
    /// Undo the last history rewrite (reword/squash).
    Undo,
    /// Open the stash manager.
    Stashes,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Branches,
    Commits,
    Files,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Divider {
    Col1,
    Col2,
    Horiz,
}

enum RowKind {
    Root { key: String, expanded: bool },
    Folder { key: String, expanded: bool },
    Branch { refname: String, current: bool },
}

struct TreeRow {
    depth: u16,
    label: String,
    kind: RowKind,
}

/// One rectangle set for the log screen, shared by render and hit-testing.
struct LogRects {
    branches: Rect,
    commits: Rect,
    details: Rect,
    diff: Rect,
    v1: u16, // x of the branches|commits divider
    v2: u16, // x of the commits|details divider
    hy: u16, // y of the top|diff divider
}

pub struct LogView {
    local: Vec<String>,
    remote: Vec<String>,
    current: Option<String>,
    collapsed: HashSet<String>,
    rows: Vec<TreeRow>,
    b_cursor: usize,

    selected_branch: Option<String>,
    commits: Vec<Commit>,
    c_cursor: usize,
    /// Commits marked (by hash) for a multi-select squash.
    marked: HashSet<String>,

    files: Vec<CommitFile>,
    f_cursor: usize,
    message: String,
    diff: String,
    diff_scroll: u16,

    focus: Pane,
    col1: u16,
    col2: u16,
    top_pct: u16,
    drag: Option<Divider>,
}

impl LogView {
    pub fn new(engine: &dyn GitEngine) -> Self {
        let local = engine.branches().unwrap_or_default();
        let remote = engine.remote_branches().unwrap_or_default();
        let current = engine.branch_state().ok().and_then(|b| b.current_branch);
        let mut v = LogView {
            local,
            remote,
            current,
            collapsed: HashSet::new(),
            rows: Vec::new(),
            b_cursor: 0,
            selected_branch: None,
            commits: Vec::new(),
            c_cursor: 0,
            marked: HashSet::new(),
            files: Vec::new(),
            f_cursor: 0,
            message: String::new(),
            diff: String::new(),
            diff_scroll: 0,
            focus: Pane::Commits,
            col1: 22,
            col2: 40,
            top_pct: 62,
            drag: None,
        };
        v.rebuild_tree();
        // Select the current branch by default, else the first branch row.
        let start = v.current.clone();
        let idx = v.rows.iter().position(|r| match (&r.kind, &start) {
            (RowKind::Branch { refname, .. }, Some(cur)) => refname == cur,
            _ => false,
        });
        if let Some(i) = idx {
            v.b_cursor = i;
        } else if let Some(i) = v
            .rows
            .iter()
            .position(|r| matches!(r.kind, RowKind::Branch { .. }))
        {
            v.b_cursor = i;
        }
        if let Some(RowKind::Branch { refname, .. }) = v.rows.get(v.b_cursor).map(|r| &r.kind) {
            v.selected_branch = Some(refname.clone());
        }
        v.reload_commits(engine);
        v
    }

    // ----- tree ---------------------------------------------------------------

    fn rebuild_tree(&mut self) {
        let mut rows = Vec::new();
        let cur = self.current.clone().unwrap_or_default();
        build_section(
            &mut rows,
            "Local",
            "local",
            &self.local,
            &self.collapsed,
            &cur,
        );
        build_section(
            &mut rows,
            "Remote",
            "remote",
            &self.remote,
            &self.collapsed,
            &cur,
        );
        self.rows = rows;
        if self.b_cursor >= self.rows.len() {
            self.b_cursor = self.rows.len().saturating_sub(1);
        }
    }

    fn toggle_current_tree_row(&mut self) {
        let key = match self.rows.get(self.b_cursor).map(|r| &r.kind) {
            Some(RowKind::Root { key, .. }) | Some(RowKind::Folder { key, .. }) => {
                Some(key.clone())
            }
            _ => None,
        };
        if let Some(key) = key {
            if !self.collapsed.remove(&key) {
                self.collapsed.insert(key);
            }
            self.rebuild_tree();
        }
    }

    fn activate_branch_row(&mut self, engine: &dyn GitEngine) {
        match self.rows.get(self.b_cursor).map(|r| &r.kind) {
            Some(RowKind::Branch { refname, .. }) => {
                self.selected_branch = Some(refname.clone());
                self.c_cursor = 0;
                self.marked.clear();
                self.focus = Pane::Commits;
                self.reload_commits(engine);
            }
            _ => self.toggle_current_tree_row(),
        }
    }

    // ----- data loads ---------------------------------------------------------

    pub fn reload_commits(&mut self, engine: &dyn GitEngine) {
        self.commits = match &self.selected_branch {
            Some(b) => engine.log_for(b, LOG_LIMIT).unwrap_or_default(),
            None => Vec::new(),
        };
        if self.c_cursor >= self.commits.len() {
            self.c_cursor = self.commits.len().saturating_sub(1);
        }
        self.load_commit(engine);
    }

    fn load_commit(&mut self, engine: &dyn GitEngine) {
        match self.commits.get(self.c_cursor) {
            Some(c) => {
                self.files = engine.commit_files(&c.hash).unwrap_or_default();
                self.message = engine.commit_body(&c.hash).unwrap_or_default();
            }
            None => {
                self.files.clear();
                self.message.clear();
            }
        }
        self.f_cursor = 0;
        self.load_file(engine);
    }

    fn load_file(&mut self, engine: &dyn GitEngine) {
        self.diff_scroll = 0;
        self.diff = match (
            self.commits.get(self.c_cursor),
            self.files.get(self.f_cursor),
        ) {
            (Some(c), Some(f)) => engine
                .commit_file_diff(&c.hash, &f.path)
                .unwrap_or_else(|e| format!("(diff unavailable: {e})")),
            _ => String::new(),
        };
    }

    fn selected_commit_hash(&self) -> Option<String> {
        self.commits.get(self.c_cursor).map(|c| c.hash.clone())
    }

    /// Marked commit hashes in commit-list (newest-first) order.
    fn marked_hashes(&self) -> Vec<String> {
        self.commits
            .iter()
            .filter(|c| self.marked.contains(&c.hash))
            .map(|c| c.hash.clone())
            .collect()
    }

    // ----- input --------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent, engine: &dyn GitEngine) -> LogAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('L') | KeyCode::Char('q') => return LogAction::Exit,
            KeyCode::Char('?') => return LogAction::Help,
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Pane::Branches => Pane::Commits,
                    Pane::Commits => Pane::Files,
                    Pane::Files => Pane::Branches,
                };
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_focus(1, engine),
            KeyCode::Up | KeyCode::Char('k') => self.move_focus(-1, engine),
            KeyCode::Enter => {
                if self.focus == Pane::Branches {
                    self.activate_branch_row(engine);
                }
            }
            KeyCode::Char('v') => {
                if let Some(h) = self.selected_commit_hash() {
                    return LogAction::Revert(h);
                }
            }
            KeyCode::Char('x') => {
                if let Some(h) = self.selected_commit_hash() {
                    return LogAction::Reset(h);
                }
            }
            KeyCode::Char('r') => {
                if let Some(h) = self.selected_commit_hash() {
                    return LogAction::Reword(h);
                }
            }
            KeyCode::Char('b') => {
                if let Some(h) = self.selected_commit_hash() {
                    return LogAction::NewBranchFrom(h);
                }
            }
            KeyCode::Char(' ') => {
                // Toggle a mark on the commit under the cursor (for multi-squash).
                if self.focus == Pane::Commits {
                    if let Some(h) = self.selected_commit_hash() {
                        if !self.marked.remove(&h) {
                            self.marked.insert(h);
                        }
                    }
                }
            }
            KeyCode::Char('s') => {
                if !self.marked.is_empty() {
                    return LogAction::SquashMarked(self.marked_hashes());
                }
                if let Some(h) = self.selected_commit_hash() {
                    return LogAction::Squash(h);
                }
            }
            KeyCode::Char('d') => {
                if let Some(h) = self.selected_commit_hash() {
                    return LogAction::Drop(h);
                }
            }
            KeyCode::Char('C') => {
                if let Some(h) = self.selected_commit_hash() {
                    return LogAction::CherryPick(h);
                }
            }
            KeyCode::Char('u') => return LogAction::Undo,
            KeyCode::Char('S') => return LogAction::Stashes,
            KeyCode::Char('c') => {
                if let Some(t) = self.checkout_target() {
                    return LogAction::Checkout(t);
                }
            }
            KeyCode::Char('R') => {
                // While a sequencer op is stopped, R drives it; otherwise it starts
                // a rebase onto the selected branch.
                if engine.op_in_progress().is_some() {
                    return LogAction::OpControl;
                }
                if let Some((refname, _)) = self.selected_branch_ref() {
                    return LogAction::RebaseOnto(refname);
                }
            }
            KeyCode::Char('P') => return LogAction::Push,
            _ => {}
        }
        LogAction::None
    }

    fn selected_branch_ref(&self) -> Option<(String, bool)> {
        match self.rows.get(self.b_cursor).map(|r| &r.kind) {
            Some(RowKind::Branch { refname, .. }) => {
                let remote = self.remote.iter().any(|r| r == refname);
                Some((refname.clone(), remote))
            }
            _ => None,
        }
    }

    /// The branch to check out: for a remote branch, strip the remote prefix so
    /// `git checkout` creates/switches to the local tracking branch.
    fn checkout_target(&self) -> Option<String> {
        self.selected_branch_ref().map(|(refname, remote)| {
            if remote {
                refname
                    .split_once('/')
                    .map(|(_, rest)| rest.to_string())
                    .unwrap_or(refname)
            } else {
                refname
            }
        })
    }

    fn move_focus(&mut self, delta: i32, engine: &dyn GitEngine) {
        match self.focus {
            Pane::Branches => {
                self.b_cursor = step(self.b_cursor, delta, self.rows.len());
                // Auto-select when landing on a branch (folders need Enter/click).
                if let Some(RowKind::Branch { refname, .. }) =
                    self.rows.get(self.b_cursor).map(|r| &r.kind)
                {
                    if self.selected_branch.as_deref() != Some(refname.as_str()) {
                        self.selected_branch = Some(refname.clone());
                        self.c_cursor = 0;
                        self.marked.clear();
                        self.reload_commits(engine);
                    }
                }
            }
            Pane::Commits => {
                let n = step(self.c_cursor, delta, self.commits.len());
                if n != self.c_cursor {
                    self.c_cursor = n;
                    self.load_commit(engine);
                }
            }
            Pane::Files => {
                let n = step(self.f_cursor, delta, self.files.len());
                if n != self.f_cursor {
                    self.f_cursor = n;
                    self.load_file(engine);
                }
            }
        }
    }

    pub fn handle_mouse(&mut self, m: MouseEvent, area: Rect, engine: &dyn GitEngine) {
        let r = self.layout(area);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let (col, row) = (m.column, m.row);
                if near(col, r.v1) && row < r.hy {
                    self.drag = Some(Divider::Col1);
                } else if near(col, r.v2) && row < r.hy {
                    self.drag = Some(Divider::Col2);
                } else if near(row, r.hy) {
                    self.drag = Some(Divider::Horiz);
                } else if r.diff.contains_pt(col, row) {
                    self.focus = Pane::Files; // diff scrolls with the files pane
                } else if r.branches.contains_pt(col, row) {
                    self.focus = Pane::Branches;
                    self.click_branch(r.branches, row, engine);
                } else if r.commits.contains_pt(col, row) {
                    self.focus = Pane::Commits;
                    self.click_commit(r.commits, row, engine);
                } else if r.details.contains_pt(col, row) {
                    self.focus = Pane::Files;
                    self.click_file(r.details, row, engine);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => self.drag_to(m, area),
            MouseEventKind::Up(_) => self.drag = None,
            MouseEventKind::ScrollDown => self.scroll(m.column, m.row, &r, 1, engine),
            MouseEventKind::ScrollUp => self.scroll(m.column, m.row, &r, -1, engine),
            _ => {}
        }
    }

    fn drag_to(&mut self, m: MouseEvent, area: Rect) {
        match self.drag {
            Some(Divider::Col1) => {
                let pct = pct_of(m.column, area.x, area.width);
                self.col1 = pct.clamp(10, self.col1 + self.col2 - 10);
            }
            Some(Divider::Col2) => {
                let pct = pct_of(m.column, area.x, area.width);
                self.col2 = pct.saturating_sub(self.col1).clamp(10, 100 - self.col1 - 5);
            }
            Some(Divider::Horiz) => {
                let pct = pct_of(m.row, area.y, area.height);
                self.top_pct = pct.clamp(20, 85);
            }
            None => {}
        }
    }

    fn click_branch(&mut self, rect: Rect, row: u16, engine: &dyn GitEngine) {
        if let Some(i) = row_index(rect, row, self.b_cursor, self.rows.len()) {
            self.b_cursor = i;
            self.activate_branch_row(engine);
        }
    }

    fn click_commit(&mut self, rect: Rect, row: u16, engine: &dyn GitEngine) {
        if let Some(i) = row_index(rect, row, self.c_cursor, self.commits.len()) {
            if i != self.c_cursor {
                self.c_cursor = i;
                self.load_commit(engine);
            }
        }
    }

    fn click_file(&mut self, rect: Rect, row: u16, engine: &dyn GitEngine) {
        // The details pane shows the message block, then the files; only the
        // file rows (after the message) are clickable.
        let inner_top = rect.y + 1 + message_lines(&self.message);
        if row < inner_top {
            return;
        }
        let start = viewport_start(self.f_cursor, files_view_height(rect, &self.message));
        let i = start + (row - inner_top) as usize;
        if i < self.files.len() && i != self.f_cursor {
            self.f_cursor = i;
            self.load_file(engine);
        }
    }

    fn scroll(&mut self, col: u16, row: u16, r: &LogRects, delta: i32, engine: &dyn GitEngine) {
        if r.diff.contains_pt(col, row) {
            if delta > 0 {
                self.diff_scroll = self.diff_scroll.saturating_add(1);
            } else {
                self.diff_scroll = self.diff_scroll.saturating_sub(1);
            }
        } else if r.branches.contains_pt(col, row) {
            self.focus = Pane::Branches;
            self.move_focus(delta.signum(), engine);
        } else if r.commits.contains_pt(col, row) {
            self.focus = Pane::Commits;
            self.move_focus(delta.signum(), engine);
        } else if r.details.contains_pt(col, row) {
            self.focus = Pane::Files;
            self.move_focus(delta.signum(), engine);
        }
    }

    // ----- layout + render ----------------------------------------------------

    fn layout(&self, area: Rect) -> LogRects {
        let top_h = (area.height as u32 * self.top_pct as u32 / 100) as u16;
        let top = Rect::new(area.x, area.y, area.width, top_h.min(area.height));
        let diff = Rect::new(
            area.x,
            area.y + top.height,
            area.width,
            area.height.saturating_sub(top.height),
        );
        let w1 = (area.width as u32 * self.col1 as u32 / 100) as u16;
        let w2 = (area.width as u32 * self.col2 as u32 / 100) as u16;
        let branches = Rect::new(top.x, top.y, w1, top.height);
        let commits = Rect::new(top.x + w1, top.y, w2, top.height);
        let details = Rect::new(
            top.x + w1 + w2,
            top.y,
            area.width.saturating_sub(w1 + w2),
            top.height,
        );
        LogRects {
            v1: top.x + w1,
            v2: top.x + w1 + w2,
            hy: area.y + top.height,
            branches,
            commits,
            details,
            diff,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect, t: &Theme) {
        let r = self.layout(area);
        self.render_branches(f, r.branches, t);
        self.render_commits(f, r.commits, t);
        self.render_details(f, r.details, t);
        self.render_diff(f, r.diff, t);
    }

    fn render_branches(&self, f: &mut Frame, area: Rect, t: &Theme) {
        let block = pane_block("BRANCHES", self.focus == Pane::Branches, t);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let h = inner.height as usize;
        let start = viewport_start(self.b_cursor, h);
        let mut lines = Vec::new();
        for (i, row) in self.rows.iter().enumerate().skip(start).take(h) {
            let indent = "  ".repeat(row.depth as usize);
            let (glyph, style) = match &row.kind {
                RowKind::Root { expanded, .. } => (
                    if *expanded { "▾" } else { "▸" },
                    Style::default().fg(t.fg_muted).add_modifier(Modifier::BOLD),
                ),
                RowKind::Folder { expanded, .. } => (
                    if *expanded { "▾" } else { "▸" },
                    Style::default().fg(t.accent),
                ),
                RowKind::Branch { current, .. } => (
                    if *current { "★" } else { "⎇" },
                    if *current {
                        Style::default().fg(t.success).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(t.fg)
                    },
                ),
            };
            let mut line = Line::from(vec![Span::styled(
                format!("{indent}{glyph} {}", row.label),
                style,
            )]);
            if i == self.b_cursor {
                line = line.style(Style::default().bg(t.sel_bg));
            }
            lines.push(line);
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn render_commits(&self, f: &mut Frame, area: Rect, t: &Theme) {
        let title = match &self.selected_branch {
            Some(b) => format!("COMMITS: {b}"),
            None => "COMMITS".to_string(),
        };
        let block = pane_block(&title, self.focus == Pane::Commits, t);
        let inner = block.inner(area);
        f.render_widget(block, area);
        if self.commits.is_empty() {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "  no commits",
                    Style::default().fg(t.fg_muted),
                )),
                inner,
            );
            return;
        }
        let h = inner.height as usize;
        let start = viewport_start(self.c_cursor, h);
        let mut lines = Vec::new();
        for (i, c) in self.commits.iter().enumerate().skip(start).take(h) {
            let short = &c.hash[..c.hash.len().min(8)];
            let marked = self.marked.contains(&c.hash);
            let mut spans = vec![
                Span::styled(
                    if marked { "● " } else { "  " },
                    Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{short} "), Style::default().fg(t.warn)),
                Span::styled(c.summary.clone(), Style::default().fg(t.fg)),
            ];
            if !c.refs.is_empty() {
                spans.push(Span::styled(
                    format!("  ({})", c.refs.join(", ")),
                    Style::default().fg(t.accent),
                ));
            }
            spans.push(Span::styled(
                format!("  — {}", c.author),
                Style::default().fg(t.fg_muted),
            ));
            let mut line = Line::from(spans);
            if i == self.c_cursor && self.focus == Pane::Commits {
                line = line.style(Style::default().bg(t.sel_bg));
            }
            lines.push(line);
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn render_details(&self, f: &mut Frame, area: Rect, t: &Theme) {
        let block = pane_block("COMMIT", self.focus == Pane::Files, t);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let mut lines: Vec<Line> = Vec::new();
        // message (a few lines)
        for l in self
            .message
            .lines()
            .take(message_lines(&self.message) as usize)
        {
            lines.push(Line::from(Span::styled(
                l.to_string(),
                Style::default().fg(t.fg_muted),
            )));
        }
        // files
        let h = inner.height as usize;
        let msg_h = message_lines(&self.message) as usize;
        let file_h = h.saturating_sub(msg_h);
        let start = viewport_start(self.f_cursor, file_h);
        for (i, cf) in self.files.iter().enumerate().skip(start).take(file_h) {
            let color = status_color(cf.status, t);
            let mut line = Line::from(vec![
                Span::styled(format!("{} ", cf.status), Style::default().fg(color)),
                Span::styled(cf.path.clone(), Style::default().fg(t.fg)),
            ]);
            if i == self.f_cursor && self.focus == Pane::Files {
                line = line.style(Style::default().bg(t.sel_bg));
            }
            lines.push(line);
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn render_diff(&self, f: &mut Frame, area: Rect, t: &Theme) {
        let title = match self.files.get(self.f_cursor) {
            Some(cf) => format!("DIFF: {}", cf.path),
            None => "DIFF".to_string(),
        };
        let block = pane_block(&title, false, t);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let lines: Vec<Line> = self.diff.lines().map(|l| diff_line(l, t)).collect();
        f.render_widget(Paragraph::new(lines).scroll((self.diff_scroll, 0)), inner);
    }
}

// ----- free helpers -----------------------------------------------------------

fn build_section(
    rows: &mut Vec<TreeRow>,
    title: &str,
    root_key: &str,
    names: &[String],
    collapsed: &HashSet<String>,
    current: &str,
) {
    let expanded = !collapsed.contains(root_key);
    rows.push(TreeRow {
        depth: 0,
        label: title.to_string(),
        kind: RowKind::Root {
            key: root_key.to_string(),
            expanded,
        },
    });
    if !expanded {
        return;
    }
    let mut root: BTreeMap<String, TNode> = BTreeMap::new();
    for n in names {
        let segs: Vec<&str> = n.split('/').collect();
        insert(&mut root, &segs, n);
    }
    emit(rows, &root, 1, root_key, collapsed, current);
}

enum TNode {
    Dir(BTreeMap<String, TNode>),
    Leaf(String),
}

fn insert(map: &mut BTreeMap<String, TNode>, segs: &[&str], refname: &str) {
    if segs.is_empty() {
        return;
    }
    if segs.len() == 1 {
        map.insert(segs[0].to_string(), TNode::Leaf(refname.to_string()));
        return;
    }
    let entry = map
        .entry(segs[0].to_string())
        .or_insert_with(|| TNode::Dir(BTreeMap::new()));
    if let TNode::Dir(child) = entry {
        insert(child, &segs[1..], refname);
    }
}

fn emit(
    rows: &mut Vec<TreeRow>,
    map: &BTreeMap<String, TNode>,
    depth: u16,
    prefix: &str,
    collapsed: &HashSet<String>,
    current: &str,
) {
    for (name, node) in map {
        match node {
            TNode::Leaf(refname) => rows.push(TreeRow {
                depth,
                label: name.clone(),
                kind: RowKind::Branch {
                    refname: refname.clone(),
                    current: refname == current,
                },
            }),
            TNode::Dir(child) => {
                let key = format!("{prefix}/{name}");
                let expanded = !collapsed.contains(&key);
                rows.push(TreeRow {
                    depth,
                    label: name.clone(),
                    kind: RowKind::Folder {
                        key: key.clone(),
                        expanded,
                    },
                });
                if expanded {
                    emit(rows, child, depth + 1, &key, collapsed, current);
                }
            }
        }
    }
}

fn pane_block(title: &str, focused: bool, t: &Theme) -> Block<'static> {
    let border = if focused { t.accent } else { t.fg_muted };
    Block::default()
        .title(Line::from(format!(" {title} ")))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
}

fn diff_line(l: &str, t: &Theme) -> Line<'static> {
    let style = if l.starts_with("+++") || l.starts_with("---") {
        Style::default().fg(t.fg_muted).add_modifier(Modifier::BOLD)
    } else if l.starts_with("@@") {
        Style::default().fg(t.accent)
    } else if l.starts_with('+') {
        Style::default().fg(t.success)
    } else if l.starts_with('-') {
        Style::default().fg(t.danger)
    } else {
        Style::default().fg(t.fg)
    };
    Line::from(Span::styled(l.to_string(), style))
}

fn status_color(status: char, t: &Theme) -> ratatui::style::Color {
    match status {
        'A' => t.success,
        'D' => t.danger,
        'R' | 'C' => t.accent,
        _ => t.warn,
    }
}

/// How many message lines the details pane reserves at the top.
fn message_lines(message: &str) -> u16 {
    (message.lines().count() as u16).min(4)
}

fn files_view_height(details: Rect, message: &str) -> usize {
    let inner = details.height.saturating_sub(2) as usize; // borders
    inner.saturating_sub(message_lines(message) as usize)
}

fn viewport_start(cursor: usize, height: usize) -> usize {
    cursor.saturating_sub(height.saturating_sub(1))
}

/// Map a click row inside a bordered list pane to a row index.
fn row_index(rect: Rect, row: u16, cursor: usize, len: usize) -> Option<usize> {
    let inner_top = rect.y + 1;
    if row < inner_top {
        return None;
    }
    let h = rect.height.saturating_sub(2) as usize;
    let start = viewport_start(cursor, h);
    let i = start + (row - inner_top) as usize;
    (i < len).then_some(i)
}

fn step(cursor: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let max = len - 1;
    if delta > 0 {
        (cursor + 1).min(max)
    } else {
        cursor.saturating_sub(1)
    }
}

fn near(a: u16, b: u16) -> bool {
    (a as i32 - b as i32).abs() <= 1
}

fn pct_of(pos: u16, origin: u16, span: u16) -> u16 {
    if span == 0 {
        return 50;
    }
    ((pos.saturating_sub(origin)) as u32 * 100 / span as u32) as u16
}

trait ContainsPt {
    fn contains_pt(&self, col: u16, row: u16) -> bool;
}
impl ContainsPt for Rect {
    fn contains_pt(&self, col: u16, row: u16) -> bool {
        col >= self.x && col < self.x + self.width && row >= self.y && row < self.y + self.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_builds_folders_and_leaves() {
        let mut rows = Vec::new();
        let local = vec![
            "main".to_string(),
            "feature/x".to_string(),
            "feature/y".to_string(),
        ];
        build_section(&mut rows, "Local", "local", &local, &HashSet::new(), "main");
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        // BTreeMap order: the "feature" folder (with x, y) sorts before "main".
        assert_eq!(labels, vec!["Local", "feature", "x", "y", "main"]);
        assert_eq!(rows[1].depth, 1); // feature folder under the root
        assert_eq!(rows[2].depth, 2); // x under feature
        let main = rows.iter().find(|r| r.label == "main").unwrap();
        assert!(matches!(main.kind, RowKind::Branch { current: true, .. }));
    }

    #[test]
    fn collapsed_folder_hides_children() {
        let mut rows = Vec::new();
        let local = vec!["feature/x".to_string()];
        let mut collapsed = HashSet::new();
        collapsed.insert("local/feature".to_string());
        build_section(&mut rows, "Local", "local", &local, &collapsed, "");
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["Local", "feature"]); // "x" is hidden
    }
}
