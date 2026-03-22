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

        let popup = centered_rect(66, 50, area);

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
            help_row("enter", "▶/■  play or stop selected"),
            help_row("space", "⏸  pause / resume"),
            help_row("← → or - +", "vol ▼ / vol ▲"),
            help_row(", / .", "seek ±30s  (Shift = ±5m)"),
            help_row("n / P", "⏭ next  /  ⏮ prev"),
            help_row("r", "⇀ random station"),
            help_row("l", "⏴ play last station"),
            help_row("m", "⊘ mute toggle"),
            help_row("i", "identify current song"),
            help_row("d", "download NTS episode"),
            Line::from(""),
            Line::from(Span::styled(
                " navigation & panes",
                Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
            )),
            help_row("↑↓  or  j/k", "move selection"),
            help_row("PgUp / PgDn", "jump 10 rows"),
            help_row("Home/End  g/G", "first / last"),
            help_row("Tab / Shift-Tab", "next / prev pane"),
            help_row("1", "focus stations"),
            help_row("2", "toggle icy history pane"),
            help_row("3", "toggle logged mixtapes/songs"),
            help_row("4", "focus NTS panel"),
            help_row("c", "jump to current playing station"),
            help_row("`", "select previously played station"),
            help_row("f", "Radio ↔ Files workspace"),
            help_row("! / @", "NTS 1 / NTS 2 overlay"),
            help_row("o", "oscilloscope panel"),
            help_row("_ or |", "full-width right pane"),
            Line::from(""),
            Line::from(Span::styled(
                " lists & ui",
                Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
            )),
            help_row("/", "filter  (Enter=apply  Esc=clear)"),
            help_row("s / S", "sort ↑ / sort ↓"),
            help_row("*", "cycle stars"),
            help_row("y", "copy url/path"),
            help_row("C", "collapse focused pane"),
            help_row("Ctrl+l", "log panel"),
            help_row("p", "toggle auto-polling"),
            help_row("?", "this help"),
            help_row("q  or  Ctrl+C", "quit"),
            Line::from(""),
            Line::from(Span::styled(
                " scope (when focused)",
                Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
            )),
            help_row("↑↓ ←→", "scale / sample window  (Shift=coarse)"),
            help_row("Esc", "reset scope"),
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
