//! FileMeta component — file metadata + tracklist/chapters panel.

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
    theme::{C_LOCATION, C_MUTED, C_PRIMARY, C_SECONDARY},
    widgets::{
        filter_input::{FilterAction, FilterInput},
        pane_chrome::{pane_chrome_borders, Badge},
    },
};
use ratatui::widgets::Borders;

pub struct FileMeta {
    pub scroll: usize,
    pub filter_input: FilterInput,
    pub filter: String,
    /// Which borders to draw (for collapsed/shared-border layouts).
    pub borders: Borders,
}

impl FileMeta {
    pub fn new() -> Self {
        Self {
            scroll: 0,
            filter_input: FilterInput::new("search metadata / tracklist…"),
            filter: String::new(),
            borders: Borders::ALL,
        }
    }

    pub fn is_filter_active(&self) -> bool {
        self.filter_input.is_active()
    }

    fn build_lines(&self, state: &AppState) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        // Find currently selected file from state
        let selected_path = state.daemon_state.current_file.clone();

        // We show meta for the currently-playing file (or the selected file
        // in the FileList — that is passed via AppState.files + a selected idx
        // but we don't have that index here; use current_file as proxy).
        let path = match selected_path.as_deref() {
            Some(p) => p.to_string(),
            None => {
                lines.push(Line::from(Span::styled(
                    "  no file selected",
                    Style::default().fg(C_MUTED),
                )));
                return lines;
            }
        };

        let file_name = std::path::Path::new(&path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file")
            .to_string();

        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(file_name, Style::default().fg(C_PRIMARY)),
        ]));

        if let Some(meta) = state.file_metadata_cache.get(&path) {
            if let Some(genre) = meta.genre.as_deref() {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(genre.to_string(), Style::default().fg(C_LOCATION)),
                ]));
            }

            let mut parts: Vec<String> = Vec::new();
            if let Some(dur) = meta.duration_secs {
                parts.push(fmt_clock(dur));
            }
            if let Some(codec) = meta.codec.as_deref() {
                parts.push(codec.to_string());
            }
            if let Some(br) = meta.bitrate_kbps {
                parts.push(format!("{}k", br));
            }
            if let Some(sr) = meta.sample_rate_hz {
                parts.push(format!("{}Hz", sr));
            }
            if let Some(ch) = meta.channels {
                parts.push(format!("{}ch", ch));
            }

            if !parts.is_empty() {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(parts.join("  ·  "), Style::default().fg(C_SECONDARY)),
                ]));
            }

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " tracklist".to_string(),
                Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
            )));

            if !meta.tracklist.is_empty() {
                for item in meta.tracklist.iter().take(200) {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(item.clone(), Style::default().fg(C_PRIMARY)),
                    ]));
                }
            } else if !meta.chapters.is_empty() {
                for ch in meta.chapters.iter().take(200) {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            format!("{}-{}", fmt_clock(ch.start_secs), fmt_clock(ch.end_secs)),
                            Style::default().fg(C_MUTED),
                        ),
                        Span::raw("  "),
                        Span::styled(ch.title.clone(), Style::default().fg(C_PRIMARY)),
                    ]));
                }
            } else {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled("(no tracklist)".to_string(), Style::default().fg(C_MUTED)),
                ]));
            }
        } else {
            lines.push(Line::from(Span::styled(
                "  loading metadata…",
                Style::default().fg(C_MUTED),
            )));
        }

        lines
    }

    fn filtered_lines(&self, state: &AppState) -> Vec<Line<'static>> {
        let all = self.build_lines(state);
        if self.filter.is_empty() {
            return all;
        }
        let q = self.filter.to_lowercase();
        all.into_iter()
            .filter(|line| {
                line.spans
                    .iter()
                    .any(|s| s.content.to_lowercase().contains(q.as_str()))
            })
            .collect()
    }
}

impl Component for FileMeta {
    fn id(&self) -> ComponentId {
        ComponentId::FileMeta
    }

    fn handle_key(&mut self, key: KeyEvent, state: &AppState) -> Vec<Action> {
        if key.kind == KeyEventKind::Release {
            return vec![];
        }

        if self.filter_input.is_active() {
            match self.filter_input.handle_key(key) {
                FilterAction::Changed(q) => {
                    self.filter = q;
                    self.scroll = 0;
                    return vec![];
                }
                FilterAction::Cancelled => {
                    self.filter.clear();
                    self.scroll = 0;
                    return vec![Action::CloseFilter];
                }
                FilterAction::Confirmed | FilterAction::None => return vec![],
            }
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
                // scroll to bottom: compute in draw
                self.scroll = usize::MAX;
            }

            KeyCode::Char('/') => {
                self.filter_input.activate();
                return vec![Action::OpenFilter];
            }

            _ => {}
        }

        let _ = state;
        vec![]
    }

    fn handle_mouse(&mut self, event: MouseEvent, _area: Rect, _state: &AppState) -> Vec<Action> {
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

    fn on_action(&mut self, action: &Action, state: &AppState) -> Vec<Action> {
        match action {
            Action::ClearFilter => {
                self.filter.clear();
                self.filter_input.clear();
                self.filter_input.deactivate();
                self.scroll = 0;
            }
            _ => {}
        }
        let _ = state;
        vec![]
    }

    fn collapse_summary(&self, state: &AppState) -> Option<String> {
        // Show title + artist from currently-selected file metadata
        let path = state
            .daemon_state
            .current_file
            .as_deref()
            .or_else(|| None)?;
        let meta = state.file_metadata_cache.get(path)?;
        match (&meta.title, &meta.artist) {
            (Some(t), Some(a)) => Some(format!("{} — {}", t, a)),
            (Some(t), None) => Some(t.clone()),
            _ => None,
        }
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, focused: bool, state: &AppState) {
        if area.height == 0 {
            return;
        }

        let block = pane_chrome_borders("meta", Some('2'), focused, None, self.borders);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let lines = self.filtered_lines(state);
        let total = lines.len();
        let height = inner.height as usize;

        // Clamp scroll
        let max_scroll = total.saturating_sub(height);
        if self.scroll > max_scroll {
            self.scroll = max_scroll;
        }

        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((self.scroll as u16, 0)),
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
    }
}

fn fmt_clock(v: f64) -> String {
    let total = v.max(0.0).round() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{:02}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}
