//! SongsTicker — songs.vds recognition history panel.
//!
//! Displays recognised songs (newest at top).  Each row shows:
//!   [source badge]  HH:MM  Artist – Title  ·  station  ·  show
//!
//! Keybindings (when focused):
//!   i        — trigger song recognition (vibra + ICY + NTS pipeline)
//!   y        — copy display text to clipboard
//!   /        — open filter
//!   Esc      — clear filter text (first press) / close filter (second press)
//!   j/k ↑↓   — navigate
//!   Enter    — open show URL if present

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseEvent, MouseEventKind};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

use crate::{
    action::{Action, ComponentId},
    app_state::AppState,
    component::Component,
    theme::{C_MUTED, C_PRIMARY, C_SECONDARY, C_SELECTION_BG},
    widgets::{
        filter_input::{FilterAction, FilterInput},
        pane_chrome::pane_chrome_borders,
    },
};
use radio_proto::songs::RecognitionResult;
use ratatui::style::Color;
use ratatui::widgets::Borders;

// Source badge colours
const C_VIBRA: Color = Color::Rgb(180, 120, 220); // purple
const C_ICY: Color = Color::Rgb(80, 160, 220); // blue
const C_NTS: Color = Color::Rgb(220, 80, 80); // red

pub struct SongsTicker {
    pub selected: usize,
    pub scroll_offset: usize,
    pub filter_input: FilterInput,
    pub filter: String,
    last_visible: Vec<usize>,
    pub borders: Borders,
    /// Dynamic pane number hint (set by app.rs before draw).
    pub number_key: Option<char>,
}

impl SongsTicker {
    pub fn new() -> Self {
        Self {
            selected: 0,
            scroll_offset: 0,
            filter_input: FilterInput::new("search songs…"),
            filter: String::new(),
            last_visible: Vec::new(),
            borders: Borders::ALL,
            number_key: Some('3'),
        }
    }

    pub fn is_filter_active(&self) -> bool {
        self.filter_input.is_active()
    }

    fn visible_indices(&self, state: &AppState) -> Vec<usize> {
        let q = self.filter.to_lowercase();
        // newest first
        (0..state.songs_history.len())
            .rev()
            .filter(|&i| {
                if q.is_empty() {
                    return true;
                }
                let e = &state.songs_history[i];
                let mut text = e.display().to_lowercase();
                if let Some(s) = e.station.as_deref() {
                    text.push(' ');
                    text.push_str(&s.to_lowercase());
                }
                if let Some(s) = e.nts_show.as_deref() {
                    text.push(' ');
                    text.push_str(&s.to_lowercase());
                }
                if let Some(s) = e.icy_info.as_deref() {
                    text.push(' ');
                    text.push_str(&s.to_lowercase());
                }
                q.split_whitespace().all(|term| text.contains(term))
            })
            .collect()
    }

    fn select_up(&mut self, n: usize) {
        self.selected = self.selected.saturating_sub(n);
    }

    fn select_down(&mut self, n: usize, max: usize) {
        self.selected = (self.selected + n).min(max.saturating_sub(1));
    }

    fn selected_entry<'a>(&self, state: &'a AppState) -> Option<&'a RecognitionResult> {
        let idx = self.last_visible.get(self.selected)?;
        state.songs_history.get(*idx)
    }
}

impl Component for SongsTicker {
    fn id(&self) -> ComponentId {
        ComponentId::SongsTicker
    }

