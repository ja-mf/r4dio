//! IcyTicker component — ICY metadata history panel (right pane, upper).

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
        pane_chrome::{pane_chrome_borders, Badge},
    },
};
use ratatui::widgets::Borders;

pub struct IcyTicker {
    pub selected: usize,
    pub scroll_offset: usize,
    pub filter_input: FilterInput,
    pub filter: String,
    /// Cached visible indices from last draw (newest-first filtered).
    last_visible: Vec<usize>,
    /// Which borders to draw (for collapsed/shared-border layouts).
    pub borders: Borders,
    /// Dynamic pane number hint (set by app.rs before draw).
    pub number_key: Option<char>,
}

impl IcyTicker {
    pub fn new() -> Self {
        Self {
            selected: 0,
            scroll_offset: 0,
            filter_input: FilterInput::new("search icy metadata…"),
            filter: String::new(),
            last_visible: Vec::new(),
            borders: Borders::ALL,
            number_key: Some('2'),
        }
    }

    pub fn is_filter_active(&self) -> bool {
        self.filter_input.is_active()
    }

    fn visible_indices(&self, state: &AppState) -> Vec<usize> {
        let q = self.filter.to_lowercase();
        (0..state.icy_history.len())
            .rev()
            .filter(|&i| {
                let e = &state.icy_history[i];
                if q.is_empty() {
                    return true;
                }
                let mut text = e.display.to_lowercase();
                if let Some(st) = e.station.as_deref() {
                    text.push(' ');
                    text.push_str(&st.to_lowercase());
                }
                q.split_whitespace().all(|term| text.contains(term))
            })
            .collect()
    }

    pub fn select_up(&mut self, n: usize) {
        self.selected = self.selected.saturating_sub(n);
    }

    pub fn select_down(&mut self, n: usize, max: usize) {
        self.selected = (self.selected + n).min(max.saturating_sub(1));
    }
}

impl Component for IcyTicker {
    fn id(&self) -> ComponentId {
        ComponentId::IcyTicker
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
                    let max = self.last_visible.len();
                    self.select_down(1, max);
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
                let max = self.last_visible.len();
                self.select_down(1, max);
            }
            KeyCode::PageUp => self.select_up(10),
            KeyCode::PageDown => {
                let max = self.last_visible.len();
                self.select_down(10, max);
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

            KeyCode::Char('y') => {
                let text = self
                    .last_visible
                    .get(self.selected)
                    .and_then(|&i| state.icy_history.get(i))
                    .map(|e| e.raw.clone())
                    .unwrap_or_default();
                if !text.is_empty() {
                    return vec![Action::CopyToClipboard(text)];
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
                let max = self.last_visible.len();
                self.select_down(1, max);
            }
            MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left) => {
                let rel_row = event.row.saturating_sub(area.y + 1) as usize; // +1 for header
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
        match action {
            Action::ClearFilter => {
                self.filter.clear();
                self.filter_input.clear();
                self.filter_input.deactivate();
                self.selected = 0;
            }
            _ => {}
        }
        vec![]
    }

    fn collapse_summary(&self, state: &AppState) -> Option<String> {
        state.icy_history.last().map(|e| e.raw.clone())
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, focused: bool, state: &AppState) {
        if area.height == 0 {
            return;
        }

        let block = pane_chrome_borders("icy", self.number_key, focused, None, self.borders);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let visible = self.visible_indices(state);
        self.last_visible = visible.clone();

        let total = visible.len();
        let height = inner.height as usize;

        if total == 0 {
            let msg = if self.filter.is_empty() {
                "  no icy metadata yet"
            } else {
                "  no entries match filter"
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

        // Clamp selection
        if self.selected >= total {
            self.selected = total.saturating_sub(1);
        }

        // Scroll to keep selected in view
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
                let entry = &state.icy_history[orig_idx];
                let is_selected = abs_i == self.selected;
                let is_newest =
                    orig_idx == state.icy_history.len().saturating_sub(1) && self.filter.is_empty();

                let bullet = if is_selected {
                    "  ▸  "
                } else if is_newest {
                    "  ♪  "
                } else {
                    "     "
                };

                let style = if is_selected && focused {
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

                let mut spans = vec![
                    Span::styled(bullet, Style::default().fg(C_MUTED)),
                    Span::styled(entry.display.as_str(), style),
                ];

                if let Some(st) = entry.station.as_deref() {
                    spans.push(Span::styled(
                        format!("  {}", st),
                        Style::default().fg(C_MUTED),
                    ));
                }

                Line::from(spans)
            })
            .collect();

        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);

        // Filter bar at bottom when active
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
