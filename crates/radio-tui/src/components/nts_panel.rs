//! NtsPanel component — NTS live schedule panel.
//!
//! Shows current show + upcoming schedule for one NTS channel.
//! Supports scrolling through content taller than the panel.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseEvent, MouseEventKind};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Wrap},
    Frame,
};

use crate::{
    action::{Action, ComponentId},
    app_state::{AppState, NtsChannel, NtsShow},
    component::Component,
    theme::{C_ACCENT, C_FILTER_BG, C_LOCATION, C_MUTED, C_PRIMARY, C_SECONDARY, C_TAG},
    widgets::pane_chrome::pane_chrome_borders,
};
use ratatui::widgets::Borders;

/// How many upcoming shows to display in the popup.
const COMPACT_UPCOMING: usize = 8;

pub struct NtsPanel {
    /// 0 = NTS 1, 1 = NTS 2
    pub channel: usize,
    pub scroll: usize,
    /// Horizontal scroll offset (columns).
    pub scroll_x: usize,
    /// Which borders to draw (for collapsed/shared-border layouts).
    pub borders: Borders,
    /// Dynamic pane number hint (set by app.rs before draw).
    pub number_key: Option<char>,
}

impl NtsPanel {
    pub fn new(channel: usize) -> Self {
        // Default: ch1 = '2', ch2 = '3' (Radio/Tickers with overlay, or right-pane)
        let number_key = if channel == 0 { Some('2') } else { Some('3') };
        Self {
            channel,
            scroll: 0,
            scroll_x: 0,
            borders: Borders::ALL,
            number_key,
        }
    }

