//! Minimal render loop (Task 01 smoke). Proves the ratatui/crossterm alt-screen
//! lifecycle: enter, draw a placeholder frame, quit cleanly on `q`/Esc/Ctrl-C.
//! The full panelled shell (layout, focus, status bar, keymap) is Task 03.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::path::Path;

/// Run the placeholder TUI until the user quits.
pub fn run(repo_root: &Path) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, repo_root);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, repo_root: &Path) -> Result<()> {
    loop {
        terminal.draw(|frame| draw(frame, repo_root))?;
        if let Event::Key(key) = event::read()? {
            let quit = matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL));
            if quit {
                return Ok(());
            }
        }
    }
}

fn draw(frame: &mut Frame, repo_root: &Path) {
    let title = format!(" mygit — {} ", repo_root.display());
    let body = Paragraph::new(vec![
        Line::from(Span::styled(
            "TUI git manager with changelists",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Skeleton render loop (Task 01). Press q to quit."),
    ])
    .block(Block::default().title(title).borders(Borders::ALL));
    frame.render_widget(body, frame.area());
}
