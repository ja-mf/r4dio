//! FileList component — left pane in Files workspace.

use std::collections::HashMap;
use std::path::PathBuf;

use radio_proto::protocol::PlaybackStatus;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState},
    Frame,
};

use crate::{
    action::{Action, ComponentId, StarContext},
    app_state::{AppState, FileMetadata, LocalFileEntry},
    component::Component,
    intent::RenderHint,
    theme::{
        C_ACCENT, C_BADGE_ERR, C_BADGE_PENDING, C_CONNECTING, C_LOCATION, C_MUTED, C_PLAYING,
        C_PRIMARY, C_SECONDARY, C_SELECTION_BG, C_STARS, C_TAG,
    },
    widgets::{
        filter_input::{FilterAction, FilterInput},
        pane_chrome::{pane_chrome_borders, Badge},
        scrollable_list::ScrollableList,
    },
};
use ratatui::widgets::Borders;

/// Sort order for the file list.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum FileSortOrder {
    #[default]
    Added, // most recently modified first
    Name,
    Stars,
    Recent,
    StarsRecent,
    RecentStars,
}

impl FileSortOrder {
    pub fn next(self) -> Self {
        match self {
            Self::Added => Self::Name,
            Self::Name => Self::Stars,
            Self::Stars => Self::Recent,
            Self::Recent => Self::StarsRecent,
            Self::StarsRecent => Self::RecentStars,
            Self::RecentStars => Self::Added,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Added => Self::RecentStars,
            Self::Name => Self::Added,
            Self::Stars => Self::Name,
            Self::Recent => Self::Stars,
            Self::StarsRecent => Self::Recent,
            Self::RecentStars => Self::StarsRecent,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Name => "name",
            Self::Stars => "stars",
            Self::Recent => "recent",
            Self::StarsRecent => "stars+recent",
            Self::RecentStars => "recent+stars",
        }
    }
}

/// Per-file search index (normalised lowercase text for fast filtering).
type SearchIndex = HashMap<String, String>;

pub struct FileList {
    pub list: ScrollableList<LocalFileEntry>,
    pub filter_input: FilterInput,
    pub sort_order: FileSortOrder,
    search_index: SearchIndex,
    list_state: ListState,
    index_cursor: usize,
    /// Which borders to draw (for collapsed/shared-border layouts).
    pub borders: Borders,
}

impl FileList {
    pub fn new() -> Self {
        Self {
            list: ScrollableList::new(|entry: &LocalFileEntry, _q: &str| {
                // Actual matching is done against the search_index externally
                true
            }),
            filter_input: FilterInput::new("filename, title, artist, genre…"),
            sort_order: FileSortOrder::Added,
            search_index: HashMap::new(),
            list_state: ListState::default(),
            index_cursor: 0,
            borders: Borders::ALL,
        }
    }

    /// Sync file list from AppState and re-apply sort/filter.
    pub fn sync_files(&mut self, state: &AppState) {
        self.list.set_items(state.files.clone());
        self.rebuild_search_index(&state.files, &state.file_metadata_cache);
        self.apply_sort(state);
    }

    fn rebuild_search_index(
        &mut self,
        files: &[LocalFileEntry],
        meta_cache: &HashMap<String, FileMetadata>,
    ) {
        self.search_index.clear();
        for f in files {
            let key = f.path.to_string_lossy().to_string();
            let mut text = format!("{} {}", f.name, f.path.to_string_lossy()).to_lowercase();
            if let Some(meta) = meta_cache.get(&key) {
                if let Some(v) = meta.title.as_deref() {
                    text.push_str(&format!(" {}", v.to_lowercase()));
                }
                if let Some(v) = meta.artist.as_deref() {
                    text.push_str(&format!(" {}", v.to_lowercase()));
                }
                if let Some(v) = meta.album.as_deref() {
                    text.push_str(&format!(" {}", v.to_lowercase()));
                }
                if let Some(v) = meta.genre.as_deref() {
                    text.push_str(&format!(" {}", v.to_lowercase()));
                }
                if let Some(v) = meta.description.as_deref() {
                    text.push_str(&format!(" {}", v.to_lowercase()));
                }
                for t in meta.tracklist.iter().take(120) {
                    text.push_str(&format!(" {}", t.to_lowercase()));
                }
                for ch in meta.chapters.iter().take(120) {
                    text.push_str(&format!(" {}", ch.title.to_lowercase()));
                }
            }
            self.search_index.insert(key, text);
        }
    }

