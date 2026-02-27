//! StationList component — left pane in Radio workspace.

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState},
    Frame,
};

use radio_proto::protocol::{PlaybackStatus, Station};
use ratatui::widgets::Borders;
use std::time::Instant;

use crate::{
    action::{Action, ComponentId, StarContext},
    app_state::AppState,
    component::Component,
    intent::RenderHint,
    theme::{
        C_BADGE_ERR, C_BADGE_PENDING, C_CONNECTING, C_LOCATION, C_MUTED, C_NETWORK, C_PLAYING,
        C_PRIMARY, C_SECONDARY, C_SELECTION_BG, C_STARS, C_TAG,
    },
    widgets::{
        filter_input::{FilterAction, FilterInput},
        pane_chrome::pane_chrome_borders,
        scrollable_list::ScrollableList,
    },
};

/// Sort order for the station list.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SortOrder {
    #[default]
    Default,
    Network,
    Location,
    Name,
    Stars,
    Recent,
    StarsRecent,
    RecentStars,
}

impl SortOrder {
    pub fn next(self) -> Self {
        match self {
            Self::Default => Self::Network,
            Self::Network => Self::Location,
            Self::Location => Self::Name,
            Self::Name => Self::Stars,
            Self::Stars => Self::Recent,
            Self::Recent => Self::StarsRecent,
            Self::StarsRecent => Self::RecentStars,
            Self::RecentStars => Self::Default,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Default => Self::RecentStars,
            Self::Network => Self::Default,
            Self::Location => Self::Network,
            Self::Name => Self::Location,
            Self::Stars => Self::Name,
            Self::Recent => Self::Stars,
            Self::StarsRecent => Self::Recent,
            Self::RecentStars => Self::StarsRecent,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Network => "network",
            Self::Location => "location",
            Self::Name => "name",
            Self::Stars => "stars",
            Self::Recent => "recent",
            Self::StarsRecent => "stars+recent",
            Self::RecentStars => "recent+stars",
        }
    }
}

pub struct StationList {
    pub list: ScrollableList<Station>,
    pub filter_input: FilterInput,
    pub sort_order: SortOrder,
    list_state: ListState,
    /// When set, jump-to this station on next state update.
    pub jump_from_station: Option<Option<usize>>,
    /// Which borders to draw (for collapsed/shared-border layouts).
    pub borders: Borders,
    /// Track last click (row index, time) for double-click detection.
    last_click: Option<(usize, Instant)>,
}

impl StationList {
    pub fn new() -> Self {
        Self {
            list: ScrollableList::new(|station: &Station, q: &str| station_matches(station, q)),
            filter_input: FilterInput::new("station name, network, location, tags…"),
            sort_order: SortOrder::Default,
            list_state: ListState::default(),
            jump_from_station: None,
            borders: Borders::ALL,
            last_click: None,
        }
    }

    /// Update items from daemon state and re-apply sort+filter.
    pub fn sync_stations(&mut self, state: &AppState) {
        let stations = state.daemon_state.stations.clone();
        self.list.set_items(stations);
        self.apply_sort(state);
    }