    fn channel_data<'a>(&self, state: &'a AppState) -> Option<&'a NtsChannel> {
        if self.channel == 0 {
            state.nts_ch1.as_ref()
        } else {
            state.nts_ch2.as_ref()
        }
    }

    fn channel_error<'a>(&self, state: &'a AppState) -> Option<&'a str> {
        if self.channel == 0 {
            state.nts_ch1_error.as_deref()
        } else {
            state.nts_ch2_error.as_deref()
        }
    }

    // ── Full-pane (right-panel) content ──────────────────────────────────────

    fn build_lines(&self, ch: &NtsChannel, area_width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        let now = &ch.now;

        // Live / Replay indicator + show title
        let live_span = if now.is_replay {
            Span::styled("(R) ".to_string(), Style::default().fg(C_MUTED))
        } else {
            Span::styled("● ".to_string(), Style::default().fg(C_ACCENT))
        };
        lines.push(Line::from(vec![
            Span::raw(" "),
            live_span,
            Span::styled(
                now.broadcast_title.clone(),
                Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
            ),
        ]));

        // Time · Location
        {
            let time_str = fmt_time_range(now);
            let mut spans: Vec<Span> = vec![
                Span::raw("   "),
                Span::styled(time_str, Style::default().fg(C_SECONDARY)),
            ];
            let loc = if !now.location_long.is_empty() {
                Some(now.location_long.clone())
            } else if !now.location_short.is_empty() {
                Some(now.location_short.clone())
            } else {
                None
            };
            if let Some(l) = loc {
                spans.push(Span::styled(
                    "  ·  ".to_string(),
                    Style::default().fg(C_MUTED),
                ));
                spans.push(Span::styled(l, Style::default().fg(C_LOCATION)));
            }
            lines.push(Line::from(spans));
        }

        // Genres
        if !now.genres.is_empty() {
            lines.push(Line::from(vec![
                Span::raw("   "),
                Span::styled(now.genres.join(" · "), Style::default().fg(C_TAG)),
            ]));
        }

        // Moods
        if !now.moods.is_empty() {
            let wrap_width = (area_width as usize).saturating_sub(4).max(10);
            let mood_str = now.moods.join(" · ");
            for mood_line in word_wrap(&mood_str, wrap_width) {
                lines.push(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(mood_line, Style::default().fg(C_MUTED)),
                ]));
            }
        }

        // Description
        if !now.description.is_empty() {
            lines.push(Line::from(""));
            let wrap_width = (area_width as usize).saturating_sub(4).max(10);
            for desc_line in word_wrap(&now.description, wrap_width) {
                lines.push(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(desc_line, Style::default().fg(C_PRIMARY)),
                ]));
            }
        }

        // Upcoming schedule — one line per show: time  title  location
        if !ch.upcoming.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " upcoming".to_string(),
                Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
            )));
            for show in ch.upcoming.iter() {
                let mut spans = vec![
                    Span::raw("   "),
                    Span::styled(fmt_time_range(show), Style::default().fg(C_SECONDARY)),
                    Span::raw("  "),
                    Span::styled(show.broadcast_title.clone(), Style::default().fg(C_PRIMARY)),
                ];
                if !show.location_short.is_empty() {
                    spans.push(Span::styled(
                        format!("  {}", show.location_short),
                        Style::default().fg(C_LOCATION),
                    ));
                }
                if !show.moods.is_empty() || !show.genres.is_empty() {
                    let tags: Vec<&str> = show
                        .moods
                        .iter()
                        .chain(show.genres.iter())
                        .map(|s| s.as_str())
                        .collect();
                    spans.push(Span::styled(
                        format!("  {}", tags.join(" · ")),
                        Style::default().fg(C_MUTED),
                    ));
                }
                lines.push(Line::from(spans));
            }
        }

        lines
    }

    // ── Compact popup (hover overlay) ────────────────────────────────────────

    /// Build lines for the LEFT column of the compact popup:
    /// title, time/location, genres, moods, description.
    fn build_left_lines(&self, now: &NtsShow, col_width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        // Live / Replay + title
        let live_span = if now.is_replay {
            Span::styled("(R) ".to_string(), Style::default().fg(C_MUTED))
        } else {
            Span::styled("● ".to_string(), Style::default().fg(C_ACCENT))
        };
        // Word-wrap the title itself in case it's long
        let title_wrap = word_wrap(
            &now.broadcast_title,
            (col_width as usize).saturating_sub(4).max(8),
        );
        for (i, tl) in title_wrap.iter().enumerate() {
            if i == 0 {
                lines.push(Line::from(vec![
                    live_span.clone(),
                    Span::styled(
                        tl.clone(),
                        Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
                    ),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        tl.clone(),
                        Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
                    ),
                ]));
            }
        }

        // Time · Location
        {
            let time_str = fmt_time_range(now);
            let loc = if !now.location_long.is_empty() {
                Some(now.location_long.clone())
            } else if !now.location_short.is_empty() {
                Some(now.location_short.clone())
            } else {
                None
            };
            if let Some(l) = loc {
                lines.push(Line::from(vec![
                    Span::styled(time_str, Style::default().fg(C_SECONDARY)),
                    Span::styled("  ·  ".to_string(), Style::default().fg(C_MUTED)),
                    Span::styled(l, Style::default().fg(C_LOCATION)),
                ]));
            } else {
                lines.push(Line::from(Span::styled(
                    time_str,
                    Style::default().fg(C_SECONDARY),
                )));
            }
        }

        // Genres
        if !now.genres.is_empty() {
            let wrap_width = (col_width as usize).saturating_sub(1).max(8);
            for gl in word_wrap(&now.genres.join(" · "), wrap_width) {
                lines.push(Line::from(Span::styled(gl, Style::default().fg(C_TAG))));
            }
        }

        // Moods
        if !now.moods.is_empty() {
            let wrap_width = (col_width as usize).saturating_sub(1).max(8);
            let mood_str = now.moods.join(" · ");
            for ml in word_wrap(&mood_str, wrap_width) {
                lines.push(Line::from(Span::styled(ml, Style::default().fg(C_MUTED))));
            }
        }

        // Description
        if !now.description.is_empty() {
            lines.push(Line::from(""));
            let wrap_width = (col_width as usize).saturating_sub(1).max(8);
            for dl in word_wrap(&now.description, wrap_width) {
                lines.push(Line::from(Span::styled(dl, Style::default().fg(C_PRIMARY))));
            }
        }

        lines
    }

    /// Build lines for the RIGHT column: upcoming schedule (next N, one line each).
    /// Format: `HH:MM – HH:MM  Title  location`
    fn build_right_lines(&self, ch: &NtsChannel, _col_width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        lines.push(Line::from(Span::styled(
            "upcoming".to_string(),
            Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
        )));

        if ch.upcoming.is_empty() {
            lines.push(Line::from(Span::styled(
                "—".to_string(),
                Style::default().fg(C_MUTED),
            )));
            return lines;
        }

        for show in ch.upcoming.iter().take(COMPACT_UPCOMING) {
            let time_str = fmt_time_range(show);
            let mut spans = vec![
                Span::styled(time_str, Style::default().fg(C_SECONDARY)),
                Span::raw("  "),
                Span::styled(show.broadcast_title.clone(), Style::default().fg(C_PRIMARY)),
            ];
            if !show.location_short.is_empty() {
                spans.push(Span::styled(
                    format!("  {}", show.location_short),
                    Style::default().fg(C_LOCATION),
                ));
            }
            lines.push(Line::from(spans));
        }

        lines
    }

    /// Returns the number of inner rows needed to display the compact popup
    /// given an inner width. Used by app.rs to size the overlay before drawing.
    pub fn compact_content_height(&self, ch: &NtsChannel, inner_width: u16) -> u16 {
        // Split the same way draw_compact does: left 60%, right 40% (min widths applied)
        let (left_w, right_w) = compact_col_widths(inner_width);
        let left_h = self.build_left_lines(&ch.now, left_w).len();
        let right_h = self.build_right_lines(ch, right_w).len();
        left_h.max(right_h) as u16
    }

    /// Convenience wrapper: compute inner rows needed given the full overlay
    /// width (borders included). Returns 1 (for "fetching…") when no data yet.
    pub fn compact_content_height_for_state(&self, state: &AppState, overlay_width: u16) -> u16 {
        let inner_width = overlay_width.saturating_sub(2);
        if let Some(ch) = self.channel_data(state) {
            self.compact_content_height(ch, inner_width)
        } else {
            1
        }
    }

    /// Draw compact variant used by the hover overlay.
    /// Two-column layout: show info left, schedule right.
    /// The overlay rect is expected to already be content-sized.
    pub fn draw_compact(&mut self, frame: &mut Frame, area: Rect, focused: bool, state: &AppState) {
        if area.height == 0 {
            return;
        }

        // Clear the area first so station-list characters don't show through.
        frame.render_widget(Clear, area);

        let title = if self.channel == 0 { "nts 1" } else { "nts 2" };
        let block = pane_chrome_borders(title, self.number_key, focused, None, self.borders)
            .style(Style::default().bg(C_FILTER_BG));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(ch) = self.channel_data(state) else {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "  fetching NTS data…",
                    Style::default().fg(C_MUTED),
                ))
                .style(Style::default().bg(C_FILTER_BG)),
                inner,
            );
            return;
        };

        // Split inner area into left (show info) and right (schedule)
        let (left_w, right_w) = compact_col_widths(inner.width);
        // Add 1-char gutter between columns
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(left_w),
                Constraint::Length(1),
                Constraint::Length(right_w),
            ])
            .split(inner);

        let left_area = cols[0];
        let right_area = cols[2];

        let left_lines = self.build_left_lines(&ch.now, left_w);
        let right_lines = self.build_right_lines(ch, right_w);

        frame.render_widget(
            Paragraph::new(left_lines)
                .style(Style::default().bg(C_FILTER_BG))
                .wrap(Wrap { trim: false }),
            left_area,
        );
        frame.render_widget(
            Paragraph::new(right_lines)
                .style(Style::default().bg(C_FILTER_BG))
                .wrap(Wrap { trim: false }),
            right_area,
        );
    }
}