    fn apply_sort(&mut self, state: &AppState) {
        match self.sort_order {
            FileSortOrder::Added => {
                self.list.sort_by(|a, b| {
                    b.modified
                        .cmp(&a.modified)
                        .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
            }
            FileSortOrder::Name => {
                self.list
                    .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            }
            FileSortOrder::Stars => {
                let stars = state.file_stars.clone();
                self.list.sort_by(move |a, b| {
                    let pa = a.path.to_string_lossy().to_string();
                    let pb = b.path.to_string_lossy().to_string();
                    let sa = stars.get(&pa).copied().unwrap_or(0);
                    let sb = stars.get(&pb).copied().unwrap_or(0);
                    sb.cmp(&sa)
                        .then(b.modified.cmp(&a.modified))
                        .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
            }
            FileSortOrder::Recent => {
                let recent = state.recent_file.clone();
                self.list.sort_by(move |a, b| {
                    let pa = a.path.to_string_lossy().to_string();
                    let pb = b.path.to_string_lossy().to_string();
                    let ra = recent.get(&pa).copied().unwrap_or(0);
                    let rb = recent.get(&pb).copied().unwrap_or(0);
                    rb.cmp(&ra).then(b.modified.cmp(&a.modified))
                });
            }
            FileSortOrder::StarsRecent => {
                let stars = state.file_stars.clone();
                let recent = state.recent_file.clone();
                self.list.sort_by(move |a, b| {
                    let pa = a.path.to_string_lossy().to_string();
                    let pb = b.path.to_string_lossy().to_string();
                    let sa = stars.get(&pa).copied().unwrap_or(0);
                    let sb = stars.get(&pb).copied().unwrap_or(0);
                    let ra = recent.get(&pa).copied().unwrap_or(0);
                    let rb = recent.get(&pb).copied().unwrap_or(0);
                    sb.cmp(&sa).then(rb.cmp(&ra))
                });
            }
            FileSortOrder::RecentStars => {
                let stars = state.file_stars.clone();
                let recent = state.recent_file.clone();
                self.list.sort_by(move |a, b| {
                    let pa = a.path.to_string_lossy().to_string();
                    let pb = b.path.to_string_lossy().to_string();
                    let sa = stars.get(&pa).copied().unwrap_or(0);
                    let sb = stars.get(&pb).copied().unwrap_or(0);
                    let ra = recent.get(&pa).copied().unwrap_or(0);
                    let rb = recent.get(&pb).copied().unwrap_or(0);
                    rb.cmp(&ra).then(sb.cmp(&sa))
                });
            }
        }

        // Apply current text filter on top of sort
        let q = self.list.filter.clone();
        if !q.is_empty() {
            self.apply_text_filter(&q);
        }
    }

    fn apply_text_filter(&mut self, q: &str) {
        let q_norm = q.to_lowercase();
        let idx = &self.search_index;
        self.list.filtered_indices = self
            .list
            .items
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                let key = f.path.to_string_lossy().to_string();
                let text = idx.get(&key).map(|s| s.as_str()).unwrap_or("");
                q_norm.split_whitespace().all(|term| text.contains(term))
            })
            .map(|(i, _)| i)
            .collect();
        if self.list.selected >= self.list.filtered_indices.len() {
            self.list.selected = self.list.filtered_indices.len().saturating_sub(1);
        }
    }

    pub fn selected_path(&self) -> Option<PathBuf> {
        self.list.selected_item().map(|f| f.path.clone())
    }

    /// Select the item with the given original index (for session restore).
    pub fn set_selected(&mut self, original_idx: usize) {
        self.list.set_selected_by_original(original_idx);
    }

    /// Current sort label string (for session persistence).
    pub fn sort_label(&self) -> &'static str {
        self.sort_order.label()
    }

    /// Restore sort order from a label string (session persistence).
    pub fn set_sort_from_label(&mut self, label: &str) {
        self.sort_order = match label {
            "added" => FileSortOrder::Added,
            "name" => FileSortOrder::Name,
            "stars" => FileSortOrder::Stars,
            "recent" => FileSortOrder::Recent,
            "stars+recent" => FileSortOrder::StarsRecent,
            "recent+stars" => FileSortOrder::RecentStars,
            _ => FileSortOrder::Added,
        };
    }

    pub fn selected_original_index(&self) -> Option<usize> {
        self.list.selected_original_index()
    }

    pub fn filter_query(&self) -> &str {
        self.list.filter.as_str()
    }
}

impl Component for FileList {
    fn id(&self) -> ComponentId {
        ComponentId::FileList
    }

