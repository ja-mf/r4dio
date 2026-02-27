//! FilterInput — wraps tui-input for use as a filter bar in panes.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use tui_input::{backend::crossterm::EventHandler, Input};

use crate::theme::{C_FILTER_BG, C_FILTER_FG, C_MUTED, C_SECONDARY};

pub enum FilterAction {
    Changed(String),
    Confirmed,
    Cancelled,
    None,
}

pub struct FilterInput {
    input: Input,
    pub active: bool,
    placeholder: String,
}

impl FilterInput {
    pub fn new(placeholder: impl Into<String>) -> Self {
        Self {
            input: Input::default(),
            active: false,
            placeholder: placeholder.into(),
        }
    }

    pub fn activate(&mut self) {
        self.active = true;
    }

    pub fn deactivate(&mut self) {
        self.active = false;
    }

    pub fn clear(&mut self) {
        self.input = Input::default();
    }

    pub fn set_value(&mut self, value: &str) {
        self.input = Input::new(value.to_string());
    }

    pub fn text(&self) -> &str {
        self.input.value()
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn is_empty(&self) -> bool {
        self.input.value().is_empty()
    }

    /// Handle a key event. Returns what happened.
    ///
    /// Esc behaviour:
    ///   - If the input has text: clear the text, emit `Changed("")` (keeps filter open but empty)
    ///   - If the input is already empty: deactivate and emit `Cancelled`
    pub fn handle_key(&mut self, key: KeyEvent) -> FilterAction {
        match key.code {
            KeyCode::Esc => {
                if !self.input.value().is_empty() {
                    // First Esc: just clear the text
                    self.input = tui_input::Input::default();
                    FilterAction::Changed(String::new())
                } else {
                    // Second Esc (already empty): close filter
                    self.deactivate();
                    FilterAction::Cancelled
                }
            }
            KeyCode::Enter => {
                self.deactivate();
                FilterAction::Confirmed
            }
            _ => {
                self.input
                    .handle_event(&ratatui::crossterm::event::Event::Key(key));
                FilterAction::Changed(self.input.value().to_string())
            }
        }
    }

    /// Render the filter input bar into `area`.
    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        let scroll = self
            .input
            .visual_scroll(area.width.saturating_sub(4) as usize);
        let value = self.input.value();
        let display = if value.is_empty() {
            Span::styled(
                format!("/ {}", self.placeholder),
                Style::default().fg(C_MUTED),
            )
        } else {
            Span::styled(
                format!("/ {}", &value[scroll..]),
                Style::default().fg(C_FILTER_FG),
            )
        };

        let paragraph =
            Paragraph::new(Line::from(vec![display])).style(Style::default().bg(C_FILTER_BG));
        frame.render_widget(paragraph, area);

        // Show cursor when active
        if self.active && !value.is_empty() {
            let cursor_x = area.x + 2 + (self.input.visual_cursor() - scroll) as u16;
            frame.set_cursor_position((cursor_x.min(area.x + area.width - 1), area.y));
        }
    }
}

impl Default for FilterInput {
    fn default() -> Self {
        Self::new("filter...")
    }
}
