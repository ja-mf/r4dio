//! Smooth Unicode progress bar widget.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::theme::{C_MUTED, C_PLAYING, C_SECONDARY};

/// Render a smooth progress bar in `area`.
/// `progress` is 0.0..=1.0. `time_pos` and `duration` are optional display values.
pub fn draw_progress(
    frame: &mut Frame,
    area: Rect,
    progress: f64,
    time_pos: Option<f64>,
    duration: Option<f64>,
) {
    if area.width < 4 || area.height == 0 {
        return;
    }

    // Time labels
    let left_label = time_pos.map(fmt_time).unwrap_or_default();
    let right_label = duration.map(fmt_time).unwrap_or_default();
    let label_w = (left_label.len() + right_label.len() + 1) as u16;
    let bar_w = area.width.saturating_sub(label_w).max(4) as usize;

    // Unicode smooth fill: 8 eighths per cell
    let eighths = (progress.clamp(0.0, 1.0) * bar_w as f64 * 8.0) as usize;
    let full_blocks = eighths / 8;
    let partial = eighths % 8;

    const BLOCKS: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

    let mut bar = String::with_capacity(bar_w + 4);
    for _ in 0..full_blocks {
        bar.push('█');
    }
    if full_blocks < bar_w {
        bar.push(BLOCKS[partial]);
        for _ in (full_blocks + 1)..bar_w {
            bar.push(' ');
        }
    }

    let mut spans = Vec::new();
    if !left_label.is_empty() {
        spans.push(Span::styled(
            format!("{} ", left_label),
            Style::default().fg(C_SECONDARY),
        ));
    }
    spans.push(Span::styled(bar, Style::default().fg(C_PLAYING)));
    if !right_label.is_empty() {
        spans.push(Span::styled(
            format!(" {}", right_label),
            Style::default().fg(C_MUTED),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn fmt_time(secs: f64) -> String {
    if secs < 0.0 {
        return "0:00".to_string();
    }
    let s = secs as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let s = s % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    }
}
