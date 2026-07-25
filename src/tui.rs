//! Minimal render loop (Task 01 smoke). Proves the ratatui/crossterm alt-screen
//! lifecycle: enter, draw a placeholder frame, quit cleanly on `q`/Esc/Ctrl-C.
//! The full panelled shell (layout, focus, status bar, keymap) is Task 03.

use crate::changelists::ChangelistStore;
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::path::Path;

/// Run the placeholder TUI until the user quits. The full panelled shell
/// (layout, focus, status bar, keymap) is Task 03; the grouped changes panel is
/// Task 04. This proves the live pipeline: discovered repo + reconciled store.
pub fn run(repo_root: &Path, store: &ChangelistStore) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, repo_root, store);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    repo_root: &Path,
    store: &ChangelistStore,
) -> Result<()> {
    loop {
        terminal.draw(|frame| draw(frame, repo_root, store))?;
        if let Event::Key(key) = event::read()? {
            let quit = matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL));
            if quit {
                return Ok(());
            }
        }
    }
}

fn draw(frame: &mut Frame, repo_root: &Path, store: &ChangelistStore) {
    let title = format!(" mygit — {} ", repo_root.display());
    let total: usize = store.changelists.iter().map(|c| c.files.len()).sum();
    let active = store
        .changelists
        .iter()
        .find(|c| c.id == store.active_changelist_id)
        .map(|c| c.name.as_str())
        .unwrap_or("Default");

    let mut lines = vec![
        Line::from(Span::styled(
            "TUI git manager with changelists",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "{} changed file(s) across {} changelist(s) — active: {}",
            total,
            store.changelists.len(),
            active
        )),
        Line::from(""),
    ];
    for cl in &store.changelists {
        lines.push(Line::from(format!("▸ {} ({})", cl.name, cl.files.len())));
    }
    lines.push(Line::from(""));
    lines.push(Line::from("Skeleton (Tasks 01–02 wired). Press q to quit."));

    let body = Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL));
    frame.render_widget(body, frame.area());
}
