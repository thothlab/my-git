//! Rendering for the TUI shell: status bar, Changes panel (grouped by
//! changelist), diff/detail panel, footer hints, and the help overlay. Pure
//! read of `App` state; all colours come from the role-based `Theme`.

use super::keymap::BINDINGS;
use super::theme::Theme;
use super::{
    App, CommandsState, ConfirmState, Focus, InputState, MenuState, Overlay, PickerState, Row,
    StashState,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// The four screen regions: `[status, changes, detail, footer]`. Shared by the
/// renderer and the mouse hit-testing so clicks/drags map to what's drawn.
pub(super) fn regions(area: Rect, split_pct: u16) -> [Rect; 4] {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(split_pct),
            Constraint::Percentage(100 - split_pct),
        ])
        .split(rows[1]);
    [rows[0], cols[0], cols[1], rows[2]]
}

pub fn render(f: &mut Frame, app: &App<'_>) {
    let area = f.area();
    if let Some(log) = &app.log {
        // Log browser mode: status bar + full-width browser + footer.
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(area);
        render_status_bar(f, app, rows[0]);
        log.render(f, rows[1], &app.theme);
        render_footer(f, app, rows[2]);
    } else {
        let [status, changes, detail, footer] = regions(area, app.split_pct);
        render_status_bar(f, app, status);
        render_changes(f, app, changes);
        render_detail(f, app, detail);
        render_footer(f, app, footer);
    }
    match &app.overlay {
        Overlay::Help(cursor) => render_help(f, app, area, *cursor),
        Overlay::Input(s) => render_input(f, app, area, s),
        Overlay::Picker(p) => render_picker(f, app, area, p),
        Overlay::Confirm(c) => render_confirm(f, app, area, c),
        Overlay::Stashes(s) => render_stashes(f, app, area, s),
        Overlay::Commands(c) => render_commands(f, app, area, c),
        Overlay::Menu(mn) => render_menu(f, app, area, mn),
        Overlay::None => {}
    }
    // A busy op paints over everything so a long git call doesn't look frozen.
    if let Some(label) = &app.busy {
        render_busy(f, app, area, label);
    }
}

fn render_busy(f: &mut Frame, app: &App<'_>, area: Rect, label: &str) {
    let t = &app.theme;
    let text = format!("⏳ {label}");
    let w = (text.chars().count() as u16 + 4)
        .max(30)
        .min(area.width.saturating_sub(2));
    let rect = centered(area, w, 4);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(Line::from(" Working "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let lines = vec![
        Line::from(Span::styled(
            format!(" {text}"),
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            " large repos take a moment…",
            Style::default().fg(t.fg_muted),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

/// The rectangle a menu occupies — shared by the renderer and mouse hit-testing.
/// Anchored menus (context menus) open at the click and are clamped on-screen;
/// an anchorless menu (the palette) is centred.
pub(super) fn menu_rect(area: Rect, mn: &MenuState) -> Rect {
    let content_w = mn
        .items
        .iter()
        .map(|i| i.label.chars().count())
        .chain(std::iter::once(mn.title.chars().count()))
        .chain(mn.footer.iter().map(|f| f.chars().count()))
        .max()
        .unwrap_or(10) as u16;
    let w = (content_w + 4)
        .clamp(16, area.width.saturating_sub(2))
        .min(area.width);
    let footer_h = if mn.footer.is_some() { 1 } else { 0 };
    let h = (mn.items.len() as u16 + 2 + footer_h).min(area.height);
    match mn.anchor {
        Some((cx, cy)) => {
            let x = cx.min(area.x + area.width.saturating_sub(w));
            let y = cy.min(area.y + area.height.saturating_sub(h));
            Rect::new(x, y, w, h)
        }
        None => centered(area, w, h),
    }
}

fn render_menu(f: &mut Frame, app: &App<'_>, area: Rect, mn: &MenuState) {
    let t = &app.theme;
    let rect = menu_rect(area, mn);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(Line::from(format!(" {} ", mn.title)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let mut lines: Vec<Line> = mn
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let mut line = Line::from(format!(" {}", item.label));
            if i == mn.cursor {
                line = line.style(Style::default().bg(t.sel_bg).fg(t.accent));
            }
            line
        })
        .collect();
    if let Some(footer) = &mn.footer {
        lines.push(Line::from(Span::styled(
            format!(" {footer}"),
            Style::default().fg(t.fg_muted),
        )));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// Git command log: a large, scrollable console of recent `git` invocations
/// (newest first) with failures highlighted and their stderr expanded.
fn render_commands(f: &mut Frame, app: &App<'_>, area: Rect, c: &CommandsState) {
    let t = &app.theme;
    let w = area.width.saturating_sub(6).min(110);
    let h = area.height.saturating_sub(4);
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);
    let fail_count = c.entries.iter().filter(|e| !e.ok).count();
    let title = if c.failures_only {
        format!(" Git command log — failures only ({fail_count}) ")
    } else {
        format!(
            " Git command log — {} commands, {fail_count} failed ",
            c.entries.len()
        )
    };
    let block = Block::default()
        .title(Line::from(title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    // Reserve the last row for the hint bar.
    let body_h = inner.height.saturating_sub(1);
    let mut lines: Vec<Line> = Vec::new();
    for e in c.entries.iter().filter(|e| !c.failures_only || !e.ok) {
        let (glyph, cmd_style) = if e.ok {
            ("✓", Style::default().fg(t.fg_muted))
        } else {
            (
                "✗",
                Style::default().fg(t.danger).add_modifier(Modifier::BOLD),
            )
        };
        let code = match e.code {
            Some(n) if !e.ok => format!("  [exit {n}]"),
            _ => String::new(),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{glyph} "), cmd_style),
            Span::styled(format!("git {}", e.command), cmd_style),
            Span::styled(code, Style::default().fg(t.danger)),
        ]));
        if !e.ok && !e.stderr.is_empty() {
            for sl in e.stderr.lines() {
                lines.push(Line::from(Span::styled(
                    format!("    {sl}"),
                    Style::default().fg(t.danger),
                )));
            }
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            if c.failures_only {
                "  no failed commands"
            } else {
                "  no git commands recorded yet"
            },
            Style::default().fg(t.fg_muted),
        )));
    }
    let max_scroll = (lines.len() as u16).saturating_sub(body_h);
    let scroll = c.scroll.min(max_scroll);
    let body = Rect::new(inner.x, inner.y, inner.width, body_h);
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), body);

    let hint = " j/k scroll · f failures only · Esc/g close";
    let hint_row = Rect::new(inner.x, inner.y + body_h, inner.width, 1);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(t.fg_muted),
        ))),
        hint_row,
    );
}

