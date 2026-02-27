//! Status bar — bottom line with connection state, mode, and keybindings.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::theme::{
    C_ACCENT, C_MODE_COMMAND, C_MODE_FILTER, C_MODE_NORMAL, C_MUTED, C_PLAYING, C_SECONDARY,
    C_SEPARATOR,
};

/// Map RMS dBFS to bulb brightness (fixed hue, variable intensity only).
fn bulb_color(audio_level_db: f32) -> Color {
    const FLOOR: f32 = -90.0;
    const CEIL: f32 = -3.0;
    let t = ((audio_level_db - FLOOR) / (CEIL - FLOOR))
        .clamp(0.0, 1.0)
        .powf(0.72);
    let scale = 0.45 + 1.35 * t;
    Color::Rgb(
        (82.0 * scale).round().min(255.0) as u8,
        (82.0 * scale).round().min(255.0) as u8,
        (102.0 * scale).round().min(255.0) as u8,
    )
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputMode {
    Normal,
    Filter,
    Command,
}

impl InputMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Filter => "FILTER",
            Self::Command => "COMMAND",
        }
    }

    pub fn color(self) -> ratatui::style::Color {
        match self {
            Self::Normal => C_MODE_NORMAL,
            Self::Filter => C_MODE_FILTER,
            Self::Command => C_MODE_COMMAND,
        }
    }
}

/// Draw the log bar: last log line.
pub fn draw_log_bar(frame: &mut Frame, area: Rect, last_log: Option<&str>, connected: bool) {
    let conn_span = if connected {
        Span::styled("●", Style::default().fg(C_PLAYING))
    } else {
        Span::styled("○", Style::default().fg(C_ACCENT))
    };

    let log_span = Span::styled(last_log.unwrap_or(""), Style::default().fg(C_SECONDARY));

    let line = Line::from(vec![conn_span, Span::raw(" "), log_span]);
    frame.render_widget(Paragraph::new(line), area);
}

/// Draw a horizontal separator line.
pub fn draw_separator(frame: &mut Frame, area: Rect) {
    let line = Line::from(Span::styled(
        "─".repeat(area.width as usize),
        Style::default().fg(C_SEPARATOR),
    ));
    frame.render_widget(Paragraph::new(line), area);
}

/// Draw the keybindings footer bar (one row).
pub fn draw_keys_bar(
    frame: &mut Frame,
    area: Rect,
    mode: InputMode,
    workspace: crate::action::Workspace,
    mpv_audio_level: f32,
) {
    let (label, label_color, bulb, show_bulb) = match mode {
        InputMode::Filter => ("FILTER", C_MODE_FILTER, C_MODE_FILTER, false),
        InputMode::Command => ("COMMAND", C_MODE_COMMAND, C_MODE_COMMAND, false),
        InputMode::Normal => match workspace {
            crate::action::Workspace::Radio => (
                "RADIO",
                C_MODE_NORMAL,
                bulb_color(mpv_audio_level),
                true,
            ),
            crate::action::Workspace::Files => (
                "FILES",
                C_MODE_NORMAL,
                bulb_color(mpv_audio_level),
                true,
            ),
        },
    };

    let mut left_spans = vec![Span::styled(
        format!(" {} ", label),
        Style::default().fg(label_color).add_modifier(Modifier::BOLD),
    )];
    if show_bulb {
        left_spans.push(Span::styled(
            "●",
            Style::default().fg(bulb).add_modifier(Modifier::BOLD),
        ));
        left_spans.push(Span::raw(" "));
    }

    let keys = match mode {
        InputMode::Normal => match workspace {
            crate::action::Workspace::Radio => {
                " ↑↓/jk select  Enter play/stop  Space pause  ←→ vol  n/p/r playback  !/@ NTS  o scope  Tab/1-4 panes  / filter  K keys  L logs  ? help  q quit"
            }
            crate::action::Workspace::Files => {
                " ↑↓/jk select  Enter play/stop  Space pause  ,/. seek (Shift=±5m)  ←→ vol  r random  R back  Tab/1-4 panes  / filter  K keys  L logs  ? help  q quit"
            }
        },
        InputMode::Filter => " type to filter  Up/Down move  Enter keep  Esc clear+close  Tab next pane",
        InputMode::Command => " type command  Esc cancel  Enter execute",
    };

    let keys_span = Span::styled(keys, Style::default().fg(C_MUTED));

    left_spans.push(Span::raw(" "));
    left_spans.push(keys_span);
    let line = Line::from(left_spans);
    frame.render_widget(Paragraph::new(line), area);
}