    fn handle_key(&mut self, key: KeyEvent, state: &AppState) -> Vec<Action> {
        if key.kind == KeyEventKind::Release {
            return vec![];
        }

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
                    self.apply_text_filter(&q);
                    self.list.filter = q;
                    return vec![];
                }
                FilterAction::Confirmed => return vec![],
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
            KeyCode::Up | KeyCode::Char('k') => self.list.select_up(step),
            KeyCode::Down | KeyCode::Char('j') => self.list.select_down(step),
            KeyCode::PageUp => self.list.select_up(10),
            KeyCode::PageDown => self.list.select_down(10),
            KeyCode::Home | KeyCode::Char('g') => self.list.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.list.select_last(),

            KeyCode::Enter => {
                if let Some(f) = self.list.selected_item() {
                    let path = f.path.to_string_lossy().to_string();
                    let is_current = state.daemon_state.current_file.as_deref() == Some(&path);
                    if is_current {
                        return vec![Action::Stop];
                    } else {
                        let start = state.file_position_for(&path);
                        return vec![Action::PlayFileAt(path, start)];
                    }
                }
            }
            KeyCode::Char(' ') => {
                if state.daemon_state.current_station.is_some()
                    || state.daemon_state.current_file.is_some()
                {
                    return vec![Action::TogglePause];
                } else if let Some(f) = self.list.selected_item() {
                    let path = f.path.to_string_lossy().to_string();
                    let start = state.file_position_for(&path);
                    return vec![Action::PlayFileAt(path, start)];
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
                if let Some(f) = self.list.selected_item() {
                    let path = f.path.to_string_lossy().to_string();
                    let cur = state.file_stars_for(&path);
                    let next = (cur + 1) % 4;
                    return vec![Action::SetStar(next, StarContext::File(path))];
                }
            }

            KeyCode::Char('r') => return vec![Action::Random],
            KeyCode::Char('R') => return vec![Action::RandomBack],
            KeyCode::Char('y') => {
                if let Some(f) = self.list.selected_item() {
                    let text = f.path.to_string_lossy().to_string();
                    return vec![Action::CopyToClipboard(text)];
                }
            }

            _ => {}
        }