fn render_stashes(f: &mut Frame, app: &App<'_>, area: Rect, s: &StashState) {
    let t = &app.theme;
    let w = 66.min(area.width.saturating_sub(4));
    let h = (s.items.len() as u16 + 4).min(area.height.saturating_sub(2));
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(Line::from(" Stashes "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let mut lines: Vec<Line> = s
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let mut line = Line::from(format!("  {}", item.label));
            if i == s.cursor {
                line = line.style(Style::default().bg(t.sel_bg).fg(t.accent));
            }
            line
        })
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Enter/p pop · a apply · d drop · Esc close",
        Style::default().fg(t.fg_muted),
    )));
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_status_bar(f: &mut Frame, app: &App<'_>, area: Rect) {
    let t = &app.theme;
    let b = &app.branch;
    let branch = b
        .current_branch
        .clone()
        .unwrap_or_else(|| "(no branch)".into());
    let mut spans = vec![Span::styled(
        format!(" {branch} "),
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
    )];
    if let Some(rb) = &b.rebase {
        spans.push(Span::styled(
            format!("⟳ rebase {}/{} ", rb.current, rb.total),
            Style::default().fg(t.danger),
        ));
    }
    if b.detached {
        spans.push(Span::styled("detached ", Style::default().fg(t.danger)));
    }
    spans.push(Span::styled(
        format!("↑{} ↓{} ", b.ahead, b.behind),
        Style::default().fg(if b.behind > 0 { t.warn } else { t.success }),
    ));
    spans.push(Span::styled(
        format!("{} changed  ", app.status_map.len()),
        Style::default().fg(t.fg_muted),
    ));
    spans.push(Span::styled("[? help]", Style::default().fg(t.fg_muted)));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_changes(f: &mut Frame, app: &App<'_>, area: Rect) {
    let t = &app.theme;
    let focused = app.focus == Focus::Changes;
    let block = panel_block("CHANGES", focused, t);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.status_map.is_empty() {
        let hint = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No changes in the working tree.",
                Style::default().fg(t.fg_muted),
            )),
            Line::from(Span::styled(
                "  Edit files, then press F5 to refresh.",
                Style::default().fg(t.fg_muted),
            )),
        ]);
        f.render_widget(hint, inner);
        return;
    }

    let height = inner.height as usize;
    let start = app.cursor.saturating_sub(height.saturating_sub(1));
    let mut lines = Vec::new();
    for (i, row) in app.rows.iter().enumerate().skip(start).take(height) {
        let selected = i == app.cursor && focused;
        let mut line = match row {
            Row::Header { list } => {
                let cl = &app.store.changelists[*list];
                let count = cl
                    .files
                    .iter()
                    .filter(|p| app.status_map.contains_key(*p))
                    .count();
                // The auto Unversioned Files list is tinted to read as special.
                let color = if cl.id == crate::changelists::UNVERSIONED_ID {
                    t.warn
                } else {
                    t.accent
                };
                Line::from(Span::styled(
                    format!("▸ {} ({count})", cl.name),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ))
            }
            Row::File { path, status, .. } => {
                let mark = if app.marked.contains(path) { "*" } else { " " };
                Line::from(vec![
                    Span::raw(format!("  {mark}")),
                    Span::styled(
                        format!("{} ", status.letter()),
                        Style::default().fg(t.status_color(*status)),
                    ),
                    Span::styled(path.clone(), Style::default().fg(t.fg)),
                ])
            }
        };
        if selected {
            line = line.style(Style::default().bg(t.sel_bg));
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_detail(f: &mut Frame, app: &App<'_>, area: Rect) {
    let t = &app.theme;
    let focused = app.focus == Focus::Detail;
    let title = match &app.diff_path {
        Some(p) => format!("DIFF: {p}"),
        None => "DIFF".to_string(),
    };
    let block = panel_block(&title, focused, t);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.diff.is_empty() {
        let hint = Paragraph::new(Span::styled(
            "  select a file to see its diff",
            Style::default().fg(t.fg_muted),
        ));
        f.render_widget(hint, inner);
        return;
    }
    let lines: Vec<Line> = app.diff.lines().map(|l| diff_line(l, t)).collect();
    f.render_widget(Paragraph::new(lines).scroll((app.diff_scroll, 0)), inner);
}

fn render_confirm(f: &mut Frame, app: &App<'_>, area: Rect, c: &ConfirmState) {
    let t = &app.theme;
    let w = 56.min(area.width.saturating_sub(4));
    let rect = centered(area, w, 6);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(Line::from(format!(" {} ", c.title)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.danger));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", c.body),
            Style::default().fg(t.fg),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  [y] confirm    [n / Esc] cancel (default)",
            Style::default().fg(t.fg_muted),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
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

fn render_footer(f: &mut Frame, app: &App<'_>, area: Rect) {
    let t = &app.theme;
    let hints = if app.log.is_some() {
        "[space]mark [s]squash [d]drop [C]pick [r]reword [c]checkout [P]push [R]rebase/resolve [b]branch [u]undo [g]git-log [?]help [L/Esc]back"
    } else {
        "[n]new [m]move [space]mark [c]commit [u]rollback [P]push [L]log [R]rebase [S]stash [g]git-log [?]help [q]quit"
    };
    let line = if app.message.is_empty() {
        Line::from(Span::styled(hints, Style::default().fg(t.fg_muted)))
    } else {
        Line::from(Span::styled(
            format!(" {} ", app.message),
            Style::default().fg(t.warn),
        ))
    };
    f.render_widget(Paragraph::new(line), area);
}

fn render_help(f: &mut Frame, app: &App<'_>, area: Rect, cursor: usize) {
    let t = &app.theme;
    let w = 48.min(area.width.saturating_sub(4));
    let h = (BINDINGS.len() as u16 + 4).min(area.height.saturating_sub(2));
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(Line::from(" Help — key bindings "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let mut lines: Vec<Line> = BINDINGS
        .iter()
        .enumerate()
        .map(|(i, (key, action))| {
            let mut line = Line::from(vec![
                Span::styled(format!(" {key:<10}"), Style::default().fg(t.accent)),
                Span::styled(action.label(), Style::default().fg(t.fg)),
            ]);
            if i == cursor {
                line = line.style(Style::default().bg(t.sel_bg));
            }
            line
        })
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ↑↓ select · Enter run · Esc close",
        Style::default().fg(t.fg_muted),
    )));
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_input(f: &mut Frame, app: &App<'_>, area: Rect, s: &InputState) {
    let t = &app.theme;
    let w = 50.min(area.width.saturating_sub(4));
    let rect = centered(area, w, 5);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(Line::from(format!(" {} ", s.title)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let lines = vec![
        Line::from(vec![
            Span::styled("> ", Style::default().fg(t.accent)),
            Span::styled(format!("{}\u{2588}", s.value), Style::default().fg(t.fg)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " Enter: confirm   Esc: cancel",
            Style::default().fg(t.fg_muted),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_picker(f: &mut Frame, app: &App<'_>, area: Rect, p: &PickerState) {
    let t = &app.theme;
    let w = 46.min(area.width.saturating_sub(4));
    let h = (p.items.len() as u16 + 4).min(area.height.saturating_sub(2));
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(Line::from(format!(" {} ", p.title)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let mut lines: Vec<Line> = p
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let mut line = Line::from(format!("  {}", item.label));
            if i == p.cursor {
                line = line.style(Style::default().bg(t.sel_bg).fg(t.accent));
            }
            line
        })
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ↑↓ select   Enter: confirm   Esc: cancel",
        Style::default().fg(t.fg_muted),
    )));
    f.render_widget(Paragraph::new(lines), inner);
}

fn panel_block(title: &str, focused: bool, t: &Theme) -> Block<'static> {
    let border = if focused { t.accent } else { t.fg_muted };
    Block::default()
        .title(Line::from(format!(" {title} ")))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}
