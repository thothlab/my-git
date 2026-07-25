//! Rendering for the TUI shell: status bar, Changes panel (grouped by
//! changelist), diff/detail panel, footer hints, and the help overlay. Pure
//! read of `App` state; all colours come from the role-based `Theme`.

use super::keymap::BINDINGS;
use super::theme::Theme;
use super::{App, ConfirmState, Focus, InputState, Overlay, PickerState, Row};
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
        Overlay::None => {}
    }
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
        "[Tab]pane [j/k]nav [Enter]branch/folder [v]revert [x]reset drag dividers · [L/Esc]back"
    } else {
        "[n]new [m]move [space]mark [c]commit [u]rollback [P]push [L]log [R]rebase [?]help [q]quit"
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