        vec![]
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: Rect, _state: &AppState) -> Vec<Action> {
        let rel_row = event.row.saturating_sub(area.y + 1) as usize;
        match event.kind {
            MouseEventKind::ScrollUp => self.list.select_up(1),
            MouseEventKind::ScrollDown => self.list.select_down(1),
            MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left) => {
                self.list.handle_click(rel_row);
            }
            _ => {}
        }
        vec![]
    }

    fn on_action(&mut self, action: &Action, state: &AppState) -> Vec<Action> {
        match action {
            Action::FilterChanged(q) => {
                self.apply_text_filter(q);
                self.list.filter = q.clone();
            }
            Action::ClearFilter => {
                self.list.set_filter("");
                self.filter_input.clear();
                self.filter_input.deactivate();
            }
            _ => {}
        }
        vec![]
    }

    fn collapse_summary(&self, _state: &AppState) -> Option<String> {
        self.selected_path()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, focused: bool, state: &AppState) {
        let block = pane_chrome_borders("files", Some('1'), focused, None, self.borders);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if state.files.is_empty() {
            frame.render_widget(
                ratatui::widgets::Paragraph::new(Span::styled(
                    "  no playable files in downloads directory",
                    Style::default().fg(C_MUTED),
                )),
                inner,
            );
            return;
        }

        if self.list.is_empty() && !self.list.filter.is_empty() {
            frame.render_widget(
                ratatui::widgets::Paragraph::new(Span::styled(
                    "  no files match filter",
                    Style::default().fg(C_MUTED),
                )),
                inner,
            );
            return;
        }

        let content_h = inner.height as usize;
        self.list.ensure_visible(content_h);
        let items_with_idx = self.list.visible_items(content_h);
        let sel_in_view = self.list.selected_in_view(content_h);

        let items: Vec<ListItem> = items_with_idx
            .iter()
            .enumerate()
            .map(|(view_row, (orig_idx, file))| {
                let is_selected = view_row == sel_in_view;
                let path = file.path.to_string_lossy().to_string();
                let is_current = state.daemon_state.current_file.as_deref() == Some(&path);

                let base_icon: &str = if is_current {
                    match state.daemon_state.playback_status {
                        PlaybackStatus::Playing => "▶",
                        PlaybackStatus::Paused => "⏸",
                        PlaybackStatus::Connecting => "⋯",
                        PlaybackStatus::Error => "✗",
                        PlaybackStatus::Idle => "■",
                    }
                } else {
                    " "
                };

                let base_icon_color = if is_current {
                    match state.daemon_state.playback_status {
                        PlaybackStatus::Playing => C_PLAYING,
                        PlaybackStatus::Paused => C_CONNECTING,
                        PlaybackStatus::Connecting => C_CONNECTING,
                        PlaybackStatus::Error => C_ACCENT,
                        _ => C_MUTED,
                    }
                } else {
                    C_MUTED
                };

                // Apply pause_hint overlay for the current file row
                let (icon, icon_color) = if is_current {
                    match state.pause_hint {
                        RenderHint::PendingHidden => (" ", base_icon_color),
                        RenderHint::PendingVisible => (base_icon, C_BADGE_PENDING),
                        RenderHint::TimedOut => ("?", C_BADGE_ERR),
                        RenderHint::Normal => (base_icon, base_icon_color),
                    }
                } else {
                    (base_icon, base_icon_color)
                };

                let name_color = if is_current {
                    match state.daemon_state.playback_status {
                        PlaybackStatus::Playing => C_PLAYING,
                        _ => C_PRIMARY,
                    }
                } else if is_selected {
                    C_PRIMARY
                } else {
                    C_SECONDARY
                };

                let stars = state.file_stars_for(&path).min(3);
                let star_prefix = if stars > 0 {
                    format!("{} ", "★".repeat(stars as usize))
                } else {
                    String::new()
                };

                let (genre, duration) = if let Some(meta) = state.file_metadata_cache.get(&path) {
                    let g = meta.genre.as_deref().unwrap_or("").to_string();
                    let d = meta
                        .duration_secs
                        .map(fmt_clock)
                        .unwrap_or_else(|| "--:--".to_string());
                    (g, d)
                } else {
                    (String::new(), "--:--".to_string())
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

                let line = Line::from(vec![
                    Span::styled(star_prefix, Style::default().fg(C_STARS)),
                    Span::styled(icon, Style::default().fg(icon_color)),
                    Span::raw("  "),
                    Span::styled(file.name.clone(), name_style),
                    if !genre.is_empty() {
                        Span::styled(format!("  {}", genre), Style::default().fg(C_LOCATION))
                    } else {
                        Span::raw("")
                    },
                    Span::styled(format!("  {}", duration), Style::default().fg(C_SECONDARY)),
                ]);

                ListItem::new(line).style(item_bg)
            })
            .collect();

        let list = List::new(items)
            .highlight_style(Style::default())
            .highlight_symbol("");

        self.list_state.select(Some(sel_in_view));
        frame.render_stateful_widget(list, inner, &mut self.list_state);

        // Filter bar at bottom when active
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
