//! Role-based theme (pattern from gwm-cli `tui/theme.rs`) mapping Pane semantic
//! tokens (ТЗ §7) to terminal colours. The **default respects the user's
//! terminal palette** by using named ANSI colours and never hardcoding a
//! background; `pane()` is an optional truecolor preset. Every visual signal
//! reads a role here rather than a literal `Color`.

use crate::engine::FileStatus;
use ratatui::style::Color;

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// Pane `accent` (brand blue): focus, selection, focused-panel title, renamed.
    pub accent: Color,
    /// Pane `success` (green): Added / staged / resolved / "ahead ok".
    pub success: Color,
    /// Pane `warn` (yellow): Modified / Untracked / ahead-behind.
    pub warn: Color,
    /// Pane `danger` (red): Deleted / Conflict / destructive actions.
    pub danger: Color,
    /// Normal foreground (uses the terminal default).
    pub fg: Color,
    /// Muted foreground: paths meta, hints.
    pub fg_muted: Color,
    /// Selected-row background.
    pub sel_bg: Color,
}

impl Default for Theme {
    fn default() -> Self {
        // Named ANSI → the terminal's own palette (honours the user's theme).
        Self {
            accent: Color::Blue,
            success: Color::Green,
            warn: Color::Yellow,
            danger: Color::Red,
            fg: Color::Reset,
            fg_muted: Color::DarkGray,
            sel_bg: Color::Indexed(238),
        }
    }
}

impl Theme {
    /// Truecolor preset approximating Pane brand tokens. Exact hex to be
    /// confirmed from `pane-app` `src/styles/tokens.css` (no local checkout).
    #[allow(dead_code)]
    pub fn pane() -> Self {
        Self {
            accent: Color::Rgb(0x3b, 0x82, 0xf6),
            success: Color::Rgb(0x22, 0xc5, 0x5e),
            warn: Color::Rgb(0xf5, 0x9e, 0x0b),
            danger: Color::Rgb(0xef, 0x44, 0x44),
            fg: Color::Reset,
            fg_muted: Color::Rgb(0x94, 0xa3, 0xb8),
            sel_bg: Color::Rgb(0x1e, 0x29, 0x3b),
        }
    }

    /// Colour for a file-status marker.
    pub fn status_color(&self, s: FileStatus) -> Color {
        match s {
            FileStatus::Added => self.success,
            FileStatus::Modified | FileStatus::Untracked => self.warn,
            FileStatus::Deleted | FileStatus::Conflicted => self.danger,
            FileStatus::Renamed => self.accent,
        }
    }
}