    fn handle_key(&mut self, key: KeyEvent, state: &AppState) -> Vec<Action> {
        if key.kind == KeyEventKind::Release {
            return vec![];
        }

        if self.filter_input.is_active() {
            match key.code {
                KeyCode::Up => {
                    self.select_up(1);
                    return vec![];
                }
                KeyCode::Down => {
                    let m = self.last_visible.len();
                    self.select_down(1, m);
                    return vec![];
                }
                _ => {}
            }
            match self.filter_input.handle_key(key) {
                FilterAction::Changed(q) => {
                    self.filter = q;
                    self.selected = 0;
                    self.scroll_offset = 0;
                    return vec![];
                }
                FilterAction::Cancelled => {
                    self.filter.clear();
                    self.selected = 0;
                    return vec![Action::CloseFilter];
                }
                FilterAction::Confirmed | FilterAction::None => return vec![],
            }
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.select_up(1),
            KeyCode::Down | KeyCode::Char('j') => {
                let m = self.last_visible.len();
                self.select_down(1, m);
            }
            KeyCode::PageUp => self.select_up(10),
            KeyCode::PageDown => {
                let m = self.last_visible.len();
                self.select_down(10, m);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.selected = 0;
                self.scroll_offset = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.selected = self.last_visible.len().saturating_sub(1);
            }

            KeyCode::Char('/') => {
                self.filter_input.activate();
                return vec![Action::OpenFilter];
            }

            // Song identification (lowercase i)
            KeyCode::Char('i') => {
                return vec![Action::RecognizeSong];
            }

            KeyCode::Char('y') => {
                if let Some(e) = self.selected_entry(state) {
                    let text = e.display();
                    if !text.is_empty() {
                        return vec![Action::CopyToClipboard(text)];
                    }
                }
            }

            KeyCode::Enter => {
                if let Some(e) = self.selected_entry(state) {
                    if let Some(url) = e.nts_url.clone() {
                        return vec![Action::CopyToClipboard(url)];
                    }
                }
            }

            _ => {}
        }
        vec![]
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: Rect, _state: &AppState) -> Vec<Action> {
        match event.kind {
            MouseEventKind::ScrollUp => self.select_up(1),
            MouseEventKind::ScrollDown => {
                let m = self.last_visible.len();
                self.select_down(1, m);
            }
            MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left) => {
                let rel_row = event.row.saturating_sub(area.y) as usize;
                let target = self.scroll_offset + rel_row;
                if target < self.last_visible.len() {
                    self.selected = target;
                }
            }
            _ => {}
        }
        vec![]
    }

    fn on_action(&mut self, action: &Action, _state: &AppState) -> Vec<Action> {
        if let Action::ClearFilter = action {
            self.filter.clear();
            self.filter_input.clear();
            self.filter_input.deactivate();
            self.selected = 0;
        }
        vec![]
    }

    fn collapse_summary(&self, state: &AppState) -> Option<String> {
        state.songs_history.last().map(|e| e.display())
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, focused: bool, state: &AppState) {
        if area.height == 0 {
            return;
        }

        let block = pane_chrome_borders("songs", self.number_key, focused, None, self.borders);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let visible = self.visible_indices(state);
        self.last_visible = visible.clone();
        let total = visible.len();
        let height = inner.height as usize;

        if total == 0 {
            let msg = if self.filter.is_empty() {
                "  no songs yet — press i to identify"
            } else {
                "  no songs match filter"
            };
            frame.render_widget(
                Paragraph::new(Span::styled(msg, Style::default().fg(C_MUTED))),
                inner,
            );
            if self.filter_input.is_active() {
                let bar = Rect {
                    y: inner.y + inner.height.saturating_sub(1),
                    height: 1,
                    ..inner
                };
                self.filter_input.draw(frame, bar);
            }
            return;
        }

        if self.selected >= total {
            self.selected = total.saturating_sub(1);
        }
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + height {
            self.scroll_offset = self.selected.saturating_sub(height.saturating_sub(1));
        }

        let lines: Vec<Line> = visible
            .iter()
            .skip(self.scroll_offset)
            .take(height)
            .enumerate()
            .map(|(view_i, &orig_idx)| {
                let abs_i = self.scroll_offset + view_i;
                let entry = &state.songs_history[orig_idx];
                let is_selected = abs_i == self.selected;
                let is_newest = orig_idx == state.songs_history.len().saturating_sub(1)
                    && self.filter.is_empty();

                let row_style = if is_selected && focused {
                    Style::default()
                        .fg(C_PRIMARY)
                        .bg(C_SELECTION_BG)
                        .add_modifier(Modifier::BOLD)
                } else if is_selected {
                    Style::default().fg(C_PRIMARY)
                } else if is_newest {
                    Style::default().fg(C_PRIMARY)
                } else {
                    Style::default().fg(C_SECONDARY)
                };

                let mut spans: Vec<Span> = Vec::new();

                // Source badge(s)
                let src_str = source_badge_str(entry);
                spans.push(Span::styled(
                    format!(" {:6} ", src_str),
                    Style::default().fg(source_color(entry)),
                ));

                // Timestamp
                if let Some(ts) = &entry.timestamp {
                    let ts_str = format_ts(ts);
                    spans.push(Span::styled(
                        format!("{} ", ts_str),
                        Style::default().fg(C_MUTED),
                    ));
                }

                // Main display: Artist – Title
                let display = entry.display();
                spans.push(Span::styled(display, row_style));

                // Station · show
                if let Some(st) = entry.station.as_deref() {
                    spans.push(Span::styled(
                        format!("  {}", st),
                        Style::default().fg(C_MUTED),
                    ));
                }
                if let Some(show) = entry.nts_show.as_deref() {
                    spans.push(Span::styled(" · ", Style::default().fg(C_MUTED)));
                    spans.push(Span::styled(show, Style::default().fg(C_SECONDARY)));
                }

                // Show URL hint
                if entry.nts_url.is_some() {
                    spans.push(Span::styled(" ↗", Style::default().fg(C_MUTED)));
                }

                Line::from(spans)
            })
            .collect();

        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);

        if self.filter_input.is_active() {
            let bar = Rect {
                y: inner.y + inner.height.saturating_sub(1),
                height: 1,
                ..inner
            };
            self.filter_input.draw(frame, bar);
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn source_badge_str(entry: &RecognitionResult) -> String {
    let srcs = entry.sources();
    if srcs.is_empty() {
        // If only a placeholder row (no data yet), show "…"
        return "\u{2026}".to_string();
    }
    if srcs.len() == 1 {
        srcs[0].label().to_string()
    } else {
        // Multiple sources: show primary (secondary) format
        // e.g., "vibra (icy)" when both vibra and icy are present
        let primary = srcs[0].label();
        let rest: Vec<_> = srcs[1..].iter().map(|s| s.label()).collect();
        format!("{} ({})", primary, rest.join(" "))
    }
}

fn source_color(entry: &RecognitionResult) -> Color {
    use radio_proto::songs::RecognitionSource;
    let srcs = entry.sources();
    if srcs.contains(&RecognitionSource::Vibra) {
        return C_VIBRA;
    }
    if srcs.contains(&RecognitionSource::Nts) {
        return C_NTS;
    }
    if srcs.contains(&RecognitionSource::Icy) {
        return C_ICY;
    }
    C_MUTED
}

fn format_ts(ts: &chrono::DateTime<chrono::Local>) -> String {
    let today = chrono::Local::now().date_naive();
    if ts.date_naive() == today {
        ts.format("%H:%M").to_string()
    } else {
        ts.format("%m/%d %H:%M").to_string()
    }
}
