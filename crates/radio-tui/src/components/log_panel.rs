//! LogPanel component — collapsible log viewer.
//!
//! Shows one line (most recent log) when collapsed; expands to full panel.
//! Handles its own scroll state.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseEvent, MouseEventKind};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Clear, Paragraph, Wrap},
    Frame,
};

use crate::{
    action::{Action, ComponentId},
    app_state::AppState,
    component::Component,
    theme::{C_MUTED, C_SECONDARY},
    widgets::pane_chrome::pane_chrome_borders,
};
use ratatui::widgets::Borders;

pub struct LogPanel {
    pub expanded: bool,
    pub scroll: usize,
    /// Which borders to draw (for collapsed/shared-border layouts).
    pub borders: Borders,
    /// Track last log count to detect new entries for auto-scroll
    last_log_count: usize,
}

impl LogPanel {
    pub fn new() -> Self {
        Self {
            expanded: false,
            scroll: 0,
            borders: Borders::ALL,
            last_log_count: 0,
        }
    }

    pub fn toggle(&mut self) {
        self.expanded = !self.expanded;
        if self.expanded {
            // Jump to bottom on open
            self.scroll = usize::MAX;
        }
    }
}

impl Component for LogPanel {
    fn id(&self) -> ComponentId {
        ComponentId::LogPanel
    }

    fn handle_key(&mut self, key: KeyEvent, state: &AppState) -> Vec<Action> {
        if key.kind == KeyEventKind::Release {
            return vec![];
        }
        if !self.expanded {
            return vec![];
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll += 1;
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
            }
            KeyCode::PageDown => {
                self.scroll += 10;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.scroll = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.scroll = usize::MAX;
            }
            _ => {}
        }
        let _ = state;
        vec![]
    }

    fn handle_mouse(&mut self, event: MouseEvent, _area: Rect, _state: &AppState) -> Vec<Action> {
        if !self.expanded {
            return vec![];
        }
        match event.kind {
            MouseEventKind::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(1);
            }
            MouseEventKind::ScrollDown => {
                self.scroll += 1;
            }
            _ => {}
        }
        vec![]
    }

    fn on_action(&mut self, action: &Action, _state: &AppState) -> Vec<Action> {
        match action {
            Action::ToggleLogs => {
                self.toggle();
            }
            _ => {}
        }
        vec![]
    }

    fn collapse_summary(&self, state: &AppState) -> Option<String> {
        state.tui_log_lines.last().cloned()
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, focused: bool, state: &AppState) {
        if area.height == 0 {
            return;
        }
        frame.render_widget(Clear, area);

        if !self.expanded || area.height <= 1 {
            // Collapsed: single-line summary, no border
            let last = state
                .tui_log_lines
                .last()
                .map(|s| compact_log_line(s))
                .unwrap_or_else(|| "(no log)".to_string());
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" log ", Style::default().fg(C_MUTED)),
                    Span::styled(last, Style::default().fg(C_SECONDARY)),
                ])),
                area,
            );
            return;
        }

        // Expanded: pane_chrome border + log content
        let block = pane_chrome_borders("log", None, focused, None, self.borders);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let logs = &state.tui_log_lines;
        let height = inner.height as usize;
        let log_count = logs.len();

        // Auto-scroll to bottom if new logs arrived and we were at bottom
        if log_count > self.last_log_count {
            let max_scroll = log_count.saturating_sub(height);
            if self.scroll >= max_scroll.saturating_sub(1) {
                self.scroll = usize::MAX;
            }
            self.last_log_count = log_count;
        }

        if logs.is_empty() {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "  no log entries yet",
                    Style::default().fg(C_MUTED),
                )),
                inner,
            );
            return;
        }

        // Clamp scroll — newest last (scroll 0 = top = oldest)
        let max_scroll = log_count.saturating_sub(height);
        if self.scroll > max_scroll {
            self.scroll = max_scroll;
        }

        let lines: Vec<Line> = logs
            .iter()
            .skip(self.scroll)
            .take(height)
            .map(|msg| {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(compact_log_line(msg), Style::default().fg(C_MUTED)),
                ])
            })
            .collect();

        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }
}

// ── Log line formatting ───────────────────────────────────────────────────────

fn compact_log_line(raw: &str) -> String {
    let clean = strip_ansi(raw).trim().to_string();
    let mut rest = clean.as_str();
    let mut head: Vec<String> = Vec::new();

    // Try to parse a leading RFC3339 timestamp
    if let Some((tok, rem)) = split_first_token(rest) {
        if let Some(ts) = compact_timestamp(tok) {
            head.push(ts);
            rest = rem.trim_start();
        }
    }

    // Try to strip a log level
    if let Some((tok, rem)) = split_first_token(rest) {
        let upper = tok.to_ascii_uppercase();
        if matches!(
            upper.as_str(),
            "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR"
        ) {
            head.push(upper);
            rest = rem.trim_start();
        }
    }

    // Strip a module path prefix like "foo::bar: "
    if let Some((left, msg)) = rest.split_once(": ") {
        if !left.is_empty()
            && left.len() <= 48
            && left
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '-'))
        {
            rest = msg.trim_start();
        }
    }

    if head.is_empty() {
        rest.to_string()
    } else if rest.is_empty() {
        head.join(" ")
    } else {
        format!("{} {}", head.join(" "), rest)
    }
}

fn compact_timestamp(token: &str) -> Option<String> {
    let dt = chrono::DateTime::parse_from_rfc3339(token).ok()?;
    let local = dt.with_timezone(&chrono::Local);
    let fmt = if local.date_naive() == chrono::Local::now().date_naive() {
        "%H:%M:%S"
    } else {
        "%m-%d %H:%M"
    };
    Some(local.format(fmt).to_string())
}

fn split_first_token(s: &str) -> Option<(&str, &str)> {
    let mut parts = s.splitn(2, char::is_whitespace);
    let first = parts.next()?.trim();
    if first.is_empty() {
        return None;
    }
    Some((first, parts.next().unwrap_or("")))
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_escape = false;
    for ch in s.chars() {
        if in_escape {
            if ('@'..='~').contains(&ch) {
                in_escape = false;
            }
            continue;
        }
        if ch == '\u{1b}' {
            in_escape = true;
            continue;
        }
        out.push(ch);
    }
    out
}
