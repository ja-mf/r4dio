//! PaneChrome — standardized bordered pane with focus styling and badges.

use crate::theme::{
    style_focused_border, style_unfocused_border, C_MUTED, C_NUMBER_HINT, C_PANEL_BORDER,
    C_PANEL_BORDER_FOCUSED, C_PRIMARY, C_SECONDARY,
};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// A badge shown in the top-right of the pane header (e.g., "LIVE", "ERR").
pub struct Badge<'a> {
    pub text: &'a str,
    pub color: Color,
}

/// Renders a bordered pane with consistent focus styling and optional badge.
///
/// `borders` controls which sides are drawn — pass `Borders::ALL` for the
/// default full-border look, or omit shared edges for a collapsed/lazygit style.
pub fn pane_chrome<'a>(
    title: &'a str,
    number_key: Option<char>,
    focused: bool,
    badge: Option<Badge<'a>>,
) -> Block<'a> {
    pane_chrome_borders(title, number_key, focused, badge, Borders::ALL)
}

/// Like `pane_chrome` but with explicit border selection for collapsed layouts.
pub fn pane_chrome_borders<'a>(
    title: &'a str,
    number_key: Option<char>,
    focused: bool,
    badge: Option<Badge<'a>>,
    borders: Borders,
) -> Block<'a> {
    let border_style = if focused {
        style_focused_border()
    } else {
        style_unfocused_border()
    };

    let title_style = if focused {
        Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(C_MUTED)
    };

    // Build title spans: "[N] title"
    let mut title_spans = Vec::new();
    if let Some(key) = number_key {
        title_spans.push(Span::styled(
            format!("[{}] ", key),
            Style::default().fg(C_NUMBER_HINT),
        ));
    }
    title_spans.push(Span::styled(title, title_style));

    let block = Block::default()
        .borders(borders)
        .border_style(border_style)
        .title(Line::from(title_spans));

    // Add badge to title_top_right if present
    if let Some(b) = badge {
        block.title_top(
            Line::from(Span::styled(
                format!(" {} ", b.text),
                Style::default().fg(b.color).add_modifier(Modifier::BOLD),
            ))
            .right_aligned(),
        )
    } else {
        block
    }
}

/// Draw a collapsed pane as a single horizontal strip showing:
///   " ▶ title  summary… "
///
/// The strip uses the same focused/unfocused colour scheme as `pane_chrome`.
pub fn draw_collapsed_pane(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    summary: Option<&str>,
    focused: bool,
) {
    if area.height == 0 {
        return;
    }

    let title_style = if focused {
        Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(C_MUTED)
    };
    let summary_style = Style::default().fg(C_SECONDARY);
    let dim_style = Style::default().fg(C_PANEL_BORDER);

    let mut spans = vec![
        Span::styled(" ▸ ", dim_style),
        Span::styled(title, title_style),
    ];

    if let Some(s) = summary {
        if !s.is_empty() {
            spans.push(Span::styled("  ", Style::default()));
            spans.push(Span::styled(s, summary_style));
        }
    }

    let line = Line::from(spans);
    frame.render_widget(Paragraph::new(line), area);
}