impl Component for NtsPanel {
    fn id(&self) -> ComponentId {
        ComponentId::NtsPanel
    }

    fn handle_key(&mut self, key: KeyEvent, _state: &AppState) -> Vec<Action> {
        if key.kind == KeyEventKind::Release {
            return vec![];
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll += 1;
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.scroll_x = self.scroll_x.saturating_sub(4);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.scroll_x += 4;
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
            }
            KeyCode::PageDown => {
                self.scroll += 10;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.scroll = 0;
                self.scroll_x = 0;
            }
            _ => {}
        }
        vec![]
    }

    fn handle_mouse(&mut self, event: MouseEvent, _area: Rect, _state: &AppState) -> Vec<Action> {
        use ratatui::crossterm::event::KeyModifiers;
        match event.kind {
            MouseEventKind::ScrollUp => {
                if event.modifiers.contains(KeyModifiers::SHIFT) {
                    self.scroll_x = self.scroll_x.saturating_sub(4);
                } else {
                    self.scroll = self.scroll.saturating_sub(1);
                }
            }
            MouseEventKind::ScrollDown => {
                if event.modifiers.contains(KeyModifiers::SHIFT) {
                    self.scroll_x += 4;
                } else {
                    self.scroll += 1;
                }
            }
            _ => {}
        }
        vec![]
    }

    fn on_action(&mut self, action: &Action, state: &AppState) -> Vec<Action> {
        match action {
            Action::ToggleNts(ch) if *ch == self.channel => {
                self.scroll = 0; // reset scroll when toggled
                self.scroll_x = 0;
            }
            _ => {}
        }
        let _ = state;
        vec![]
    }

    fn collapse_summary(&self, state: &AppState) -> Option<String> {
        self.channel_data(state)
            .map(|ch| ch.now.broadcast_title.clone())
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, focused: bool, state: &AppState) {
        if area.height == 0 {
            return;
        }

        let title = if self.channel == 0 { "nts 1" } else { "nts 2" };
        let block = pane_chrome_borders(title, self.number_key, focused, None, self.borders);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if let Some(err) = self.channel_error(state) {
            if self.channel_data(state).is_none() {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        format!("  error: {}", err),
                        Style::default().fg(C_ACCENT),
                    )),
                    inner,
                );
                return;
            }
        }

        let Some(ch) = self.channel_data(state) else {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "  fetching NTS data…",
                    Style::default().fg(C_MUTED),
                )),
                inner,
            );
            return;
        };

        let lines = self.build_lines(ch, inner.width);

        // Clamp scroll
        let max_scroll = lines.len().saturating_sub(inner.height as usize);
        if self.scroll > max_scroll {
            self.scroll = max_scroll;
        }

        // Use wrapping only when not scrolled horizontally — ratatui ignores
        // the column scroll offset when Wrap is active.
        let para = if self.scroll_x == 0 {
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((self.scroll as u16, 0))
        } else {
            Paragraph::new(lines).scroll((self.scroll as u16, self.scroll_x as u16))
        };
        frame.render_widget(para, inner);
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Compute left/right column widths for the compact two-column layout.
/// Left gets ~60%, right gets ~40%, each with a minimum of 16 chars.
fn compact_col_widths(inner_width: u16) -> (u16, u16) {
    // Reserve 1 char for the gutter between columns
    let available = inner_width.saturating_sub(1);
    let left_w = ((available as f32 * 0.60) as u16)
        .max(16)
        .min(available.saturating_sub(12));
    let right_w = available.saturating_sub(left_w);
    (left_w, right_w)
}

fn fmt_time_range(show: &NtsShow) -> String {
    let start = show.start.format("%H:%M").to_string();
    let end = show.end.format("%H:%M").to_string();
    format!("{} – {}", start, end)
}

fn word_wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
            } else if current.len() + 1 + word.len() <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(current.clone());
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
        if paragraph.is_empty() {
            lines.push(String::new());
        }
    }
    lines
}