    fn apply_sort(&mut self, state: &AppState) {
        match self.sort_order {
            SortOrder::Default => {
                // restore original toml order — rebuild_filter handles this
                self.list.rebuild_filter();
            }
            SortOrder::Network => {
                self.list.sort_by(|a, b| {
                    a.network
                        .to_lowercase()
                        .cmp(&b.network.to_lowercase())
                        .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
            }
            SortOrder::Location => {
                self.list.sort_by(|a, b| {
                    a.country
                        .to_lowercase()
                        .cmp(&b.country.to_lowercase())
                        .then(a.city.to_lowercase().cmp(&b.city.to_lowercase()))
                        .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
            }
            SortOrder::Name => {
                self.list
                    .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            }
            SortOrder::Stars => {
                let stars = state.station_stars.clone();
                self.list.sort_by(move |a, b| {
                    let sa = stars.get(&a.name).copied().unwrap_or(0);
                    let sb = stars.get(&b.name).copied().unwrap_or(0);
                    sb.cmp(&sa)
                        .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
            }
            SortOrder::Recent => {
                let recent = state.recent_station.clone();
                self.list.sort_by(move |a, b| {
                    let ra = recent.get(&a.name).copied().unwrap_or(0);
                    let rb = recent.get(&b.name).copied().unwrap_or(0);
                    rb.cmp(&ra)
                        .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
            }
            SortOrder::StarsRecent => {
                let stars = state.station_stars.clone();
                let recent = state.recent_station.clone();
                self.list.sort_by(move |a, b| {
                    let sa = stars.get(&a.name).copied().unwrap_or(0);
                    let sb = stars.get(&b.name).copied().unwrap_or(0);
                    let ra = recent.get(&a.name).copied().unwrap_or(0);
                    let rb = recent.get(&b.name).copied().unwrap_or(0);
                    sb.cmp(&sa).then(rb.cmp(&ra))
                });
            }
            SortOrder::RecentStars => {
                let stars = state.station_stars.clone();
                let recent = state.recent_station.clone();
                self.list.sort_by(move |a, b| {
                    let sa = stars.get(&a.name).copied().unwrap_or(0);
                    let sb = stars.get(&b.name).copied().unwrap_or(0);
                    let ra = recent.get(&a.name).copied().unwrap_or(0);
                    let rb = recent.get(&b.name).copied().unwrap_or(0);
                    rb.cmp(&ra).then(sb.cmp(&sa))
                });
            }
        }
    }

    /// Select the station by original index in stations vec.
    pub fn select_by_station_idx(&mut self, idx: usize) {
        self.list.set_selected_by_original(idx);
    }

    /// Returns the original station index of the currently selected item.
    pub fn selected_station_idx(&self) -> Option<usize> {
        self.list.selected_original_index()
    }

    pub fn filter_query(&self) -> &str {
        self.list.filter.as_str()
    }

    pub fn is_filter_active(&self) -> bool {
        self.filter_input.is_active()
    }

    /// Returns the name of the currently selected station (from the original station list).
    pub fn selected_name<'a>(
        &self,
        stations: &'a [radio_proto::protocol::Station],
    ) -> Option<String> {
        self.selected_station_idx()
            .and_then(|i| stations.get(i))
            .map(|s| s.name.clone())
    }

    /// Current sort label string (for session persistence).
    pub fn sort_label(&self) -> &'static str {
        self.sort_order.label()
    }

    /// Restore sort order from a label string (session persistence).
    pub fn set_sort_from_label(&mut self, label: &str) {
        self.sort_order = match label {
            "network" => SortOrder::Network,
            "location" => SortOrder::Location,
            "name" => SortOrder::Name,
            "stars" => SortOrder::Stars,
            "recent" => SortOrder::Recent,
            "stars+recent" => SortOrder::StarsRecent,
            "recent+stars" => SortOrder::RecentStars,
            _ => SortOrder::Default,
        };
    }

    fn render_item<'a>(
        &self,
        station: &'a Station,
        orig_idx: usize,
        is_selected: bool,
        state: &AppState,
        network_count: &std::collections::HashMap<String, usize>,
    ) -> ListItem<'a> {
        let ds = &state.daemon_state;
        let is_current = ds.current_station == Some(orig_idx);
        let filtering = self.filter_input.is_active() || !self.list.filter.is_empty();

        let (base_icon, base_icon_color): (&'static str, Color) = if is_current {
            match ds.playback_status {
                PlaybackStatus::Playing => ("▶", C_PLAYING),
                PlaybackStatus::Paused => ("⏸", C_CONNECTING),
                PlaybackStatus::Connecting => ("⋯", C_CONNECTING),
                PlaybackStatus::Error => ("✗", crate::theme::C_ACCENT),
                PlaybackStatus::Idle => ("■", C_MUTED),
            }
        } else {
            (" ", C_MUTED)
        };

        // For the current station row, apply the station_hint overlay
        let (icon, icon_color): (&'static str, Color) = if is_current {
            match state.station_hint {
                RenderHint::PendingHidden => (" ", base_icon_color),
                RenderHint::PendingVisible => (base_icon, C_BADGE_PENDING),
                RenderHint::TimedOut => ("?", C_BADGE_ERR),
                RenderHint::Normal => (base_icon, base_icon_color),
            }
        } else {
            (base_icon, base_icon_color)
        };

        let name_color = if is_current {
            match ds.playback_status {
                PlaybackStatus::Playing => C_PLAYING,
                PlaybackStatus::Paused => C_CONNECTING,
                PlaybackStatus::Connecting => C_CONNECTING,
                PlaybackStatus::Error => crate::theme::C_ACCENT,
                PlaybackStatus::Idle => C_PRIMARY,
            }
        } else if is_selected {
            C_PRIMARY
        } else {
            C_SECONDARY
        };

        let name_style = if is_current || is_selected {
            Style::default().fg(name_color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(name_color)
        };

        let item_bg = if is_selected {
            Style::default().bg(C_SELECTION_BG)
        } else {
            Style::default()
        };

        let show_network = !station.network.is_empty()
            && network_count.get(&station.network).copied().unwrap_or(0) > 1;
        let location = station.city.clone();

        let stars = state.station_stars_for(&station.name).min(3);
        let star_prefix = if stars > 0 {
            format!("{} ", "✹".repeat(stars as usize))        } else {
            "  ".to_string()
        };

        let mut spans: Vec<Span> = vec![
            Span::styled(star_prefix, Style::default().fg(C_STARS)),
            Span::styled(icon, Style::default().fg(icon_color)),
            Span::raw("  "),
        ];

        if show_network {
            spans.push(Span::styled(
                station.network.clone(),
                Style::default().fg(C_NETWORK),
            ));
            spans.push(Span::styled(" · ", Style::default().fg(C_MUTED)));
        }

        spans.push(Span::styled(station.name.clone(), name_style));

        if !location.is_empty() {
            spans.push(Span::styled("  ", Style::default()));
            spans.push(Span::styled(location, Style::default().fg(C_LOCATION)));
        }

        // Tags — shown only on selected row while filtering
        if filtering && is_selected && !station.tags.is_empty() {
            spans.push(Span::styled("  ", Style::default()));
            for (i, tag) in station.tags.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(" · ", Style::default().fg(C_MUTED)));
                }
                spans.push(Span::styled(tag.clone(), Style::default().fg(C_TAG)));
            }
        }

        ListItem::new(Line::from(spans)).style(item_bg)
    }
}

fn station_matches(station: &Station, q: &str) -> bool {
    if q.trim().is_empty() {
        return true;
    }
    let q = q.to_lowercase();
    let text = format!(
        "{} {} {} {} {}",
        station.name.to_lowercase(),
        station.network.to_lowercase(),
        station.city.to_lowercase(),
        station.country.to_lowercase(),
        station.tags.join(" ").to_lowercase()
    );
    q.split_whitespace().all(|term| text.contains(term))
}

impl Component for StationList {
    fn id(&self) -> ComponentId {
        ComponentId::StationList
    }

    fn handle_key(&mut self, key: KeyEvent, state: &AppState) -> Vec<Action> {
        if key.kind == KeyEventKind::Release {
            return vec![];
        }

        // Filter mode input
        if self.filter_input.is_active() {
            match key.code {
                KeyCode::Up => {
                    self.list.select_up(1);
                    return vec![];
                }
                KeyCode::Down => {
                    self.list.select_down(1);
                    return vec![];
                }
                _ => {}
            }
            match self.filter_input.handle_key(key) {
                FilterAction::Changed(q) => {
                    self.list.set_filter(&q);
                    return vec![];
                }
                FilterAction::Confirmed => {
                    return vec![];
                }
                FilterAction::Cancelled => {
                    self.list.set_filter("");
                    return vec![Action::CloseFilter];
                }
                FilterAction::None => return vec![],
            }
        }

        let step = if key.modifiers.contains(KeyModifiers::SHIFT) {
            5
        } else {
            1
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.list.select_up(step);
                if let Some(idx) = self.list.selected_original_index() {
                    return vec![nts_hover_action(idx, state)];
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.list.select_down(step);
                if let Some(idx) = self.list.selected_original_index() {
                    return vec![nts_hover_action(idx, state)];
                }
            }
            KeyCode::PageUp => self.list.select_up(10),
            KeyCode::PageDown => self.list.select_down(10),
            KeyCode::Home | KeyCode::Char('g') => self.list.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.list.select_last(),

            KeyCode::Enter => {
                if let Some(idx) = self.list.selected_original_index() {
                    let is_current = state.daemon_state.current_station == Some(idx);
                    let is_active = is_current
                        && matches!(
                            state.daemon_state.playback_status,
                            PlaybackStatus::Playing | PlaybackStatus::Connecting
                        );
                    if is_active {
                        // Enter on the currently-playing station stops it.
                        return vec![Action::Stop];
                    } else {
                        // Enter on any other station (or same station when stopped/idle) plays it.
                        return vec![Action::Play(idx)];
                    }
                }
            }
            KeyCode::Char(' ') => {
                let is_station_active = matches!(
                    state.daemon_state.playback_status,
                    PlaybackStatus::Playing | PlaybackStatus::Connecting
                ) && state.daemon_state.current_station.is_some();
                let is_file_active =
                    state.daemon_state.current_file.is_some() && state.daemon_state.is_playing;

                if is_station_active || is_file_active {
                    // Space pauses/resumes whatever is currently playing.
                    return vec![Action::TogglePause];
                } else if let Some(idx) = self.list.selected_original_index() {
                    // Space when idle plays the selected station.
                    return vec![Action::Play(idx)];
                }
            }

            KeyCode::Char('/') => {
                self.filter_input.activate();
                return vec![Action::OpenFilter];
            }

            KeyCode::Char('s') => {
                self.sort_order = self.sort_order.next();
                self.apply_sort(state);
            }
            KeyCode::Char('S') => {
                self.sort_order = self.sort_order.prev();
                self.apply_sort(state);
            }

            KeyCode::Char('*') => {
                if let Some(st) = self.list.selected_item() {
                    let cur = state.station_stars_for(&st.name);
                    let next = (cur + 1) % 4;
                    return vec![Action::SetStar(next, StarContext::Station(st.name.clone()))];
                }
            }

            KeyCode::Char('n') => {
                self.jump_from_station = Some(state.daemon_state.current_station);
                return vec![Action::Next];
            }
            KeyCode::Char('p') => {
                self.jump_from_station = Some(state.daemon_state.current_station);
                return vec![Action::Prev];
            }
            KeyCode::Char('r') => {
                self.jump_from_station = Some(state.daemon_state.current_station);
                return vec![Action::Random];
            }

            KeyCode::Char('y') => {
                if let Some(st) = self.list.selected_item() {
                    return vec![Action::CopyToClipboard(st.url.clone())];
                }
            }

            _ => {}
        }

        vec![]
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: Rect, state: &AppState) -> Vec<Action> {
        let rel_row = event.row.saturating_sub(area.y + 1) as usize; // +1 for header
        match event.kind {
            MouseEventKind::ScrollUp => {
                self.list.select_up(1);
            }
            MouseEventKind::ScrollDown => {
                self.list.select_down(1);
            }
            MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left) => {
                let now = Instant::now();
                let is_double = self
                    .last_click
                    .map(|(row, t)| row == rel_row && t.elapsed().as_millis() < 400)
                    .unwrap_or(false);

                if self.list.handle_click(rel_row) {
                    if is_double {
                        // Double-click: play the station
                        self.last_click = None;
                        if let Some(idx) = self.list.selected_original_index() {
                            return vec![Action::Play(idx)];
                        }
                    } else {
                        self.last_click = Some((rel_row, now));
                        if let Some(idx) = self.list.selected_original_index() {
                            return vec![nts_hover_action(idx, state)];
                        }
                    }
                } else {
                    self.last_click = Some((rel_row, now));
                }
            }
            _ => {}
        }
        vec![]
    }

    fn on_action(&mut self, action: &Action, state: &AppState) -> Vec<Action> {
        match action {
            Action::FilterChanged(q) => {
                self.list.set_filter(q);
            }
            Action::ClearFilter => {
                self.list.set_filter("");
                self.filter_input.clear();
                self.filter_input.deactivate();
            }
            _ => {}
        }
        // Check for jump-to after shuffle/next/prev
        if let Some(from) = self.jump_from_station {
            if state.daemon_state.current_station != from {
                if let Some(idx) = state.daemon_state.current_station {
                    self.list.set_selected_by_original(idx);
                }
                self.jump_from_station = None;
            }
        }
        // Always re-emit hover state so app can keep nts_hover_channel in sync
        if let Some(idx) = self.list.selected_original_index() {
            vec![nts_hover_action(idx, state)]
        } else {
            vec![Action::HoverNts(None)]
        }
    }

    fn collapse_summary(&self, state: &AppState) -> Option<String> {
        // Show the currently selected station name, or the playing station
        let selected = self.list.selected_item().map(|s| s.name.as_str());
        let playing = state
            .daemon_state
            .current_station
            .and_then(|i| state.daemon_state.stations.get(i))
            .map(|s| s.name.as_str());
        let label = selected.or(playing)?;
        Some(label.to_string())
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, focused: bool, state: &AppState) {
        let block = pane_chrome_borders("stations", Some('1'), focused, None, self.borders);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if state.daemon_state.stations.is_empty() {
            let msg = if state.connected {
                "  no stations loaded"
            } else {
                "  connecting to daemon…"
            };
            frame.render_widget(
                ratatui::widgets::Paragraph::new(Span::styled(msg, Style::default().fg(C_MUTED))),
                inner,
            );
            return;
        }

        if self.list.is_empty() && !self.list.filter.is_empty() {
            frame.render_widget(
                ratatui::widgets::Paragraph::new(Span::styled(
                    "  no stations match filter",
                    Style::default().fg(C_MUTED),
                )),
                inner,
            );
            return;
        }

        // Count networks to determine which ones get the network label
        let mut network_count: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for s in &state.daemon_state.stations {
            if !s.network.is_empty() {
                *network_count.entry(s.network.clone()).or_insert(0) += 1;
            }
        }

        let content_h = inner.height as usize;
        self.list.ensure_visible(content_h);
        let items_with_idx: Vec<(usize, Station)> = self
            .list
            .visible_items(content_h)
            .into_iter()
            .map(|(i, s)| (i, s.clone()))
            .collect();
        let sel_in_view = self.list.selected_in_view(content_h);

        let items: Vec<ListItem> = items_with_idx
            .iter()
            .enumerate()
            .map(|(view_row, (orig_idx, station))| {
                let is_selected = view_row == sel_in_view;
                self.render_item(station, *orig_idx, is_selected, state, &network_count)
            })
            .collect();

        // Use a cloned station to avoid borrow issue
        let list = List::new(items)
            .highlight_style(Style::default())
            .highlight_symbol("");

        self.list_state.select(Some(sel_in_view));
        frame.render_stateful_widget(list, inner, &mut self.list_state);

        // Filter input bar drawn at bottom of inner area if active
        if self.filter_input.is_active() {
            let filter_area = Rect {
                y: inner.y + inner.height.saturating_sub(1),
                height: 1,
                ..inner
            };
            self.filter_input.draw(frame, filter_area);
        }
    }
}

impl StationList {
    fn update_nts_for_idx(&self, _idx: usize, _state: &AppState) {
        // Replaced by nts_hover_action() free function — kept for borrow-checker convenience.
    }
}

/// Produce the correct `HoverNts` action for a given station original-index.
fn nts_hover_action(orig_idx: usize, state: &AppState) -> Action {
    match state
        .daemon_state
        .stations
        .get(orig_idx)
        .map(|s| s.name.as_str())
    {
        Some("NTS 1") => Action::HoverNts(Some(0)),
        Some("NTS 2") => Action::HoverNts(Some(1)),
        _ => Action::HoverNts(None),
    }
}
