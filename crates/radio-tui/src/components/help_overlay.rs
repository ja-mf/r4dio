//! HelpOverlay component — centered popup with keyboard shortcut reference.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::{
    action::{Action, ComponentId},
    app_state::AppState,
    component::Component,
    theme::{C_MUTED, C_PANEL_BORDER, C_PRIMARY, C_SECONDARY},
};

pub struct HelpOverlay {
    pub visible: bool,
}

impl HelpOverlay {
    pub fn new() -> Self {
        Self { visible: false }
    }

    pub fn show(&mut self) {
        self.visible = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }
}

impl Component for HelpOverlay {
    fn id(&self) -> ComponentId {
        ComponentId::HelpOverlay
    }

    fn handle_key(&mut self, key: KeyEvent, _state: &AppState) -> Vec<Action> {
        if key.kind == KeyEventKind::Release {
            return vec![];
        }
        if !self.visible {
            return vec![];
        }
        match key.code {
            KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Esc => {
                self.hide();
                return vec![Action::ToggleHelp];
            }
            _ => {}
        }
        // Consume all keys while overlay is open
        vec![]
    }

    fn handle_mouse(&mut self, _event: MouseEvent, _area: Rect, _state: &AppState) -> Vec<Action> {
        vec![]
    }

    fn on_action(&mut self, action: &Action, _state: &AppState) -> Vec<Action> {
        match action {
            Action::ToggleHelp => {
                self.toggle();
            }
            _ => {}
        }
        vec![]
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, _focused: bool, _state: &AppState) {
        if !self.visible {
            return;
        }

        let popup = centered_rect(68, 34, area);

        let help_lines: Vec<Line> = vec![
            Line::from(Span::styled(
                " keyboard shortcuts (current)",
                Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                " playback",
                Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
            )),
            help_row("enter", "play/stop selected station or file"),
            help_row("space", "toggle pause/play"),
            help_row("← / →  or  - / +", "volume down / up"),
            help_row(", / .", "seek file ±30s (Shift = ±5m)"),
            help_row("n / P / r / R", "next / prev / random / random back"),
            help_row("m", "mute"),
            help_row("i", "identify song"),
            help_row("d", "download NTS show"),
            Line::from(""),
            Line::from(Span::styled(
                " navigation & panes",
                Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
            )),
            help_row("↑ / ↓  or  j / k", "move selection / scroll"),
            help_row("pg up / pg dn", "jump 10 rows"),
            help_row("home / end  or  g / G", "jump first / last"),
            help_row("tab / shift-tab", "focus next / previous pane"),
            help_row("1 / 2 / 3 / 4", "focus pane slot"),
            help_row("f", "switch Radio ↔ Files workspace"),
            help_row("! / @", "toggle NTS 1 / NTS 2 panel"),
            help_row("o", "toggle scope panel"),
            help_row("_  or  |", "toggle right pane full width"),
            Line::from(""),
            Line::from(Span::styled(
                " lists & ui",
                Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
            )),
            help_row("/", "open filter (Esc clears + closes)"),
            help_row("s / S", "cycle sort forward / backward"),
            help_row("*", "cycle stars on selected item"),
            help_row("y", "copy selected url/text/path"),
            help_row("J", "jump to current playing item"),
            help_row("c", "collapse focused pane"),
            help_row("K / L", "toggle keys bar / log panel"),
            help_row("p", "toggle passive polling on/off"),
            help_row("?", "toggle this help overlay"),
            help_row("q / Ctrl+C", "quit"),
            Line::from(""),
            Line::from(Span::styled(
                " scope (when focused)",
                Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
            )),
            help_row("↑ / ↓ / ← / →", "adjust scale/samples (Shift = coarse)"),
            help_row("esc", "reset scope scale + sample window"),
            Line::from(""),
            Line::from(Span::styled(
                " press ? or esc to close",
                Style::default().fg(C_MUTED),
            )),
        ];

        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(help_lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(C_PANEL_BORDER))
                        .style(Style::default().bg(ratatui::style::Color::Rgb(18, 18, 26))),
                )
                .wrap(Wrap { trim: false }),
            popup,
        );
    }
}

fn help_row<'a>(key: &'a str, desc: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("{:<16}", key),
            Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled(desc, Style::default().fg(C_SECONDARY)),
    ])
}

fn centered_rect(percent_x: u16, height: u16, r: Rect) -> Rect {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vert[1])[1]
}
