//! Color palette and style constants for the radio TUI.

use ratatui::style::{Color, Modifier, Style};

// ── Color palette ─────────────────────────────────────────────────────────────

pub const C_BG: Color = Color::Rgb(18, 18, 18);
pub const C_ACCENT: Color = Color::Rgb(255, 95, 95);
pub const C_PLAYING: Color = Color::Rgb(80, 200, 120);
pub const C_CONNECTING: Color = Color::Rgb(255, 184, 80);
pub const C_ERROR: Color = Color::Rgb(255, 80, 80);
pub const C_MUTED: Color = Color::Rgb(72, 72, 88);
pub const C_SEPARATOR: Color = Color::Rgb(40, 40, 52);
pub const C_SECONDARY: Color = Color::Rgb(115, 115, 138);
pub const C_PRIMARY: Color = Color::Rgb(210, 210, 225);
pub const C_SELECTION_BG: Color = Color::Rgb(28, 28, 40);
pub const C_PANEL_BORDER: Color = Color::Rgb(40, 40, 52);
pub const C_PANEL_BORDER_FOCUSED: Color = Color::Rgb(120, 100, 200); // vibrant purple — clear focus indicator
pub const C_NUMBER_HINT: Color = Color::Rgb(90, 90, 115); // brighter than border, dimmer than secondary
pub const C_FILTER_BG: Color = Color::Rgb(20, 20, 32);
pub const C_FILTER_FG: Color = Color::Rgb(255, 200, 80);
pub const C_TAG: Color = Color::Rgb(80, 140, 200);
pub const C_LOCATION: Color = Color::Rgb(100, 160, 130);
pub const C_NETWORK: Color = Color::Rgb(180, 120, 220);
pub const C_TOAST_INFO: Color = Color::Rgb(80, 160, 220);
pub const C_TOAST_SUCCESS: Color = Color::Rgb(80, 200, 120);
pub const C_TOAST_WARNING: Color = Color::Rgb(255, 184, 80);
pub const C_TOAST_ERROR: Color = Color::Rgb(255, 95, 95);
pub const C_BADGE_LIVE: Color = Color::Rgb(80, 200, 120);
pub const C_BADGE_ERR: Color = Color::Rgb(255, 95, 95);
pub const C_BADGE_PENDING: Color = Color::Rgb(255, 184, 80);
pub const C_MODE_NORMAL: Color = Color::Rgb(115, 115, 138);
pub const C_MODE_FILTER: Color = Color::Rgb(255, 200, 80);
pub const C_MODE_COMMAND: Color = Color::Rgb(255, 95, 95);
pub const C_STARS: Color = Color::Rgb(255, 210, 50);

// ── Predefined styles ─────────────────────────────────────────────────────────

pub fn style_default() -> Style {
    Style::default().fg(C_PRIMARY)
}

pub fn style_secondary() -> Style {
    Style::default().fg(C_SECONDARY)
}

pub fn style_accent() -> Style {
    Style::default().fg(C_ACCENT)
}

pub fn style_playing() -> Style {
    Style::default().fg(C_PLAYING)
}

pub fn style_selected() -> Style {
    Style::default().bg(C_SELECTION_BG).fg(C_PRIMARY)
}

pub fn style_selected_focused() -> Style {
    Style::default()
        .bg(C_SELECTION_BG)
        .fg(C_PRIMARY)
        .add_modifier(Modifier::BOLD)
}

pub fn style_focused_border() -> Style {
    Style::default().fg(C_PANEL_BORDER_FOCUSED)
}

pub fn style_unfocused_border() -> Style {
    Style::default().fg(C_PANEL_BORDER)
}

pub fn style_filter() -> Style {
    Style::default().fg(C_FILTER_FG).bg(C_FILTER_BG)
}

pub fn style_muted() -> Style {
    Style::default().fg(C_MUTED)
}
