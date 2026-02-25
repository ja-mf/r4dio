mod connection;
mod ui;

use radio_tui::shared::config::Config;
use radio_tui::shared::protocol::{Broadcast, Command, DaemonState, Message};
use radio_tui::shared::songs::{SongDatabase, SongEntry};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use rand::Rng;
use ratatui::{backend::CrosstermBackend, Terminal};
use ratatui::layout::Rect;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tracing::{info, warn};

// ── Internal event bus ───────────────────────────────────────────────────────

enum AppMessage {
    Event(Event),
    DaemonConnected,
    DaemonDisconnected(String),
    StateUpdated(DaemonState),
    IcyUpdated(Option<String>),
    Log(String),
    NtsUpdated(usize, NtsChannel), // channel index (0 or 1), data
    NtsError(usize, String),
}

// ── NTS data types ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct NtsShow {
    pub broadcast_title: String,
    pub start: chrono::DateTime<chrono::Local>,
    pub end: chrono::DateTime<chrono::Local>,
    pub location_short: String,
    pub location_long: String,
    pub description: String,
    pub genres: Vec<String>,
    pub moods: Vec<String>,
    pub is_replay: bool,
}

#[derive(Clone, Debug)]
pub struct NtsChannel {
    pub now: NtsShow,
    pub upcoming: Vec<NtsShow>,
    pub fetched_at: chrono::DateTime<chrono::Local>,
}

// ── ICY / songs ticker entry ─────────────────────────────────────────────────

/// A single displayed line in the ICY or songs ticker.
/// `display` is the formatted string shown in the UI.
#[derive(Clone)]
pub struct TickerEntry {
    /// The raw content (song title or "track - artist") used for deduplication.
    pub raw: String,
    /// The formatted display string including timestamp prefix.
    pub display: String,
    /// Station name, if known (from songs.csv comment field).
    pub station: Option<String>,
    /// Show name, if known (second segment of the comment field, e.g. NTS show title).
    pub show: Option<String>,
    /// URL extracted from songs comment (for download/open actions).
    pub url: Option<String>,
    /// Full raw comment field from songs.csv.
    pub comment: Option<String>,
}

/// Format a chrono NaiveDateTime into "dd/mm yyyy hh:mm" only including the
/// date part when it differs from a reference date (today / last entry).
fn format_timestamp(ts: chrono::DateTime<chrono::Local>) -> String {
    let now = chrono::Local::now();
    let today = now.date_naive();
    let ts_date = ts.date_naive();
    if ts_date == today {
        ts.format("%H:%M").to_string()
    } else {
        ts.format("%d/%m/%Y %H:%M").to_string()
    }
}

// ── Sort order ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SortOrder {
    #[default]
    Default,  // stations.toml order
    Added,    // filesystem recency (files)
    Network,  // network (alphabetical), then name
    Location, // country, then city, then name
    Name,     // station name alphabetical
    Stars,    // stars desc, then name
    Recent,   // recent listening desc
    StarsRecent, // stars desc, then recent
    RecentStars, // recent desc, then stars
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftPaneMode {
    Stations,
    Files,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    Left,
    Icy,
    Songs,
    Meta,
    Nts,
}

#[derive(Debug, Clone)]
pub struct LocalFileEntry {
    pub path: PathBuf,
    pub name: String,
    pub size_bytes: u64,
    pub modified: Option<std::time::SystemTime>,
}

#[derive(Debug, Clone, Default)]
pub struct FileChapter {
    pub title: String,
    pub start_secs: f64,
    pub end_secs: f64,
}

#[derive(Debug, Clone, Default)]
pub struct FileMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub date: Option<String>,
    pub description: Option<String>,
    pub genre: Option<String>,
    pub duration_secs: Option<f64>,
    pub codec: Option<String>,
    pub bitrate_kbps: Option<u64>,
    pub sample_rate_hz: Option<u32>,
    pub channels: Option<u8>,
    pub chapters: Vec<FileChapter>,
    pub tracklist: Vec<String>,
}

impl SortOrder {
    pub fn next_for_mode(self, mode: LeftPaneMode) -> Self {
        match mode {
            LeftPaneMode::Stations => match self {
                Self::Default => Self::Network,
                Self::Added => Self::Network,
                Self::Network => Self::Location,
                Self::Location => Self::Name,
                Self::Name => Self::Stars,
                Self::Stars => Self::Recent,
                Self::Recent => Self::StarsRecent,
                Self::StarsRecent => Self::RecentStars,
                Self::RecentStars => Self::Default,
            },
            LeftPaneMode::Files => match self {
                Self::Default => Self::Added,
                Self::Added => Self::Name,
                Self::Name => Self::Stars,
                Self::Stars => Self::Recent,
                Self::Recent => Self::StarsRecent,
                Self::StarsRecent => Self::RecentStars,
                Self::RecentStars => Self::Default,
                // Skip station-only sorts in files mode
                Self::Network | Self::Location => Self::Name,
            },
        }
    }

    pub fn prev_for_mode(self, mode: LeftPaneMode) -> Self {
        match mode {
            LeftPaneMode::Stations => match self {
                Self::Default => Self::RecentStars,
                Self::Added => Self::RecentStars,
                Self::Network => Self::Default,
                Self::Location => Self::Network,
                Self::Name => Self::Location,
                Self::Stars => Self::Name,
                Self::Recent => Self::Stars,
                Self::StarsRecent => Self::Recent,
                Self::RecentStars => Self::StarsRecent,
            },
            LeftPaneMode::Files => match self {
                Self::Default => Self::RecentStars,
                Self::Added => Self::Default,
                Self::Name => Self::Added,
                Self::Stars => Self::Name,
                Self::Recent => Self::Stars,
                Self::StarsRecent => Self::Recent,
                Self::RecentStars => Self::StarsRecent,
                Self::Network | Self::Location => Self::Default,
            },
        }
    }

    pub fn label_for_mode(self, mode: LeftPaneMode) -> &'static str {
        if mode == LeftPaneMode::Files {
            match self {
                Self::Network | Self::Location => return "default",
                _ => {}
            }
        }
        match self {
            Self::Default  => "default",
            Self::Added    => "added",
            Self::Network  => "network",
            Self::Location => "location",
            Self::Name     => "name",
            Self::Stars    => "stars",
            Self::Recent => "recent",
            Self::StarsRecent => "stars+recent",
            Self::RecentStars => "recent+stars",
        }
    }
}

// ── App state ────────────────────────────────────────────────────────────────

pub struct App {
    config: Config,
    pub connected: bool,
    pub state: DaemonState,
    pub selected_idx: usize,
    pub list_state: ratatui::widgets::ListState,
    pub logs: Vec<String>,
    pub log_file_lines: Vec<String>, // last N lines from tui.log, newest first
    pub log_path: PathBuf,
    /// Rolling ICY ticker, newest last (displayed newest-first in UI).
    pub icy_history: Vec<TickerEntry>,
    pub icy_log_path: PathBuf,
    /// Latest songs from ~/songs.csv, newest last (deduplicated).
    pub songs_history: Vec<TickerEntry>,
    pub songs_csv_path: PathBuf,
    pub songs_vds_path: PathBuf,
    pub left_mode: LeftPaneMode,
    pub focus_pane: FocusPane,
    pub songs_selected: usize,
    pub icy_selected: usize,
    pub meta_scroll: usize,
    pub files: Vec<LocalFileEntry>,
    pub file_filtered_indices: Vec<usize>,
    pub file_selected: usize,
    pub downloads_dir: PathBuf,
    pub files_left_full_width: bool,
    pub files_right_maximized: bool,
    pub selected_file_metadata: Option<FileMetadata>,
    pub file_metadata_cache: HashMap<String, FileMetadata>,
    pub file_search_index: HashMap<String, String>,
    pub file_index_cursor: usize,
    pub station_stars: HashMap<String, u8>,
    pub file_stars: HashMap<String, u8>,
    pub stars_path: PathBuf,
    pub random_history_path: PathBuf,
    pub random_history: Vec<RandomHistoryEntry>,
    pub recent_path: PathBuf,
    pub recent_station: HashMap<String, i64>,
    pub recent_file: HashMap<String, i64>,
    pub file_positions_path: PathBuf,
    pub file_positions: HashMap<String, f64>,
    pub ui_state_path: PathBuf,
    pub pending_station_restore: Option<String>,
    pub last_station_name: Option<String>,
    pub last_file_path: Option<String>,
    pub last_file_pos: f64,
    pub pending_resume_file: Option<(String, f64)>,
    pub filter_left: String,
    pub filter_icy: String,
    pub filter_songs: String,
    pub filter_meta: String,
    pub filter_target: FocusPane,
    pub left_pane_rect: Rect,
    pub upper_right_rect: Rect,
    pub middle_right_rect: Rect,
    pub lower_right_rect: Rect,
    pub station_view_start: usize,
    pub file_view_start: usize,
    pub icy_view_start: usize,
    pub songs_view_start: usize,
    pub last_nonzero_volume: f32,
    pub error_message: Option<String>,
    pub show_logs: bool,
    pub log_scroll: usize,
    pub show_help: bool,
    pub show_keys: bool,             // footer keybindings bar
    pub show_nts_ch1: bool,
    pub show_nts_ch2: bool,
    pub nts_ch1: Option<NtsChannel>,
    pub nts_ch2: Option<NtsChannel>,
    pub nts_ch1_error: Option<String>,
    pub nts_ch2_error: Option<String>,
    pub nts_scroll: usize,
    /// Current filter string (empty = no filter).
    pub filter: String,
    /// True when the filter input bar is active (user pressed /).
    pub filter_active: bool,
    /// Indices into state.stations that pass the current filter.
    /// When filter is empty, this mirrors all station indices.
    pub filtered_indices: Vec<usize>,
    /// Current sort order for the station list.
    pub sort_order: SortOrder,
    pub station_sort_order: SortOrder,
    pub file_sort_order: SortOrder,
    pub last_focus_pane: FocusPane,
    cmd_tx: Option<mpsc::Sender<Command>>,
    initial_loaded: bool,
    // When Some(station), jump to current_station only once it differs from this value
    jump_from_station: Option<Option<usize>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RandomHistoryEntry {
    pub path: String,
    pub start_secs: f64,
    pub saved_at_epoch: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct UiSessionState {
    left_mode: String,
    focus_pane: String,
    selected_station_name: Option<String>,
    selected_file_path: Option<String>,
    files_left_full_width: bool,
    files_right_maximized: bool,
    station_sort_order: String,
    file_sort_order: String,
    icy_selected: usize,
    songs_selected: usize,
    meta_scroll: usize,
    last_station_name: Option<String>,
    last_file_path: Option<String>,
    last_file_pos: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct RecentState {
    recent_station: HashMap<String, i64>,
    recent_file: HashMap<String, i64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct StarredState {
    station_stars: HashMap<String, u8>,
    file_stars: HashMap<String, u8>,
}

impl App {
    fn filter_for_pane(&self, pane: FocusPane) -> &str {
        match pane {
            FocusPane::Left => &self.filter_left,
            FocusPane::Icy => &self.filter_icy,
            FocusPane::Songs => &self.filter_songs,
            FocusPane::Meta => &self.filter_meta,
            FocusPane::Nts => &self.filter_left, // NTS pane doesn't filter
        }
    }

    fn set_filter_for_pane(&mut self, pane: FocusPane, value: String) {
        match pane {
            FocusPane::Left => self.filter_left = value,
            FocusPane::Icy => self.filter_icy = value,
            FocusPane::Songs => self.filter_songs = value,
            FocusPane::Meta => self.filter_meta = value,
            FocusPane::Nts => {} // NTS pane doesn't filter
        }
    }

    /// Get the name of the currently playing station
    fn current_station_name(&self) -> String {
        if let Some(idx) = self.state.current_station {
            if let Some(station) = self.state.stations.get(idx) {
                return station.name.clone();
            }
        }
        if let Some(ref path_str) = self.state.current_file {
            let path = std::path::Path::new(path_str);
            return path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Unknown".to_string());
        }
        "Unknown".to_string()
    }

    fn set_station_star(&mut self, station_name: &str, stars: u8) {
        if stars == 0 {
            self.station_stars.remove(station_name);
        } else {
            self.station_stars.insert(station_name.to_string(), stars.min(3));
        }
        let _ = save_stars(&self.stars_path, &self.station_stars, &self.file_stars);
    }

    fn set_file_star(&mut self, path: &str, stars: u8) {
        if stars == 0 {
            self.file_stars.remove(path);
        } else {
            self.file_stars.insert(path.to_string(), stars.min(3));
        }
        let _ = save_stars(&self.stars_path, &self.station_stars, &self.file_stars);
    }

    fn push_random_history_entry(&mut self, path: String, start_secs: f64) {
        self.random_history.push(RandomHistoryEntry {
            path,
            start_secs,
            saved_at_epoch: chrono::Local::now().timestamp(),
        });
        if self.random_history.len() > 200 {
            let drop_n = self.random_history.len() - 200;
            self.random_history.drain(0..drop_n);
        }
        let _ = save_random_history(&self.random_history_path, &self.random_history);
    }

    fn pop_random_history_entry(&mut self) -> Option<RandomHistoryEntry> {
        let item = self.random_history.pop();
        let _ = save_random_history(&self.random_history_path, &self.random_history);
        item
    }

    fn clear_filters_except(&mut self, pane: FocusPane) {
        if pane != FocusPane::Left {
            self.filter_left.clear();
            self.rebuild_filter();
            self.rebuild_file_filter();
        }
        if pane != FocusPane::Icy {
            self.filter_icy.clear();
            self.icy_selected = 0;
        }
        if pane != FocusPane::Songs {
            self.filter_songs.clear();
            self.songs_selected = 0;
        }
        if pane != FocusPane::Meta {
            self.filter_meta.clear();
            self.meta_scroll = 0;
        }
    }

    fn save_ui_session_state(&self) {
        let selected_station_name = self.state.stations.get(self.selected_idx).map(|s| s.name.clone());
        let selected_file_path = self
            .files
            .get(self.file_selected)
            .map(|f| f.path.to_string_lossy().to_string());
        let state = UiSessionState {
            left_mode: match self.left_mode {
                LeftPaneMode::Stations => "stations".to_string(),
                LeftPaneMode::Files => "files".to_string(),
            },
            focus_pane: match self.focus_pane {
                FocusPane::Left => "left".to_string(),
                FocusPane::Icy => "icy".to_string(),
                FocusPane::Songs => "songs".to_string(),
                FocusPane::Meta => "meta".to_string(),
                FocusPane::Nts => "left".to_string(), // restore to left on reload
            },
            selected_station_name,
            selected_file_path,
            files_left_full_width: self.files_left_full_width,
            files_right_maximized: self.files_right_maximized,
            station_sort_order: self.station_sort_order.label_for_mode(LeftPaneMode::Stations).to_string(),
            file_sort_order: self.file_sort_order.label_for_mode(LeftPaneMode::Files).to_string(),
            icy_selected: self.icy_selected,
            songs_selected: self.songs_selected,
            meta_scroll: self.meta_scroll,
            last_station_name: self.last_station_name.clone(),
            last_file_path: self.last_file_path.clone(),
            last_file_pos: self.last_file_pos,
        };
        let _ = save_ui_session_state(&self.ui_state_path, &state);
    }

    fn rebuild_file_index_base(&mut self) {
        self.file_search_index.clear();
        for f in &self.files {
            let key = f.path.to_string_lossy().to_string();
            let base = normalize_search_text(&format!("{} {}", f.name, f.path.to_string_lossy()));
            self.file_search_index.insert(key, base);
        }
        self.file_index_cursor = 0;
    }

    fn index_one_file_metadata(&mut self) {
        if self.files.is_empty() {
            return;
        }
        let idx = self.file_index_cursor % self.files.len();
        self.file_index_cursor = (self.file_index_cursor + 1) % self.files.len();
        let Some(file) = self.files.get(idx) else {
            return;
        };
        let key = file.path.to_string_lossy().to_string();
        if !self.file_metadata_cache.contains_key(&key) {
            if let Some(meta) = probe_file_metadata(&file.path) {
                self.file_metadata_cache.insert(key.clone(), meta);
            }
        }
        if let Some(meta) = self.file_metadata_cache.get(&key) {
            let mut text = format!("{} {}", file.name, file.path.to_string_lossy());
            if let Some(v) = meta.title.as_deref() { text.push_str(&format!(" {}", v)); }
            if let Some(v) = meta.artist.as_deref() { text.push_str(&format!(" {}", v)); }
            if let Some(v) = meta.album.as_deref() { text.push_str(&format!(" {}", v)); }
            if let Some(v) = meta.date.as_deref() { text.push_str(&format!(" {}", v)); }
            if let Some(v) = meta.genre.as_deref() { text.push_str(&format!(" {}", v)); }
            if let Some(v) = meta.description.as_deref() { text.push_str(&format!(" {}", v)); }
            for it in meta.tracklist.iter().take(120) { text.push_str(&format!(" {}", it)); }
            for ch in meta.chapters.iter().take(120) { text.push_str(&format!(" {}", ch.title)); }
            self.file_search_index.insert(key, normalize_search_text(&text));
        }
    }

    fn apply_filter_for_pane(&mut self, pane: FocusPane) {
        if pane == FocusPane::Left {
            if self.left_mode == LeftPaneMode::Stations {
                self.rebuild_filter();
            } else {
                self.rebuild_file_filter();
            }
        }
    }

    fn clamp_filtered_selection(&mut self) {
        let icy_total = self.icy_visible_indices().len();
        if self.icy_selected >= icy_total {
            self.icy_selected = icy_total.saturating_sub(1);
        }
        let songs_total = self.songs_visible_indices().len();
        if self.songs_selected >= songs_total {
            self.songs_selected = songs_total.saturating_sub(1);
        }
    }

    fn icy_visible_indices(&self) -> Vec<usize> {
        let q = self.filter_icy.as_str();
        let mut out = Vec::new();
        for i in (0..self.icy_history.len()).rev() {
            let e = &self.icy_history[i];
            let mut text = e.display.clone();
            if let Some(st) = e.station.as_deref() {
                text.push(' ');
                text.push_str(st);
            }
            if search_matches(q, &text) {
                out.push(i);
            }
        }
        out
    }

    fn songs_visible_indices(&self) -> Vec<usize> {
        let q = self.filter_songs.as_str();
        let mut out = Vec::new();
        for i in (0..self.songs_history.len()).rev() {
            let e = &self.songs_history[i];
            let mut text = e.display.clone();
            if let Some(st) = e.station.as_deref() {
                text.push(' ');
                text.push_str(st);
            }
            if let Some(show) = e.show.as_deref() {
                text.push(' ');
                text.push_str(show);
            }
            if let Some(comment) = e.comment.as_deref() {
                text.push(' ');
                text.push_str(comment);
            }
            if search_matches(q, &text) {
                out.push(i);
            }
        }
        out
    }

    fn pick_random_file_pos(&self, len: usize) -> usize {
        if len <= 1 {
            return 0;
        }
        let mut rng = rand::thread_rng();
        let roll = rng.gen_range(0..100u8);
        // Mixed distribution:
        // - 40% uniform over full range (so beginning is always possible)
        // - 20% early bucket
        // - 20% late bucket
        // - 20% neighborhood around current selection
        if roll < 40 {
            return rng.gen_range(0..len);
        }
        if roll < 60 {
            let end = (len / 5).max(1);
            return rng.gen_range(0..end);
        }
        if roll < 80 {
            let start = len.saturating_sub((len / 5).max(1));
            return rng.gen_range(start..len);
        }
        let current_pos = self
            .file_filtered_indices
            .iter()
            .position(|&i| i == self.file_selected)
            .unwrap_or(0)
            .min(len - 1);
        let radius = (len / 6).max(1);
        let start = current_pos.saturating_sub(radius);
        let end = (current_pos + radius + 1).min(len);
        rng.gen_range(start..end)
    }

    fn pick_random_start_secs(&self, duration_secs: f64) -> f64 {
        if duration_secs <= 20.0 {
            return 0.0;
        }
        let max = (duration_secs - 10.0).max(0.0);
        let mut rng = rand::thread_rng();
        let roll = rng.gen_range(0..100u8);
        // Time distribution:
        // - 40% uniform over full duration
        // - 30% middle-biased (triangular around center)
        // - 15% early quarter
        // - 15% late quarter
        if roll < 40 {
            return rng.gen_range(0.0..max);
        }
        if roll < 70 {
            let u1: f64 = rng.gen_range(0.0..1.0);
            let u2: f64 = rng.gen_range(0.0..1.0);
            return ((u1 + u2) * 0.5) * max;
        }
        if roll < 85 {
            return rng.gen_range(0.0..(max * 0.25).max(1.0));
        }
        rng.gen_range((max * 0.75).min(max)..max.max(1.0))
    }

    fn set_focus_pane(&mut self, next: FocusPane) {
        if self.focus_pane != next {
            self.last_focus_pane = self.focus_pane;
            self.focus_pane = next;
        }
        self.filter = self.filter_for_pane(self.focus_pane).to_string();
    }

    /// Update NTS panel visibility based on the currently selected (highlighted) station.
    /// Called whenever selected_idx changes in stations mode.
    fn update_nts_panel_for_selection(&mut self) {
        if self.left_mode != LeftPaneMode::Stations {
            return;
        }
        let name = self.state.stations.get(self.selected_idx).map(|s| s.name.as_str());
        match name {
            Some("NTS 1") => {
                let was_showing = self.show_nts_ch1 || self.show_nts_ch2;
                self.show_nts_ch1 = true;
                self.show_nts_ch2 = false;
                if !was_showing {
                    self.nts_scroll = 0;
                }
            }
            Some("NTS 2") => {
                let was_showing = self.show_nts_ch1 || self.show_nts_ch2;
                self.show_nts_ch2 = true;
                self.show_nts_ch1 = false;
                if !was_showing {
                    self.nts_scroll = 0;
                }
            }
            _ => {
                self.show_nts_ch1 = false;
                self.show_nts_ch2 = false;
                // If the NTS pane was focused, move focus back to Left
                if self.focus_pane == FocusPane::Nts {
                    self.set_focus_pane(FocusPane::Left);
                }
            }
        }
    }
    fn new(
        config: Config,
        log_path: PathBuf,
        icy_log_path: PathBuf,
        songs_csv_path: PathBuf,
        songs_vds_path: PathBuf,
        downloads_dir: PathBuf,
        stars_path: PathBuf,
        random_history_path: PathBuf,
        recent_path: PathBuf,
        file_positions_path: PathBuf,
        ui_state_path: PathBuf,
    ) -> Self {
        let mut list_state = ratatui::widgets::ListState::default();
        list_state.select(Some(0));

        let icy_history = load_icy_log(&icy_log_path);
        let songs_history = load_songs_csv(&songs_csv_path);
        let files = load_local_files(&downloads_dir);
        let (station_stars, file_stars) = load_stars(&stars_path);
        let random_history = load_random_history(&random_history_path);
        let recent_state = load_recent_state(&recent_path);
        let file_positions = load_file_positions(&file_positions_path);
        let ui_state = load_ui_session_state(&ui_state_path);

        let mut app = Self {
            config,
            connected: false,
            state: DaemonState::default(),
            selected_idx: 0,
            list_state,
            logs: Vec::new(),
            log_file_lines: Vec::new(),
            log_path,
            icy_history,
            icy_log_path,
            songs_history,
            songs_csv_path,
            songs_vds_path,
            left_mode: LeftPaneMode::Stations,
            focus_pane: FocusPane::Left,
            songs_selected: 0,
            icy_selected: 0,
            meta_scroll: 0,
            files,
            file_filtered_indices: Vec::new(),
            file_selected: 0,
            downloads_dir,
            files_left_full_width: false,
            files_right_maximized: true,
            selected_file_metadata: None,
            file_metadata_cache: HashMap::new(),
            file_search_index: HashMap::new(),
            file_index_cursor: 0,
            station_stars,
            file_stars,
            stars_path,
            random_history_path,
            random_history,
            recent_path,
            recent_station: recent_state.recent_station,
            recent_file: recent_state.recent_file,
            file_positions_path,
            file_positions,
            ui_state_path,
            pending_station_restore: ui_state.selected_station_name.clone(),
            last_station_name: ui_state.last_station_name.clone(),
            last_file_path: ui_state.last_file_path.clone(),
            last_file_pos: ui_state.last_file_pos.max(0.0),
            pending_resume_file: ui_state
                .last_file_path
                .clone()
                .map(|p| (p, ui_state.last_file_pos.max(0.0))),
            filter_left: String::new(),
            filter_icy: String::new(),
            filter_songs: String::new(),
            filter_meta: String::new(),
            filter_target: FocusPane::Left,
            left_pane_rect: Rect::default(),
            upper_right_rect: Rect::default(),
            middle_right_rect: Rect::default(),
            lower_right_rect: Rect::default(),
            station_view_start: 0,
            file_view_start: 0,
            icy_view_start: 0,
            songs_view_start: 0,
            last_nonzero_volume: 0.7,
            error_message: None,
            show_logs: false,
            log_scroll: 0,
            show_help: false,
            show_keys: false,
            show_nts_ch1: false,
            show_nts_ch2: false,
            nts_ch1: None,
            nts_ch2: None,
            nts_ch1_error: None,
            nts_ch2_error: None,
            nts_scroll: 0,
            filter: String::new(),
            filter_active: false,
            filtered_indices: Vec::new(),
            sort_order: SortOrder::Default,
            station_sort_order: parse_sort_label_for_mode(&ui_state.station_sort_order, LeftPaneMode::Stations),
            file_sort_order: parse_sort_label_for_mode(&ui_state.file_sort_order, LeftPaneMode::Files),
            last_focus_pane: FocusPane::Left,
            cmd_tx: None,
            initial_loaded: false,
            jump_from_station: None,
        };
        app.left_mode = if ui_state.left_mode.eq_ignore_ascii_case("files")
            || (ui_state.last_file_path.is_some() && !ui_state.left_mode.eq_ignore_ascii_case("stations"))
        {
            LeftPaneMode::Files
        } else {
            LeftPaneMode::Stations
        };
        app.focus_pane = match ui_state.focus_pane.to_lowercase().as_str() {
            "icy" => FocusPane::Icy,
            "songs" => FocusPane::Songs,
            "meta" => FocusPane::Meta,
            _ => FocusPane::Left,
        };
        app.files_right_maximized = ui_state.files_right_maximized;
        app.files_left_full_width = ui_state.files_left_full_width;
        app.icy_selected = ui_state.icy_selected;
        app.songs_selected = ui_state.songs_selected;
        app.meta_scroll = ui_state.meta_scroll;
        app.sort_order = if app.left_mode == LeftPaneMode::Stations {
            app.station_sort_order
        } else {
            app.file_sort_order
        };
        if let Some(path) = ui_state
            .selected_file_path
            .as_deref()
            .or(ui_state.last_file_path.as_deref())
        {
            if let Some((idx, _)) = app
                .files
                .iter()
                .enumerate()
                .find(|(_, f)| f.path.to_string_lossy() == path)
            {
                app.file_selected = idx;
            }
        }
        app.refresh_selected_file_metadata();
        app.rebuild_file_index_base();
        app.rebuild_file_filter();
        app
    }

    async fn run(mut self) -> anyhow::Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let (tx, mut rx) = mpsc::channel::<AppMessage>(100);
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(100);
        self.cmd_tx = Some(cmd_tx);
        let _ = export_songs_vds(&self.songs_vds_path, &self.songs_history);

        // Keyboard event reader — spawn_blocking because event::read() is blocking
        let event_tx = tx.clone();
        tokio::task::spawn_blocking(move || {
            loop {
                match event::read() {
                    Ok(ev) => {
                        if event_tx.blocking_send(AppMessage::Event(ev)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Daemon connection handler
        let daemon_addr = radio_tui::shared::platform::daemon_address();
        let msg_tx = tx.clone();
        tokio::spawn(async move {
            connection_handler(daemon_addr, msg_tx, cmd_rx).await;
        });

        // Periodic log-file refresh (every 2 s, only when log panel is open)
        let mut log_refresh = tokio::time::interval(Duration::from_secs(2));
        log_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Periodic songs.csv refresh (every 10 s)
        let mut songs_refresh = tokio::time::interval(Duration::from_secs(10));
        songs_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Periodic local file list refresh (every 5 s)
        let mut files_refresh = tokio::time::interval(Duration::from_secs(5));
        files_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // NTS API refresh (every 10 s, triggers immediately on first tick)
        let mut nts_refresh = tokio::time::interval(Duration::from_secs(10));
        nts_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Main loop
        loop {
            terminal.draw(|f| ui::draw(f, &mut self))?;

            tokio::select! {
                Some(msg) = rx.recv() => {
                    match msg {
                        AppMessage::Event(event) => {
                            match event {
                                Event::Key(key) => {
                                    if self.handle_key(key).await? {
                                        self.save_ui_session_state();
                                        break;
                                    }
                                    self.save_ui_session_state();
                                }
                                Event::Mouse(mouse) => {
                                    self.handle_mouse(mouse);
                                    self.save_ui_session_state();
                                }
                                _ => {}
                            }
                        }
                        AppMessage::DaemonConnected => {
                            self.connected = true;
                            self.error_message = None;
                            self.log("connected to daemon".to_string());
                        }
                        AppMessage::DaemonDisconnected(reason) => {
                            self.connected = false;
                            self.error_message = Some(format!("disconnected: {}", reason));
                            self.log(format!("disconnected: {}", reason));
                        }
                        AppMessage::StateUpdated(state) => {
                            let was_empty = self.state.stations.is_empty();
                            let prev_station = self.state.current_station;
                            // Preserve NTS cities that were updated from API before overwriting state
                            let nts1_city = self.state.stations.iter()
                                .find(|s| s.name == "NTS 1")
                                .map(|s| s.city.clone());
                            let nts2_city = self.state.stations.iter()
                                .find(|s| s.name == "NTS 2")
                                .map(|s| s.city.clone());
                            self.state = state;
                            // Restore preserved NTS cities if they were set
                            if let Some(city) = nts1_city {
                                if let Some(s) = self.state.stations.iter_mut().find(|s| s.name == "NTS 1") {
                                    s.city = city;
                                }
                            }
                            if let Some(city) = nts2_city {
                                if let Some(s) = self.state.stations.iter_mut().find(|s| s.name == "NTS 2") {
                                    s.city = city;
                                }
                            }
                            let now_ts = chrono::Local::now().timestamp();
                            if let Some(i) = self.state.current_station {
                                if let Some(st) = self.state.stations.get(i) {
                                    self.last_station_name = Some(st.name.clone());
                                    self.recent_station.insert(st.name.clone(), now_ts);
                                }
                            }
                            if let Some(path) = self.state.current_file.clone() {
                                self.last_file_path = Some(path);
                                if let Some(p) = self.last_file_path.clone() {
                                    self.recent_file.insert(p, now_ts);
                                }
                                if let Some(pos) = self.state.time_pos_secs {
                                    self.last_file_pos = pos.max(0.0);
                                    if let Some(fp) = self.last_file_path.clone() {
                                        self.file_positions.insert(fp, self.last_file_pos);
                                    }
                                }
                            }
                            if self.state.volume > 0.001 {
                                self.last_nonzero_volume = self.state.volume;
                            }
                            if !self.state.stations.is_empty() {
                                if let Some(name) = self.pending_station_restore.take() {
                                    if let Some((idx, _)) = self
                                        .state
                                        .stations
                                        .iter()
                                        .enumerate()
                                        .find(|(_, s)| s.name == name)
                                    {
                                        self.selected_idx = idx;
                                    }
                                }
                                if self.pending_station_restore.is_none() && self.selected_idx >= self.state.stations.len() {
                                    if let Some(name) = self.last_station_name.as_deref() {
                                        if let Some((idx, _)) = self
                                            .state
                                            .stations
                                            .iter()
                                            .enumerate()
                                            .find(|(_, s)| s.name == name)
                                        {
                                            self.selected_idx = idx;
                                        }
                                    }
                                }
                                // Initial load: jump to last-played station
                                if was_empty && !self.initial_loaded {
                                    if let Some(idx) = self.state.current_station {
                                        self.selected_idx = idx;
                                    }
                                    self.initial_loaded = true;
                                }
                                // Shuffle/next/prev: wait until current_station actually changed
                                if let Some(from) = self.jump_from_station {
                                    if self.state.current_station != from {
                                        if let Some(idx) = self.state.current_station {
                                            self.selected_idx = idx;
                                        }
                                        self.jump_from_station = None;
                                    }
                                }
                                self.selected_idx = self.selected_idx
                                    .min(self.state.stations.len() - 1);
                                self.rebuild_filter();

                                // Auto-show NTS panel when switching to NTS 1 / NTS 2
                                if self.state.current_station != prev_station {
                                    let name = self.state.current_station
                                        .and_then(|i| self.state.stations.get(i))
                                        .map(|s| s.name.as_str());
                                    match name {
                                        Some("NTS 1") => {
                                            self.show_nts_ch1 = true;
                                            self.show_nts_ch2 = false;
                                        }
                                        Some("NTS 2") => {
                                            self.show_nts_ch2 = true;
                                            self.show_nts_ch1 = false;
                                        }
                                        _ => {
                                            // Leaving NTS 1/2: close whichever panel was open
                                            self.show_nts_ch1 = false;
                                            self.show_nts_ch2 = false;
                                        }
                                    }
                                }
                            } else {
                                self.filtered_indices.clear();
                                self.list_state.select(None);
                            }

                            if self.state.current_station.is_none()
                                && self.state.current_file.is_none()
                                && !self.state.is_playing
                            {
                                if let Some((path, pos)) = self.pending_resume_file.clone() {
                                    self.pending_resume_file = None;
                                    self.left_mode = LeftPaneMode::Files;
                                    self.sort_order = self.file_sort_order;
                                    self.set_focus_pane(FocusPane::Left);
                                    self.send_cmd(Command::PlayFilePausedAt {
                                        path,
                                        start_secs: pos.max(0.0),
                                    })
                                    .await;
                                }
                            }

                            self.save_ui_session_state();
                            let _ = save_recent_state(
                                &self.recent_path,
                                &RecentState {
                                    recent_station: self.recent_station.clone(),
                                    recent_file: self.recent_file.clone(),
                                },
                            );
                            let _ = save_file_positions(&self.file_positions_path, &self.file_positions);
                        }
                        AppMessage::IcyUpdated(title) => {
                            if let Some(ref t) = title {
                                // Don't add consecutive duplicates
                                let last_raw = self.icy_history.last().map(|e| e.raw.as_str()).unwrap_or("");
                                if last_raw != t.as_str() {
                                    let now = chrono::Local::now();
                                    let ts_str = format_timestamp(now);
                                    let display = format!("{}  {}",  ts_str, t);
                                    let station = self
                                        .state
                                        .current_station
                                        .and_then(|i| self.state.stations.get(i))
                                        .map(|s| s.name.clone());
                                    if let Some(st_name) = station.as_deref() {
                                        self.recent_station
                                            .insert(st_name.to_string(), chrono::Local::now().timestamp());
                                        let _ = save_recent_state(
                                            &self.recent_path,
                                            &RecentState {
                                                recent_station: self.recent_station.clone(),
                                                recent_file: self.recent_file.clone(),
                                            },
                                        );
                                    }
                                    let entry = TickerEntry {
                                        raw: t.clone(),
                                        display: display.clone(),
                                        station,
                                        show: None,
                                        url: None,
                                        comment: None,
                                    };
                                    self.icy_history.push(entry);
                                    if self.icy_history.len() > 200 {
                                        self.icy_history.remove(0);
                                    }
                                    // Persist to icyticker.log (with timestamp)
                                    let log_line = format!("{}\n", display);
                                    if let Some(parent) = self.icy_log_path.parent() {
                                        let _ = tokio::fs::create_dir_all(parent).await;
                                    }
                                    if let Ok(mut f) = tokio::fs::OpenOptions::new()
                                        .create(true)
                                        .append(true)
                                        .open(&self.icy_log_path)
                                        .await
                                    {
                                        let _ = f.write_all(log_line.as_bytes()).await;
                                    }
                                }
                            }
                            self.state.icy_title = title;
                            let vis = self.icy_visible_indices();
                            if self.icy_selected >= vis.len() {
                                self.icy_selected = vis.len().saturating_sub(1);
                            }
                        }
                        AppMessage::Log(msg) => {
                            self.log(msg);
                        }
                        AppMessage::NtsUpdated(ch, data) => {
                            // Update station list city for NTS 1 / NTS 2
                            // from the current show's location, so the list reflects
                            // where the broadcast is coming from right now.
                            // Only update if the new location is non-empty AND different
                            // from current - this prevents flickering when API returns
                            // empty/inconsistent values.
                            let station_name = if ch == 0 { "NTS 1" } else { "NTS 2" };
                            let loc = data.now.location_long.clone();
                            if !loc.is_empty() {
                                if let Some(s) = self.state.stations.iter_mut()
                                    .find(|s| s.name == station_name)
                                {
                                    if s.city != loc {
                                        s.city = loc;
                                    }
                                }
                            }
                            if ch == 0 {
                                self.nts_ch1 = Some(data);
                                self.nts_ch1_error = None;
                            } else {
                                self.nts_ch2 = Some(data);
                                self.nts_ch2_error = None;
                            }
                        }
                        AppMessage::NtsError(ch, msg) => {
                            if ch == 0 {
                                self.nts_ch1_error = Some(msg);
                            } else {
                                self.nts_ch2_error = Some(msg);
                            }
                        }
                    }
                }

                _ = log_refresh.tick() => {
                    if self.show_logs {
                        self.refresh_log_file();
                    }
                }

                _ = songs_refresh.tick() => {
                    self.songs_history = load_songs_csv(&self.songs_csv_path);
                    let _ = export_songs_vds(&self.songs_vds_path, &self.songs_history);
                    let vis = self.songs_visible_indices();
                    if self.songs_selected >= vis.len() {
                        self.songs_selected = vis.len().saturating_sub(1);
                    }
                }

                _ = files_refresh.tick() => {
                    self.files = load_local_files(&self.downloads_dir);
                    self.rebuild_file_index_base();
                    if self.file_selected >= self.files.len() {
                        self.file_selected = self.files.len().saturating_sub(1);
                    }
                    for _ in 0..8 {
                        self.index_one_file_metadata();
                    }
                    self.rebuild_file_filter();
                    self.refresh_selected_file_metadata();
                }

                _ = nts_refresh.tick() => {
                    let tx2 = tx.clone();
                    tokio::spawn(async move {
                        for ch_idx in 0usize..2 {
                            match fetch_nts_channel(ch_idx).await {
                                Ok(ch) => { let _ = tx2.send(AppMessage::NtsUpdated(ch_idx, ch)).await; }
                                Err(e) => { let _ = tx2.send(AppMessage::NtsError(ch_idx, e.to_string())).await; }
                            }
                        }
                    });
                }
            }
        }

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        Ok(())
    }

    async fn handle_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        const OPEN_LOG_ROWS: usize = 8;
        if key.kind == KeyEventKind::Release {
            return Ok(false);
        }
        // ── Shared navigation (works in both filter and normal mode) ─────────
        let visible_count = self.filtered_indices.len();
        let current_pos = self
            .filtered_indices
            .iter()
            .position(|&i| i == self.selected_idx);

        // ── Filter input mode ────────────────────────────────────────────────
        if self.filter_active {
            match key.code {
                // Arrow keys navigate while typing
                KeyCode::Up => {
                    match self.filter_target {
                        FocusPane::Left => {
                            if self.left_mode == LeftPaneMode::Stations {
                                if visible_count > 0 {
                                    let pos = current_pos.unwrap_or(0);
                                    let new_pos = pos.saturating_sub(1);
                                    self.selected_idx = self.filtered_indices[new_pos];
                                    self.list_state.select(Some(new_pos));
                                }
                            } else if !self.file_filtered_indices.is_empty() {
                                let pos = self
                                    .file_filtered_indices
                                    .iter()
                                    .position(|&i| i == self.file_selected)
                                    .unwrap_or(0);
                                let new_pos = pos.saturating_sub(1);
                                self.file_selected = self.file_filtered_indices[new_pos];
                                self.refresh_selected_file_metadata();
                            }
                        }
                        FocusPane::Icy => self.icy_selected = self.icy_selected.saturating_sub(1),
                        FocusPane::Songs => self.songs_selected = self.songs_selected.saturating_sub(1),
                        FocusPane::Meta => self.meta_scroll = self.meta_scroll.saturating_sub(1),
                        FocusPane::Nts => self.nts_scroll = self.nts_scroll.saturating_sub(1),
                    }
                }
                KeyCode::Down => {
                    match self.filter_target {
                        FocusPane::Left => {
                            if self.left_mode == LeftPaneMode::Stations {
                                if visible_count > 0 {
                                    let pos = current_pos.unwrap_or(0);
                                    let new_pos = (pos + 1).min(visible_count - 1);
                                    self.selected_idx = self.filtered_indices[new_pos];
                                    self.list_state.select(Some(new_pos));
                                }
                            } else if !self.file_filtered_indices.is_empty() {
                                let pos = self
                                    .file_filtered_indices
                                    .iter()
                                    .position(|&i| i == self.file_selected)
                                    .unwrap_or(0);
                                let new_pos = (pos + 1).min(self.file_filtered_indices.len() - 1);
                                self.file_selected = self.file_filtered_indices[new_pos];
                                self.refresh_selected_file_metadata();
                            }
                        }
                        FocusPane::Icy => {
                            let total = self.icy_visible_indices().len();
                            if total > 0 { self.icy_selected = (self.icy_selected + 1).min(total - 1); }
                        }
                        FocusPane::Songs => {
                            let total = self.songs_visible_indices().len();
                            if total > 0 { self.songs_selected = (self.songs_selected + 1).min(total - 1); }
                        }
                        FocusPane::Meta => self.meta_scroll = self.meta_scroll.saturating_add(1),
                        FocusPane::Nts => self.nts_scroll = self.nts_scroll.saturating_add(1),
                    }
                }
                KeyCode::Esc => {
                    self.filter.clear();
                    self.filter_active = false;
                    self.set_filter_for_pane(self.filter_target, String::new());
                    self.apply_filter_for_pane(self.filter_target);
                    self.clamp_filtered_selection();
                }
                KeyCode::Enter => {
                    // Confirm filter — close input bar, keep filter applied
                    self.filter_active = false;
                }
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.set_filter_for_pane(self.filter_target, self.filter.clone());
                    self.apply_filter_for_pane(self.filter_target);
                    self.clamp_filtered_selection();
                }
                KeyCode::Char(c) => {
                    self.filter.push(c);
                    self.set_filter_for_pane(self.filter_target, self.filter.clone());
                    self.apply_filter_for_pane(self.filter_target);
                    self.clamp_filtered_selection();
                }
                _ => {}
            }
            return Ok(false);
        }

        if self.show_logs {
            let max_scroll = self
                .log_file_lines
                .len()
                .saturating_sub(OPEN_LOG_ROWS);
            let page = OPEN_LOG_ROWS.saturating_sub(1).max(1);
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.log_scroll = (self.log_scroll + 1).min(max_scroll);
                    return Ok(false);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.log_scroll = self.log_scroll.saturating_sub(1);
                    return Ok(false);
                }
                KeyCode::PageUp => {
                    self.log_scroll = (self.log_scroll + page).min(max_scroll);
                    return Ok(false);
                }
                KeyCode::PageDown => {
                    self.log_scroll = self.log_scroll.saturating_sub(page);
                    return Ok(false);
                }
                KeyCode::Home => {
                    self.log_scroll = max_scroll;
                    return Ok(false);
                }
                KeyCode::End => {
                    self.log_scroll = 0;
                    return Ok(false);
                }
                _ => {}
            }
        }

        // ── Normal mode ──────────────────────────────────────────────────────
        match key.code {
            KeyCode::Tab => {
                let next = if self.left_mode == LeftPaneMode::Stations {
                    let nts_visible = self.show_nts_ch1 || self.show_nts_ch2;
                    match self.focus_pane {
                        FocusPane::Left => if nts_visible { FocusPane::Nts } else { FocusPane::Icy },
                        FocusPane::Nts => FocusPane::Icy,
                        FocusPane::Icy => FocusPane::Songs,
                        FocusPane::Songs => FocusPane::Left,
                        FocusPane::Meta => FocusPane::Left,
                    }
                } else {
                    match self.focus_pane {
                        FocusPane::Left => FocusPane::Meta,
                        FocusPane::Meta => FocusPane::Songs,
                        FocusPane::Songs => FocusPane::Icy,
                        FocusPane::Icy => FocusPane::Left,
                        FocusPane::Nts => FocusPane::Left,
                    }
                };
                self.set_focus_pane(next);
            }

            KeyCode::Char('`') => {
                if let Some(idx) = self.state.current_station {
                    self.left_mode = LeftPaneMode::Stations;
                    self.filter_left.clear();
                    self.filter = self.filter_left.clone();
                    self.selected_idx = idx;
                    self.rebuild_filter();
                    self.set_focus_pane(FocusPane::Left);
                } else if let Some(path) = self.state.current_file.clone() {
                    self.left_mode = LeftPaneMode::Files;
                    self.filter_left.clear();
                    self.filter = self.filter_left.clone();
                    self.rebuild_file_filter();
                    if let Some((i, _)) = self
                        .files
                        .iter()
                        .enumerate()
                        .find(|(_, f)| f.path.to_string_lossy() == path)
                    {
                        self.file_selected = i;
                        self.refresh_selected_file_metadata();
                    }
                    self.set_focus_pane(FocusPane::Left);
                }
            }

            // ── Quit ────────────────────────────────────────────────────────
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                return Ok(true);
            }

            // ── Filter ──────────────────────────────────────────────────────
            KeyCode::Char('/') => {
                self.filter_target = self.focus_pane;
                self.clear_filters_except(self.filter_target);
                self.filter.clear();
                self.set_filter_for_pane(self.filter_target, String::new());
                self.filter_active = true;
            }

            // ── Sort ─────────────────────────────────────────────────────────
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.sort_order = if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.sort_order.prev_for_mode(self.left_mode)
                } else {
                    self.sort_order.next_for_mode(self.left_mode)
                };
                match self.left_mode {
                    LeftPaneMode::Stations => self.station_sort_order = self.sort_order,
                    LeftPaneMode::Files => self.file_sort_order = self.sort_order,
                }
                if self.left_mode == LeftPaneMode::Stations {
                    self.rebuild_filter();
                } else {
                    self.rebuild_file_filter();
                }
            }

            // ── Navigation ──────────────────────────────────────────────────
            KeyCode::Up | KeyCode::Char('k') => {
                let step = if key.modifiers.contains(KeyModifiers::SHIFT) { 5 } else { 1 };
                match self.focus_pane {
                    FocusPane::Left => {
                        if self.left_mode == LeftPaneMode::Stations {
                            if visible_count > 0 {
                                let pos = current_pos.unwrap_or(0);
                                let new_pos = pos.saturating_sub(step);
                                self.selected_idx = self.filtered_indices[new_pos];
                                self.list_state.select(Some(new_pos));
                                self.update_nts_panel_for_selection();
                            }
                        } else if !self.file_filtered_indices.is_empty() {
                            let pos = self
                                .file_filtered_indices
                                .iter()
                                .position(|&i| i == self.file_selected)
                                .unwrap_or(0);
                            let new_pos = pos.saturating_sub(step);
                            self.file_selected = self.file_filtered_indices[new_pos];
                            self.refresh_selected_file_metadata();
                        }
                    }
                    FocusPane::Songs => {
                        let total = self.songs_visible_indices().len();
                        if total > 0 {
                            self.songs_selected = self.songs_selected.saturating_sub(step).min(total - 1);
                        } else {
                            self.songs_selected = 0;
                        }
                    }
                    FocusPane::Icy => {
                        let total = self.icy_visible_indices().len();
                        if total > 0 {
                            self.icy_selected = self.icy_selected.saturating_sub(step).min(total - 1);
                        } else {
                            self.icy_selected = 0;
                        }
                    }
                    FocusPane::Meta => {
                        self.meta_scroll = self.meta_scroll.saturating_sub(step);
                    }
                    FocusPane::Nts => {
                        self.nts_scroll = self.nts_scroll.saturating_sub(step);
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let step = if key.modifiers.contains(KeyModifiers::SHIFT) { 5 } else { 1 };
                match self.focus_pane {
                    FocusPane::Left => {
                        if self.left_mode == LeftPaneMode::Stations {
                            if visible_count > 0 {
                                let pos = current_pos.unwrap_or(0);
                                let new_pos = (pos + step).min(visible_count - 1);
                                self.selected_idx = self.filtered_indices[new_pos];
                                self.list_state.select(Some(new_pos));
                                self.update_nts_panel_for_selection();
                            }
                        } else if !self.file_filtered_indices.is_empty() {
                            let pos = self
                                .file_filtered_indices
                                .iter()
                                .position(|&i| i == self.file_selected)
                                .unwrap_or(0);
                            let new_pos = (pos + step).min(self.file_filtered_indices.len() - 1);
                            self.file_selected = self.file_filtered_indices[new_pos];
                            self.refresh_selected_file_metadata();
                        }
                    }
                    FocusPane::Songs => {
                        let total = self.songs_visible_indices().len();
                        if total > 0 {
                            self.songs_selected = (self.songs_selected + step).min(total - 1);
                        }
                    }
                    FocusPane::Icy => {
                        let total = self.icy_visible_indices().len();
                        if total > 0 {
                            self.icy_selected = (self.icy_selected + step).min(total - 1);
                        }
                    }
                    FocusPane::Meta => {
                        self.meta_scroll = self.meta_scroll.saturating_add(step);
                    }
                    FocusPane::Nts => {
                        self.nts_scroll = self.nts_scroll.saturating_add(step);
                    }
                }
            }
            KeyCode::PageUp => {
                if visible_count > 0 {
                    let pos = current_pos.unwrap_or(0);
                    let new_pos = pos.saturating_sub(10);
                    self.selected_idx = self.filtered_indices[new_pos];
                    self.list_state.select(Some(new_pos));
                    self.update_nts_panel_for_selection();
                }
            }
            KeyCode::PageDown => {
                if visible_count > 0 {
                    let pos = current_pos.unwrap_or(0);
                    let new_pos = (pos + 10).min(visible_count - 1);
                    self.selected_idx = self.filtered_indices[new_pos];
                    self.list_state.select(Some(new_pos));
                    self.update_nts_panel_for_selection();
                }
            }
            KeyCode::Home | KeyCode::Char('g') => {
                if visible_count > 0 {
                    self.selected_idx = self.filtered_indices[0];
                    self.list_state.select(Some(0));
                    self.update_nts_panel_for_selection();
                }
            }
            KeyCode::End | KeyCode::Char('G') => {
                if visible_count > 0 {
                    let last = visible_count - 1;
                    self.selected_idx = self.filtered_indices[last];
                    self.list_state.select(Some(last));
                    self.update_nts_panel_for_selection();
                }
            }

            // ── Playback ─────────────────────────────────────────────────
            // Enter: play selected item, or stop it if already playing.
            // Space: pause/play toggle (or start playing if nothing is loaded).
            KeyCode::Enter => {
                if self.focus_pane == FocusPane::Left {
                    if self.left_mode == LeftPaneMode::Stations {
                        let is_current = self.state.current_station == Some(self.selected_idx);
                        if is_current {
                            self.send_cmd(Command::Stop).await;
                        } else {
                            self.send_cmd(Command::Play {
                                station_idx: self.selected_idx,
                            })
                            .await;
                        }
                    } else if let Some(file) = self.files.get(self.file_selected) {
                        let path = file.path.to_string_lossy().to_string();
                        let is_current = self.state.current_file.as_deref() == Some(path.as_str());
                        if is_current {
                            self.send_cmd(Command::Stop).await;
                        } else {
                            let start_secs = self.file_positions.get(&path).copied().unwrap_or(0.0).max(0.0);
                            self.send_cmd(Command::PlayFileAt { path, start_secs }).await;
                        }
                    }
                }
                // Enter on non-Left panes: no-op (use Space to toggle pause)
            }
            KeyCode::Char(' ') => {
                if self.state.current_station.is_some() || self.state.current_file.is_some() {
                    self.send_cmd(Command::TogglePause).await;
                } else if self.focus_pane == FocusPane::Left {
                    if self.left_mode == LeftPaneMode::Stations {
                        self.send_cmd(Command::Play {
                            station_idx: self.selected_idx,
                        })
                        .await;
                    } else if let Some(file) = self.files.get(self.file_selected) {
                        let path = file.path.to_string_lossy().to_string();
                        let start_secs = self.file_positions.get(&path).copied().unwrap_or(0.0).max(0.0);
                        self.send_cmd(Command::PlayFileAt { path, start_secs }).await;
                    }
                }
            }
            KeyCode::Esc => {
                if self.show_help {
                    self.show_help = false;
                } else if self.filter_active {
                    self.filter_active = false;
                } else if !self.filter_for_pane(self.focus_pane).is_empty() {
                    self.set_filter_for_pane(self.focus_pane, String::new());
                    if self.filter_target == self.focus_pane {
                        self.filter.clear();
                    }
                    self.apply_filter_for_pane(self.focus_pane);
                } else if self.focus_pane != FocusPane::Left {
                    self.set_focus_pane(FocusPane::Left);
                } else {
                    // No-op: Esc should not stop playback.
                }
            }
            KeyCode::Char('n') => {
                if self.left_mode != LeftPaneMode::Stations {
                    return Ok(false);
                }
                self.jump_from_station = Some(self.state.current_station);
                self.send_cmd(Command::Next).await;
            }
            KeyCode::Char('p') => {
                if self.left_mode != LeftPaneMode::Stations {
                    return Ok(false);
                }
                self.jump_from_station = Some(self.state.current_station);
                self.send_cmd(Command::Prev).await;
            }
            KeyCode::Char('r') => {
                if self.left_mode == LeftPaneMode::Stations {
                    self.jump_from_station = Some(self.state.current_station);
                    self.send_cmd(Command::Random).await;
                } else if !self.file_filtered_indices.is_empty() {
                    if let Some(cur_path) = self.state.current_file.clone() {
                        let cur_pos = self.state.time_pos_secs.unwrap_or(0.0).max(0.0);
                        self.push_random_history_entry(cur_path, cur_pos);
                    }
                    let pos = self.pick_random_file_pos(self.file_filtered_indices.len());
                    if let Some(&idx) = self.file_filtered_indices.get(pos) {
                        self.file_selected = idx;
                        self.refresh_selected_file_metadata();
                        if let Some(file) = self.files.get(idx) {
                            let key = file.path.to_string_lossy().to_string();
                            if !self.file_metadata_cache.contains_key(&key) {
                                self.ensure_file_metadata_cached(idx);
                            }
                            let duration = self
                                .file_metadata_cache
                                .get(&key)
                                .and_then(|m| m.duration_secs)
                                .unwrap_or(0.0);
                            let start_secs = self.pick_random_start_secs(duration);
                            self.send_cmd(Command::PlayFileAt {
                                path: key,
                                start_secs,
                            })
                            .await;
                        }
                    }
                }
            }
            KeyCode::Char('R') => {
                if let Some(prev) = self.pop_random_history_entry() {
                    self.left_mode = LeftPaneMode::Files;
                    self.rebuild_file_filter();
                    if let Some((i, _)) = self
                        .files
                        .iter()
                        .enumerate()
                        .find(|(_, f)| f.path.to_string_lossy() == prev.path)
                    {
                        self.file_selected = i;
                        self.refresh_selected_file_metadata();
                    }
                    self.send_cmd(Command::PlayFileAt {
                        path: prev.path,
                        start_secs: prev.start_secs.max(0.0),
                    })
                    .await;
                    self.set_focus_pane(FocusPane::Left);
                }
            }

            KeyCode::Char('f') | KeyCode::Char('F') => {
                self.left_mode = match self.left_mode {
                    LeftPaneMode::Stations => LeftPaneMode::Files,
                    LeftPaneMode::Files => LeftPaneMode::Stations,
                };
                self.sort_order = if self.left_mode == LeftPaneMode::Stations {
                    self.station_sort_order
                } else {
                    self.file_sort_order
                };
                self.set_focus_pane(FocusPane::Left);
                self.filter_active = false;
                if self.left_mode == LeftPaneMode::Files {
                    self.rebuild_file_filter();
                    self.refresh_selected_file_metadata();
                } else {
                    self.rebuild_filter();
                }
            }

            KeyCode::Char('_') | KeyCode::Char('-') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                if self.left_mode == LeftPaneMode::Files {
                    self.files_left_full_width = !self.files_left_full_width;
                } else if self.focus_pane != FocusPane::Left {
                    self.files_right_maximized = !self.files_right_maximized;
                }
            }
            KeyCode::Char('_') => {
                if self.left_mode == LeftPaneMode::Files {
                    self.files_left_full_width = !self.files_left_full_width;
                } else if self.focus_pane != FocusPane::Left {
                    self.files_right_maximized = !self.files_right_maximized;
                }
            }

            KeyCode::Char('d') => {
                #[cfg(not(windows))]
                {
                    // NTS downloads only on Unix systems (requires nts_get script)
                    if self.focus_pane == FocusPane::Songs {
                        if let Some(entry) = self.selected_song_entry() {
                            if let Some(url) = resolve_nts_download_url(&entry) {
                                let _ = std::process::Command::new("nts_get").arg(&url).spawn();
                                self.log(format!("download started: {}", url));
                            } else {
                                self.log("download skipped: no NTS tag/url in selected entry".to_string());
                            }
                        }
                    }
                }
                #[cfg(windows)]
                {
                    // On Windows, 'd' key shows info about current song
                    if let Some(icy) = &self.state.icy_title {
                        self.log(format!("Current: {}", icy));
                    } else {
                        self.log("No ICY metadata available".to_string());
                    }
                }
            }

            KeyCode::Char('r') => {
                // Recognize/save current song to database
                if let Some(icy) = &self.state.icy_title {
                    let station = self.current_station_name();
                    
                    // Try to recognize from ICY
                    if let Some(entry) = SongEntry::from_icy(icy, &station) {
                        let display = entry.display_title();
                        let display_clone = display.clone();
                        // Add to database
                        let db = SongDatabase::new(SongDatabase::default_path());
                        tokio::spawn(async move {
                            if let Err(e) = db.init().await {
                                eprintln!("Failed to init song db: {}", e);
                                return;
                            }
                            if let Err(e) = db.add_song(&entry).await {
                                eprintln!("Failed to add song: {}", e);
                            } else {
                                println!("✓ Saved: {}", display_clone);
                            }
                        });
                        self.log(format!("✓ Saved: {}", display));
                    } else {
                        // Save raw ICY if parsing failed
                        let entry = SongEntry::new(icy.clone(), "Unknown".to_string(), station);
                        let db = SongDatabase::new(SongDatabase::default_path());
                        tokio::spawn(async move {
                            let _ = db.init().await;
                            let _ = db.add_song(&entry).await;
                        });
                        self.log(format!("✓ Saved (raw): {}", icy));
                    }
                } else {
                    self.log("No song playing to recognize".to_string());
                }
            }

            KeyCode::Char('*') => {
                if self.focus_pane == FocusPane::Left {
                    if self.left_mode == LeftPaneMode::Stations {
                        if let Some(st) = self.state.stations.get(self.selected_idx) {
                            let cur = self.station_stars.get(&st.name).copied().unwrap_or(0);
                            let next = (cur + 1) % 4;
                            let name = st.name.clone();
                            self.set_station_star(&name, next);
                            self.rebuild_filter();
                        }
                    } else if let Some(file) = self.files.get(self.file_selected) {
                        let key = file.path.to_string_lossy().to_string();
                        let cur = self.file_stars.get(&key).copied().unwrap_or(0);
                        let next = (cur + 1) % 4;
                        self.set_file_star(&key, next);
                        self.rebuild_file_filter();
                    }
                }
            }

            KeyCode::Char('y') => {
                let text = match self.focus_pane {
                    FocusPane::Icy => self.selected_icy_text(),
                    FocusPane::Songs => self.selected_song_text(),
                    FocusPane::Meta => self.selected_meta_text(),
                    FocusPane::Left => None,
                    FocusPane::Nts => None,
                };
                if let Some(text) = text {
                    let _ = copy_to_clipboard(&text);
                    self.log("copied entry".to_string());
                }
            }

            // ── Volume ────────────────────────────────────────────────────
            KeyCode::Left | KeyCode::Char('-') => {
                let new_vol = (self.state.volume - 0.05).max(0.0);
                if new_vol > 0.0 {
                    self.last_nonzero_volume = new_vol;
                }
                self.send_cmd(Command::Volume { value: new_vol }).await;
            }
            KeyCode::Right | KeyCode::Char('+') | KeyCode::Char('=') => {
                let new_vol = (self.state.volume + 0.05).min(1.0);
                if new_vol > 0.0 {
                    self.last_nonzero_volume = new_vol;
                }
                self.send_cmd(Command::Volume { value: new_vol }).await;
            }
            KeyCode::Char('m') | KeyCode::Char('M') => {
                if self.state.volume > 0.001 {
                    self.last_nonzero_volume = self.state.volume;
                    self.send_cmd(Command::Volume { value: 0.0 }).await;
                } else {
                    let restore = if self.last_nonzero_volume > 0.001 {
                        self.last_nonzero_volume
                    } else {
                        0.7
                    };
                    self.send_cmd(Command::Volume { value: restore }).await;
                }
            }

            KeyCode::Char(',') | KeyCode::Char('.') | KeyCode::Char('<') | KeyCode::Char('>') => {
                if self.state.current_file.is_some() {
                    let dur = self.state.duration_secs.unwrap_or(0.0).max(0.0);
                    let shifted_ten_min = key.modifiers.contains(KeyModifiers::SHIFT)
                        || matches!(key.code, KeyCode::Char('<') | KeyCode::Char('>'));
                    let base = if shifted_ten_min {
                        let _ = dur;
                        600.0
                    } else if key.kind == KeyEventKind::Repeat {
                        // Comfortable long-press acceleration
                        if dur > 0.0 {
                            (dur / 80.0).clamp(10.0, 45.0)
                        } else {
                            20.0
                        }
                    } else {
                        // Fine seek
                        10.0
                    };
                    let seconds = if matches!(key.code, KeyCode::Char(',') | KeyCode::Char('<')) {
                        -base
                    } else {
                        base
                    };
                    self.send_cmd(Command::SeekRelative { seconds }).await;
                }
            }

            KeyCode::Char('0') => {
                if self.state.current_file.is_some() {
                    self.send_cmd(Command::SeekTo { seconds: 0.0 }).await;
                }
            }

            // ── View toggles ──────────────────────────────────────────────
            KeyCode::Char('l') | KeyCode::Char('L') => {
                self.show_logs = !self.show_logs;
                if self.show_logs {
                    self.refresh_log_file();
                    self.log_scroll = 0;
                }
            }
            // Direct pane focus: 1=left, 2/3/4 right vertical panes
            KeyCode::Char('1') => {
                self.set_focus_pane(FocusPane::Left);
            }
            KeyCode::Char('2') => {
                let upper = if self.left_mode == LeftPaneMode::Stations {
                    FocusPane::Icy
                } else {
                    FocusPane::Meta
                };
                self.set_focus_pane(upper);
            }
            KeyCode::Char('3') => {
                self.set_focus_pane(FocusPane::Songs);
            }
            KeyCode::Char('4') => {
                if self.left_mode == LeftPaneMode::Stations {
                    if self.show_nts_ch1 || self.show_nts_ch2 {
                        self.set_focus_pane(FocusPane::Nts);
                    }
                } else {
                    self.set_focus_pane(FocusPane::Icy);
                }
            }

            // NTS panel toggles moved to Shift+1 / Shift+2
            KeyCode::Char('!') => {
                self.show_nts_ch1 = !self.show_nts_ch1;
                if self.show_nts_ch1 {
                    self.show_nts_ch2 = false;
                }
            }
            KeyCode::Char('@') => {
                self.show_nts_ch2 = !self.show_nts_ch2;
                if self.show_nts_ch2 {
                    self.show_nts_ch1 = false;
                }
            }
            KeyCode::Char('?') => {
                self.show_help = !self.show_help;
            }
            KeyCode::Char('h') | KeyCode::Char('H') => {
                self.show_keys = !self.show_keys;
            }

            _ => {}
        }

        Ok(false)
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        let col = mouse.column;
        let row = mouse.row;
        let in_rect = |r: Rect| -> bool {
            col >= r.x
                && col < r.x.saturating_add(r.width)
                && row >= r.y
                && row < r.y.saturating_add(r.height)
        };

        let hovered = if in_rect(self.left_pane_rect) {
            Some(FocusPane::Left)
        } else if in_rect(self.upper_right_rect) {
            Some(if self.left_mode == LeftPaneMode::Stations { FocusPane::Icy } else { FocusPane::Meta })
        } else if in_rect(self.middle_right_rect) {
            Some(FocusPane::Songs)
        } else if in_rect(self.lower_right_rect) {
            Some(if self.left_mode == LeftPaneMode::Stations { FocusPane::Songs } else { FocusPane::Icy })
        } else {
            None
        };

        let pane = hovered.unwrap_or(self.focus_pane);

        if let Some(p) = hovered {
            self.set_focus_pane(p);
        }

        if self.show_help {
            return;
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                match pane {
                    FocusPane::Left => {
                        if self.left_mode == LeftPaneMode::Stations {
                            if let Some(pos) = self
                                .filtered_indices
                                .iter()
                                .position(|&i| i == self.selected_idx)
                            {
                                let new_pos = pos.saturating_sub(1);
                                if let Some(&idx) = self.filtered_indices.get(new_pos) {
                                    self.selected_idx = idx;
                                    self.list_state.select(Some(new_pos));
                                }
                            }
                        } else if let Some(pos) = self
                            .file_filtered_indices
                            .iter()
                            .position(|&i| i == self.file_selected)
                        {
                            let new_pos = pos.saturating_sub(1);
                            if let Some(&idx) = self.file_filtered_indices.get(new_pos) {
                                self.file_selected = idx;
                                self.refresh_selected_file_metadata();
                            }
                        }
                    }
                    FocusPane::Nts => {
                        self.nts_scroll = self.nts_scroll.saturating_sub(1);
                    }
                    FocusPane::Icy => {
                        self.icy_selected = self.icy_selected.saturating_sub(1);
                    }
                    FocusPane::Songs => {
                        self.songs_selected = self.songs_selected.saturating_sub(1);
                    }
                    FocusPane::Meta => {
                        self.meta_scroll = self.meta_scroll.saturating_sub(2);
                    }
                }
            }
            MouseEventKind::ScrollDown => {
                match pane {
                    FocusPane::Left => {
                        if self.left_mode == LeftPaneMode::Stations {
                            if let Some(pos) = self
                                .filtered_indices
                                .iter()
                                .position(|&i| i == self.selected_idx)
                            {
                                let new_pos = (pos + 1).min(self.filtered_indices.len().saturating_sub(1));
                                if let Some(&idx) = self.filtered_indices.get(new_pos) {
                                    self.selected_idx = idx;
                                    self.list_state.select(Some(new_pos));
                                }
                            }
                        } else if let Some(pos) = self
                            .file_filtered_indices
                            .iter()
                            .position(|&i| i == self.file_selected)
                        {
                            let new_pos = (pos + 1).min(self.file_filtered_indices.len().saturating_sub(1));
                            if let Some(&idx) = self.file_filtered_indices.get(new_pos) {
                                self.file_selected = idx;
                                self.refresh_selected_file_metadata();
                            }
                        }
                    }
                    FocusPane::Nts => {
                        self.nts_scroll = self.nts_scroll.saturating_add(1);
                    }
                    FocusPane::Icy => {
                        let total = self.icy_visible_indices().len();
                        if total > 0 {
                            self.icy_selected = (self.icy_selected + 1).min(total.saturating_sub(1));
                        }
                    }
                    FocusPane::Songs => {
                        let total = self.songs_visible_indices().len();
                        if total > 0 {
                            self.songs_selected = (self.songs_selected + 1).min(total.saturating_sub(1));
                        }
                    }
                    FocusPane::Meta => {
                        self.meta_scroll = self.meta_scroll.saturating_add(2);
                    }
                }
            }
            MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                match pane {
                    FocusPane::Left => {
                        if self.left_mode == LeftPaneMode::Stations {
                            let rel = row.saturating_sub(self.left_pane_rect.y) as usize;
                            let idx_pos = self.station_view_start.saturating_add(rel);
                            if let Some(&idx) = self.filtered_indices.get(idx_pos) {
                                self.selected_idx = idx;
                                self.list_state.select(Some(idx_pos));
                                self.update_nts_panel_for_selection();
                            }
                        } else {
                            let rel = row.saturating_sub(self.left_pane_rect.y) as usize;
                            let idx_pos = self.file_view_start.saturating_add(rel);
                            if let Some(&idx) = self.file_filtered_indices.get(idx_pos) {
                                self.file_selected = idx;
                                self.refresh_selected_file_metadata();
                            }
                        }
                    }
                    FocusPane::Nts => {}
                    FocusPane::Icy => {
                        let icy_rect = if self.left_mode == LeftPaneMode::Stations {
                            self.upper_right_rect
                        } else {
                            self.lower_right_rect
                        };
                        let rel = row.saturating_sub(icy_rect.y) as usize;
                        let total = self.icy_visible_indices().len();
                        if total > 0 {
                            self.icy_selected = self
                                .icy_view_start
                                .saturating_add(rel)
                                .min(total.saturating_sub(1));
                        }
                    }
                    FocusPane::Songs => {
                        let songs_rect = if self.left_mode == LeftPaneMode::Stations {
                            self.lower_right_rect
                        } else {
                            self.middle_right_rect
                        };
                        let rel = row.saturating_sub(songs_rect.y) as usize;
                        let total = self.songs_visible_indices().len();
                        if total > 0 {
                            self.songs_selected = self
                                .songs_view_start
                                .saturating_add(rel)
                                .min(total.saturating_sub(1));
                        }
                    }
                    FocusPane::Meta => {}
                }
            }
            _ => {}
        }
    }

    async fn send_cmd(&self, cmd: Command) {
        if let Some(ref tx) = self.cmd_tx {
            let _ = tx.send(cmd).await;
        }
    }

    fn refresh_log_file(&mut self) {
        if let Ok(content) = std::fs::read_to_string(&self.log_path) {
            self.log_file_lines = content
                .lines()
                .rev()
                .take(500)
                .map(|s| s.to_string())
                .collect();
            let max_scroll = self.log_file_lines.len().saturating_sub(8);
            self.log_scroll = self.log_scroll.min(max_scroll);
        }
    }

    fn log(&mut self, msg: String) {
        self.logs.push(msg);
        if self.logs.len() > 200 {
            self.logs.remove(0);
        }
    }

    fn refresh_selected_file_metadata(&mut self) {
        let Some(file) = self.files.get(self.file_selected) else {
            self.selected_file_metadata = None;
            return;
        };
        let key = file.path.to_string_lossy().to_string();
        if let Some(meta) = self.file_metadata_cache.get(&key) {
            self.selected_file_metadata = Some(meta.clone());
            self.index_one_file_metadata();
            return;
        }
        let meta = probe_file_metadata(&file.path).unwrap_or_default();
        self.file_metadata_cache.insert(key, meta.clone());
        self.selected_file_metadata = Some(meta);
        self.index_one_file_metadata();
    }

    fn selected_icy_text(&self) -> Option<String> {
        let visible = self.icy_visible_indices();
        let idx = *visible.get(self.icy_selected)?;
        let e = self.icy_history.get(idx)?;
        let mut s = e.display.clone();
        if let Some(st) = e.station.as_deref() {
            s.push_str("  ");
            s.push_str(st);
        }
        Some(s)
    }

    fn selected_song_text(&self) -> Option<String> {
        let visible = self.songs_visible_indices();
        let idx = *visible.get(self.songs_selected)?;
        let e = self.songs_history.get(idx)?;
        let mut s = e.display.clone();
        if let Some(st) = e.station.as_deref() {
            s.push_str("  ");
            s.push_str(st);
            if let Some(show) = e.show.as_deref() {
                s.push_str(" · ");
                s.push_str(show);
            }
        }
        Some(s)
    }

    fn selected_song_entry(&self) -> Option<TickerEntry> {
        let visible = self.songs_visible_indices();
        let idx = *visible.get(self.songs_selected)?;
        self.songs_history.get(idx).cloned()
    }

    fn selected_meta_text(&self) -> Option<String> {
        let file = self.files.get(self.file_selected)?;
        let mut out = String::new();
        out.push_str(&file.name);
        out.push('\n');
        out.push_str(&file.path.to_string_lossy());
        if let Some(meta) = self.selected_file_metadata.as_ref() {
            if let Some(v) = meta.title.as_deref() { out.push_str(&format!("\ntitle: {}", v)); }
            if let Some(v) = meta.artist.as_deref() { out.push_str(&format!("\nartist: {}", v)); }
            if let Some(v) = meta.album.as_deref() { out.push_str(&format!("\nalbum: {}", v)); }
            if let Some(v) = meta.date.as_deref() { out.push_str(&format!("\ndate: {}", v)); }
            if let Some(v) = meta.genre.as_deref() { out.push_str(&format!("\ngenre: {}", v)); }
            if let Some(v) = meta.description.as_deref() { out.push_str(&format!("\ndescription: {}", v)); }
            if !meta.tracklist.is_empty() {
                out.push_str("\ntracklist:");
                for item in meta.tracklist.iter().take(120) {
                    out.push_str("\n- ");
                    out.push_str(item);
                }
            }
        }
        Some(out)
    }

    fn ensure_file_metadata_cached(&mut self, idx: usize) {
        let Some(file) = self.files.get(idx) else {
            return;
        };
        let key = file.path.to_string_lossy().to_string();
        if self.file_metadata_cache.contains_key(&key) {
            return;
        }
        if let Some(meta) = probe_file_metadata(&file.path) {
            self.file_metadata_cache.insert(key, meta);
        }
    }

    fn rebuild_file_filter(&mut self) {
        let q = self.filter_left.clone();
        let qn = normalize_search_text(&q);
        let mut indices = Vec::new();
        for i in 0..self.files.len() {
            if q.trim().is_empty() {
                indices.push(i);
                continue;
            }
            let key = self.files[i].path.to_string_lossy().to_string();
            let hit = self.file_search_index.get(&key).map(|ix| {
                if qn.is_empty() {
                    true
                } else {
                    qn.split_whitespace().all(|term| ix.contains(term))
                }
            }).unwrap_or(false);
            if hit {
                indices.push(i);
            }
        }

        match self.file_sort_order {
            SortOrder::Default | SortOrder::Added | SortOrder::Network | SortOrder::Location => {
                indices.sort_by(|&a, &b| {
                    self.files[b]
                        .modified
                        .cmp(&self.files[a].modified)
                        .then_with(|| self.files[a].name.to_lowercase().cmp(&self.files[b].name.to_lowercase()))
                });
            }
            SortOrder::Name => {
                indices.sort_by(|&a, &b| {
                    self.files[a]
                        .name
                        .to_lowercase()
                        .cmp(&self.files[b].name.to_lowercase())
                });
            }
            SortOrder::Stars => {
                indices.sort_by(|&a, &b| {
                    let pa = self.files[a].path.to_string_lossy().to_string();
                    let pb = self.files[b].path.to_string_lossy().to_string();
                    let sa = self.file_stars.get(&pa).copied().unwrap_or(0);
                    let sb = self.file_stars.get(&pb).copied().unwrap_or(0);
                    sb.cmp(&sa)
                        .then_with(|| self.files[b].modified.cmp(&self.files[a].modified))
                        .then_with(|| self.files[a].name.to_lowercase().cmp(&self.files[b].name.to_lowercase()))
                });
            }
            SortOrder::Recent => {
                indices.sort_by(|&a, &b| {
                    let pa = self.files[a].path.to_string_lossy().to_string();
                    let pb = self.files[b].path.to_string_lossy().to_string();
                    let ra = self.recent_file.get(&pa).copied().unwrap_or(0);
                    let rb = self.recent_file.get(&pb).copied().unwrap_or(0);
                    rb.cmp(&ra)
                        .then_with(|| self.files[b].modified.cmp(&self.files[a].modified))
                });
            }
            SortOrder::StarsRecent => {
                indices.sort_by(|&a, &b| {
                    let pa = self.files[a].path.to_string_lossy().to_string();
                    let pb = self.files[b].path.to_string_lossy().to_string();
                    let sa = self.file_stars.get(&pa).copied().unwrap_or(0);
                    let sb = self.file_stars.get(&pb).copied().unwrap_or(0);
                    let ra = self.recent_file.get(&pa).copied().unwrap_or(0);
                    let rb = self.recent_file.get(&pb).copied().unwrap_or(0);
                    sb.cmp(&sa).then(rb.cmp(&ra))
                });
            }
            SortOrder::RecentStars => {
                indices.sort_by(|&a, &b| {
                    let pa = self.files[a].path.to_string_lossy().to_string();
                    let pb = self.files[b].path.to_string_lossy().to_string();
                    let sa = self.file_stars.get(&pa).copied().unwrap_or(0);
                    let sb = self.file_stars.get(&pb).copied().unwrap_or(0);
                    let ra = self.recent_file.get(&pa).copied().unwrap_or(0);
                    let rb = self.recent_file.get(&pb).copied().unwrap_or(0);
                    rb.cmp(&ra).then(sb.cmp(&sa))
                });
            }
        }
        self.file_filtered_indices = indices;
        if self.file_filtered_indices.is_empty() {
            self.file_selected = 0;
            self.selected_file_metadata = None;
        } else if !self.file_filtered_indices.contains(&self.file_selected) {
            self.file_selected = self.file_filtered_indices[0];
            self.refresh_selected_file_metadata();
        }
    }

    /// Recompute `filtered_indices` from the current filter string and station list.
    pub fn rebuild_filter(&mut self) {
        let q = self.filter_left.clone();
        let stations = &self.state.stations;

        let mut indices: Vec<usize> = stations
            .iter()
            .enumerate()
            .filter(|(_, s)| station_matches(&q, s))
            .map(|(i, _)| i)
            .collect();

        match self.station_sort_order {
            SortOrder::Default | SortOrder::Added => {}  // preserve original order
            SortOrder::Network => {
                indices.sort_by(|&a, &b| {
                    let sa = &stations[a];
                    let sb = &stations[b];
                    sa.network.to_lowercase()
                        .cmp(&sb.network.to_lowercase())
                        .then(sa.name.to_lowercase().cmp(&sb.name.to_lowercase()))
                });
            }
            SortOrder::Location => {
                indices.sort_by(|&a, &b| {
                    let sa = &stations[a];
                    let sb = &stations[b];
                    sa.country.to_lowercase()
                        .cmp(&sb.country.to_lowercase())
                        .then(sa.city.to_lowercase().cmp(&sb.city.to_lowercase()))
                        .then(sa.name.to_lowercase().cmp(&sb.name.to_lowercase()))
                });
            }
            SortOrder::Name => {
                indices.sort_by(|&a, &b| {
                    stations[a].name.to_lowercase()
                        .cmp(&stations[b].name.to_lowercase())
                });
            }
            SortOrder::Stars => {
                indices.sort_by(|&a, &b| {
                    let sa = self
                        .station_stars
                        .get(&stations[a].name)
                        .copied()
                        .unwrap_or(0);
                    let sb = self
                        .station_stars
                        .get(&stations[b].name)
                        .copied()
                        .unwrap_or(0);
                    sb.cmp(&sa)
                        .then(stations[a].name.to_lowercase().cmp(&stations[b].name.to_lowercase()))
                });
            }
            SortOrder::Recent => {
                indices.sort_by(|&a, &b| {
                    let ra = self
                        .recent_station
                        .get(&stations[a].name)
                        .copied()
                        .unwrap_or(0);
                    let rb = self
                        .recent_station
                        .get(&stations[b].name)
                        .copied()
                        .unwrap_or(0);
                    rb.cmp(&ra)
                        .then(stations[a].name.to_lowercase().cmp(&stations[b].name.to_lowercase()))
                });
            }
            SortOrder::StarsRecent => {
                indices.sort_by(|&a, &b| {
                    let sa = self
                        .station_stars
                        .get(&stations[a].name)
                        .copied()
                        .unwrap_or(0);
                    let sb = self
                        .station_stars
                        .get(&stations[b].name)
                        .copied()
                        .unwrap_or(0);
                    let ra = self
                        .recent_station
                        .get(&stations[a].name)
                        .copied()
                        .unwrap_or(0);
                    let rb = self
                        .recent_station
                        .get(&stations[b].name)
                        .copied()
                        .unwrap_or(0);
                    sb.cmp(&sa).then(rb.cmp(&ra))
                });
            }
            SortOrder::RecentStars => {
                indices.sort_by(|&a, &b| {
                    let sa = self
                        .station_stars
                        .get(&stations[a].name)
                        .copied()
                        .unwrap_or(0);
                    let sb = self
                        .station_stars
                        .get(&stations[b].name)
                        .copied()
                        .unwrap_or(0);
                    let ra = self
                        .recent_station
                        .get(&stations[a].name)
                        .copied()
                        .unwrap_or(0);
                    let rb = self
                        .recent_station
                        .get(&stations[b].name)
                        .copied()
                        .unwrap_or(0);
                    rb.cmp(&ra).then(sb.cmp(&sa))
                });
            }
        }

        self.filtered_indices = indices;

        // Keep selected_idx within bounds of filtered list
        if self.filtered_indices.is_empty() {
            self.list_state.select(None);
        } else {
            let pos = self
                .filtered_indices
                .iter()
                .position(|&i| i == self.selected_idx)
                .unwrap_or(0);
            self.list_state.select(Some(pos));
        }
    }
}

// ── Filter helpers ────────────────────────────────────────────────────────────

use radio_tui::shared::protocol::Station;

/// Returns true if `query` matches any searchable field of the station.
pub fn station_matches(query: &str, s: &Station) -> bool {
    if query.is_empty() {
        return true;
    }
    search_matches(query, &s.name)
        || search_matches(query, &s.network)
        || search_matches(query, &s.description)
        || search_matches(query, &s.city)
        || search_matches(query, &s.country)
        || s.tags.iter().any(|t| search_matches(query, t))
}

pub fn normalize_search_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        for lc in c.to_lowercase() {
            let mapped: Option<&str> = match lc {
                'á' | 'à' | 'ä' | 'â' | 'ã' | 'å' | 'ā' => Some("a"),
                'ç' | 'ć' | 'č' => Some("c"),
                'é' | 'è' | 'ë' | 'ê' | 'ē' => Some("e"),
                'í' | 'ì' | 'ï' | 'î' | 'ī' => Some("i"),
                'ñ' => Some("n"),
                'ó' | 'ò' | 'ö' | 'ô' | 'õ' | 'ō' => Some("o"),
                'ú' | 'ù' | 'ü' | 'û' | 'ū' => Some("u"),
                'ý' | 'ÿ' => Some("y"),
                'ß' => Some("ss"),
                _ => None,
            };
            if let Some(m) = mapped {
                out.push_str(m);
                continue;
            }
            if lc.is_ascii_alphanumeric() {
                out.push(lc);
            } else if lc.is_whitespace() {
                out.push(' ');
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn search_matches(query: &str, text: &str) -> bool {
    let q = normalize_search_text(query);
    if q.is_empty() {
        return true;
    }
    let t = normalize_search_text(text);
    q.split_whitespace().all(|term| t.contains(term))
}

fn parse_sort_label_for_mode(label: &str, mode: LeftPaneMode) -> SortOrder {
    let s = label.to_lowercase();
    let raw = match s.as_str() {
        "default" => SortOrder::Default,
        "added" => SortOrder::Added,
        "network" => SortOrder::Network,
        "location" => SortOrder::Location,
        "name" => SortOrder::Name,
        "stars" => SortOrder::Stars,
        "recent" => SortOrder::Recent,
        "stars+recent" => SortOrder::StarsRecent,
        "recent+stars" => SortOrder::RecentStars,
        _ => SortOrder::Default,
    };
    match mode {
        LeftPaneMode::Stations => raw,
        LeftPaneMode::Files => match raw {
            SortOrder::Network | SortOrder::Location => SortOrder::Default,
            _ => raw,
        },
    }
}

// ── ICY log loader ────────────────────────────────────────────────────────────

/// Load persisted ICY entries from icyticker.log.
/// Lines may be either:
///   - Old format (no timestamp): "Artist - Title"
///   - New format: "HH:MM  Artist - Title"  or  "dd/mm/yyyy HH:MM  Artist - Title"
/// We display them as-is (the display field == the line content).
fn load_icy_log(path: &PathBuf) -> Vec<TickerEntry> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut entries: Vec<TickerEntry> = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // The raw field is used for dedup; since these are historical, use the
        // full display line as raw to avoid false dedup.
        entries.push(TickerEntry {
            raw: line.to_string(),
            display: line.to_string(),
            station: None,
            show: None,
            url: None,
            comment: None,
        });
    }

    // Keep only the last 200 entries to avoid huge history
    if entries.len() > 200 {
        let skip = entries.len() - 200;
        entries.drain(..skip);
    }

    entries
}

// ── Songs CSV loader ─────────────────────────────────────────────────────────

/// Load and deduplicate songs from ~/songs.csv.
/// CSV format: time,track,artist,genre,extra,comment (header line, then quoted fields)
/// We show: "hh:mm  Track - Artist" (or date prefix if not today)
/// Consecutive duplicate track+artist combos are collapsed to one entry.
fn load_songs_csv(path: &PathBuf) -> Vec<TickerEntry> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut entries: Vec<TickerEntry> = Vec::new();
    let mut last_raw = String::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("time,") {
            continue; // skip header
        }

        // Parse CSV fields (quoted with double-quotes)
        let fields = parse_csv_line(line);
        if fields.len() < 3 {
            continue;
        }

        let time_str = &fields[0];
        let track = fields[1].trim();
        let artist = fields[2].trim();
        // fields[5] is the comment/station column with format:
        //   "Station Name"
        //   "Station Name; episode-alias"            (NTS 1/2 no-vibra: alias is a slug)
        //   "Station Name; Show Title; episode-alias" (NTS 1/2 vibra match)
        //   "Station Name; https://..."              (NTS mixtape)
        // We want: station = first segment, show = human-readable title if present.
        // A slug (episode alias) is all-lowercase with hyphens and no spaces; skip it.
        let (station, show, url, comment): (Option<String>, Option<String>, Option<String>, Option<String>) = fields.get(5).map(|s| {
            let s = s.trim();
            if s.is_empty() {
                return (None, None, None, None);
            }
            let mut parts = s.splitn(3, ';');
            let name = parts.next().unwrap_or("").trim();
            let second = parts.next().unwrap_or("").trim();
            let third = parts.next().unwrap_or("").trim();
            let station = if name.is_empty() { None } else { Some(name.to_string()) };
            // Show title: present only when the second segment is a human-readable string
            // (contains spaces or uppercase). Slugs, URLs, and empty strings are skipped.
            let show = if second.is_empty()
                || second.starts_with("http")
                || (!second.contains(' ') && second == second.to_lowercase())
            {
                None
            } else {
                Some(second.to_string())
            };
            let url = if second.starts_with("http") {
                Some(second.to_string())
            } else if third.starts_with("http") {
                Some(third.to_string())
            } else {
                None
            };
            (station, show, url, Some(s.to_string()))
        }).unwrap_or((None, None, None, None));

        if track.is_empty() && artist.is_empty() {
            continue;
        }

        let raw = if artist.is_empty() {
            track.to_string()
        } else if track.is_empty() {
            artist.to_string()
        } else {
            format!("{} - {}", track, artist)
        };

        // Skip consecutive duplicates — but only for song entries (those with an artist).
        // Show-name-only entries (no artist) are always added so each recognition run
        // appears even when the NTS show hasn't changed.
        let is_song = !artist.is_empty();
        if is_song && raw == last_raw {
            continue;
        }
        if is_song {
            last_raw = raw.clone();
        }

        // Parse unix timestamp
        let ts_display = if let Ok(unix) = time_str.parse::<i64>() {
            use chrono::TimeZone;
            if let chrono::LocalResult::Single(dt) = chrono::Local.timestamp_opt(unix, 0) {
                format_timestamp(dt)
            } else {
                time_str.to_string()
            }
        } else {
            time_str.to_string()
        };

        let display = format!("{}  {}", ts_display, raw);
        entries.push(TickerEntry { raw, display, station, show, url, comment });
    }

    // Keep only the last 200 entries
    if entries.len() > 200 {
        let skip = entries.len() - 200;
        entries.drain(..skip);
    }

    entries
}

/// Minimal CSV line parser that handles double-quoted fields.
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_quotes {
                    // Check for escaped quote ""
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        current.push('"');
                    } else {
                        in_quotes = false;
                    }
                } else {
                    in_quotes = true;
                }
            }
            ',' if !in_quotes => {
                fields.push(current.clone());
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
    }
    fields.push(current);
    fields
}

fn resolve_nts_download_url(entry: &TickerEntry) -> Option<String> {
    if let Some(url) = entry.url.as_deref() {
        if url.contains("nts.live") {
            return Some(url.to_string());
        }
    }
    let comment = entry.comment.as_deref().unwrap_or("");
    if comment.is_empty() {
        return None;
    }
    let mut parts = comment.split(';').map(|s| s.trim());
    let station = parts.next().unwrap_or("");
    if !station.to_lowercase().contains("nts") {
        return None;
    }
    for p in parts {
        if p.starts_with("http://") || p.starts_with("https://") {
            if p.contains("nts.live") {
                return Some(p.to_string());
            }
            continue;
        }
        if p.is_empty() {
            continue;
        }
        if p.contains('/') {
            return Some(format!("https://www.nts.live/{}", p.trim_start_matches('/')));
        }
        if p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Some(format!("https://www.nts.live/shows/{}", p));
        }
    }
    None
}

fn load_stars(stars_path: &PathBuf) -> (HashMap<String, u8>, HashMap<String, u8>) {
    let content = match std::fs::read_to_string(stars_path) {
        Ok(c) => c,
        Err(_) => return (HashMap::new(), HashMap::new()),
    };
    let mut state = toml::from_str::<StarredState>(&content).unwrap_or_default();
    state.station_stars.retain(|_, v| {
        *v = (*v).min(3);
        *v > 0
    });
    state.file_stars.retain(|_, v| {
        *v = (*v).min(3);
        *v > 0
    });
    (state.station_stars, state.file_stars)
}

fn save_stars(
    stars_path: &PathBuf,
    station_stars: &HashMap<String, u8>,
    file_stars: &HashMap<String, u8>,
) -> anyhow::Result<()> {
    if let Some(parent) = stars_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let state = StarredState {
        station_stars: station_stars.clone(),
        file_stars: file_stars.clone(),
    };
    let rendered = toml::to_string_pretty(&state)?;
    std::fs::write(stars_path, rendered)?;
    Ok(())
}

fn load_random_history(path: &PathBuf) -> Vec<RandomHistoryEntry> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    match serde_json::from_str::<Vec<RandomHistoryEntry>>(&content) {
        Ok(v) => v,
        Err(_) => Vec::new(),
    }
}

fn save_random_history(path: &PathBuf, history: &[RandomHistoryEntry]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(history)?;
    std::fs::write(path, data)?;
    Ok(())
}

fn load_recent_state(path: &PathBuf) -> RecentState {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return RecentState::default(),
    };
    toml::from_str::<RecentState>(&content).unwrap_or_default()
}

fn save_recent_state(path: &PathBuf, state: &RecentState) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let rendered = toml::to_string_pretty(state)?;
    std::fs::write(path, rendered)?;
    Ok(())
}

fn load_file_positions(path: &PathBuf) -> HashMap<String, f64> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };
    let table = match toml::from_str::<toml::Value>(&content) {
        Ok(v) => v,
        Err(_) => return HashMap::new(),
    };
    let mut out = HashMap::new();
    if let Some(tbl) = table.get("file_positions").and_then(|v| v.as_table()) {
        for (k, v) in tbl {
            if let Some(f) = v.as_float() {
                if f.is_finite() && f >= 0.0 {
                    out.insert(k.clone(), f);
                }
            } else if let Some(i) = v.as_integer() {
                if i >= 0 {
                    out.insert(k.clone(), i as f64);
                }
            }
        }
    }
    out
}

fn save_file_positions(path: &PathBuf, positions: &HashMap<String, f64>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut map = toml::map::Map::new();
    let mut keys: Vec<&String> = positions.keys().collect();
    keys.sort();
    for k in keys {
        let v = positions.get(k).copied().unwrap_or(0.0).max(0.0);
        map.insert(k.clone(), toml::Value::Float(v));
    }
    let mut root = toml::map::Map::new();
    root.insert("file_positions".to_string(), toml::Value::Table(map));
    std::fs::write(path, toml::to_string_pretty(&toml::Value::Table(root))?)?;
    Ok(())
}

fn load_ui_session_state(path: &PathBuf) -> UiSessionState {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return UiSessionState::default(),
    };
    serde_json::from_str::<UiSessionState>(&content).unwrap_or_default()
}

fn save_ui_session_state(path: &PathBuf, state: &UiSessionState) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(state)?;
    std::fs::write(path, data)?;
    Ok(())
}

fn copy_to_clipboard(text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let try_cmd = |bin: &str, args: &[&str]| -> anyhow::Result<bool> {
        let mut child = match Command::new(bin)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return Ok(false),
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let status = child.wait()?;
        Ok(status.success())
    };

    if try_cmd("wl-copy", &[])? {
        return Ok(());
    }
    if try_cmd("xclip", &["-selection", "clipboard"])? {
        return Ok(());
    }
    if try_cmd("xsel", &["--clipboard", "--input"])? {
        return Ok(());
    }

    // OSC52 fallback (works over SSH/tmux in supporting terminals)
    if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        let b64 = base64_encode(text.as_bytes());
        let osc = format!("\x1b]52;c;{}\x07", b64);
        let seq = if std::env::var_os("TMUX").is_some() {
            // tmux pass-through wrapper
            format!("\x1bPtmux;\x1b{}\x1b\\", osc)
        } else {
            osc
        };
        if tty.write_all(seq.as_bytes()).is_ok() {
            return Ok(());
        }
    }

    anyhow::bail!("clipboard unavailable (wl-copy/xclip/xsel/osc52)")
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i];
        let b1 = if i + 1 < data.len() { data[i + 1] } else { 0 };
        let b2 = if i + 2 < data.len() { data[i + 2] } else { 0 };
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        if i + 1 < data.len() {
            out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if i + 2 < data.len() {
            out.push(TABLE[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}

fn is_playable_audio_path(path: &std::path::Path) -> bool {
    let ext = match path.extension().and_then(|e| e.to_str()) {
        Some(e) => e.to_lowercase(),
        None => return false,
    };
    matches!(
        ext.as_str(),
        "mp3" | "m4a" | "aac" | "flac" | "ogg" | "opus" | "wav" | "aiff" | "aif"
            | "webm" | "mp4" | "mkv"
    )
}

fn load_local_files(dir: &PathBuf) -> Vec<LocalFileEntry> {
    fn visit(dir: &std::path::Path, out: &mut Vec<LocalFileEntry>) {
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return,
        };
        for entry in rd.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, out);
                continue;
            }
            if !path.is_file() || !is_playable_audio_path(&path) {
                continue;
            }
            let meta = entry.metadata().ok();
            out.push(LocalFileEntry {
                name: path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string(),
                size_bytes: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                modified: meta.and_then(|m| m.modified().ok()),
                path,
            });
        }
    }

    let mut out = Vec::new();
    visit(dir, &mut out);

    out.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

fn extract_tracklist_lines(text: &str) -> Vec<String> {
    let normalized = text
        .replace("Tracklist:", "\n")
        .replace("TRACKLIST:", "\n")
        .replace("Track list:", "\n")
        .replace(". ", "\n");

    normalized
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| !l.starts_with("http://") && !l.starts_with("https://"))
        .filter(|l| {
            l.contains(" by ")
                || l.contains(" - ")
                || l.contains(" – ")
                || l.contains(": ")
                || l.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
        })
        .map(|s| {
            s.trim_start_matches(|c: char| c == '-' || c == '|' || c == '*' || c == '.' || c.is_whitespace())
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .take(300)
        .collect()
}

fn probe_file_metadata(path: &std::path::Path) -> Option<FileMetadata> {
    let output = std::process::Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-print_format")
        .arg("json")
        .arg("-show_format")
        .arg("-show_streams")
        .arg("-show_chapters")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;

    let format = &v["format"];
    let tags = format["tags"].as_object();
    let tag = |name: &str| -> Option<String> {
        let map = tags?;
        for (k, val) in map {
            if k.eq_ignore_ascii_case(name) {
                if let Some(s) = val.as_str() {
                    if !s.trim().is_empty() {
                        return Some(s.to_string());
                    }
                }
            }
        }
        None
    };

    let mut meta = FileMetadata::default();
    meta.title = tag("title");
    meta.artist = tag("artist");
    meta.album = tag("album");
    meta.date = tag("date").or_else(|| tag("creation_time"));
    meta.genre = tag("genre");
    let lyrics = tag("lyrics").or_else(|| tag("unsyncedlyrics"));
    let description = tag("description");
    let comment = tag("comment");
    meta.description = description.clone().or_else(|| {
        comment
            .as_deref()
            .filter(|c| !c.starts_with("http://") && !c.starts_with("https://"))
            .map(|s| s.to_string())
    });
    meta.duration_secs = format["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| format["duration"].as_f64());
    meta.bitrate_kbps = format["bit_rate"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| format["bit_rate"].as_u64())
        .map(|b| b / 1000);

    if let Some(streams) = v["streams"].as_array() {
        if let Some(audio) = streams
            .iter()
            .find(|s| s["codec_type"].as_str() == Some("audio"))
        {
            meta.codec = audio["codec_name"].as_str().map(|s| s.to_string());
            meta.sample_rate_hz = audio["sample_rate"]
                .as_str()
                .and_then(|s| s.parse::<u32>().ok())
                .or_else(|| audio["sample_rate"].as_u64().map(|n| n as u32));
            meta.channels = audio["channels"]
                .as_u64()
                .map(|n| n as u8);
            if meta.bitrate_kbps.is_none() {
                meta.bitrate_kbps = audio["bit_rate"]
                    .as_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    .or_else(|| audio["bit_rate"].as_u64())
                    .map(|b| b / 1000);
            }
        }
    }

    let mut tracklist = Vec::new();
    if let Some(l) = lyrics.as_deref() {
        tracklist.extend(extract_tracklist_lines(l));
    }
    if let Some(d) = description.as_deref() {
        tracklist.extend(extract_tracklist_lines(d));
    }
    if let Some(c) = comment.as_deref() {
        tracklist.extend(extract_tracklist_lines(c));
    }
    if !tracklist.is_empty() {
        let mut dedup = std::collections::HashSet::new();
        meta.tracklist = tracklist
            .into_iter()
            .filter(|s| dedup.insert(s.to_lowercase()))
            .take(300)
            .collect();
    }

    if let Some(chapters) = v["chapters"].as_array() {
        for ch in chapters {
            let start = ch["start_time"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| ch["start"].as_f64())
                .unwrap_or(0.0);
            let end = ch["end_time"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| ch["end"].as_f64())
                .unwrap_or(start);
            let title = ch["tags"]["title"]
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("{} - {}", fmt_secs(start), fmt_secs(end)));
            meta.chapters.push(FileChapter {
                title,
                start_secs: start,
                end_secs: end,
            });
        }
    }

    Some(meta)
}

fn fmt_secs(v: f64) -> String {
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

fn escape_vds_field(s: &str) -> String {
    s.replace('\t', " ").replace('\n', " ").replace('\r', " ")
}

fn export_songs_vds(path: &PathBuf, entries: &[TickerEntry]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut buf = String::from("display\traw\tstation\tshow\turl\tcomment\n");
    for e in entries {
        let station = e.station.as_deref().unwrap_or("");
        let show = e.show.as_deref().unwrap_or("");
        let url = e.url.as_deref().unwrap_or("");
        let comment = e.comment.as_deref().unwrap_or("");
        buf.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\n",
            escape_vds_field(&e.display),
            escape_vds_field(&e.raw),
            escape_vds_field(station),
            escape_vds_field(show),
            escape_vds_field(url),
            escape_vds_field(comment)
        ));
    }
    std::fs::write(path, buf)?;
    Ok(())
}

// ── Daemon connection handler ────────────────────────────────────────────────

async fn connection_handler(
    daemon_addr: String,
    tx: mpsc::Sender<AppMessage>,
    mut cmd_rx: mpsc::Receiver<Command>,
) {
    let mut retry_delay = Duration::from_millis(100);
    let max_retry_delay = Duration::from_secs(5);

    loop {
        // Try to connect to daemon
        match tokio::net::TcpStream::connect(&daemon_addr).await {
            Ok(stream) => {
                info!("Connected to daemon at {}", daemon_addr);
                let _ = tx.send(AppMessage::DaemonConnected).await;
                retry_delay = Duration::from_millis(100);

                let msg = Message::Command(Command::GetState);
                if let Ok(encoded) = msg.encode() {
                    let _ = stream.writable().await;
                    let _ = stream.try_write(&encoded);
                }

                let mut read_buf: Vec<u8> = Vec::new();
                let mut write_buf: Vec<u8> = Vec::new();

                loop {
                    tokio::select! {
                        result = stream.readable() => {
                            if result.is_err() {
                                let _ = tx
                                    .send(AppMessage::DaemonDisconnected("socket error".into()))
                                    .await;
                                break;
                            }

                            let mut buf = [0u8; 4096];
                            match stream.try_read(&mut buf) {
                                Ok(0) => {
                                    let _ = tx
                                        .send(AppMessage::DaemonDisconnected(
                                            "connection closed".into(),
                                        ))
                                        .await;
                                    break;
                                }
                                Ok(n) => {
                                    read_buf.extend_from_slice(&buf[..n]);
                                    while read_buf.len() >= 4 {
                                        match Message::decode(&read_buf) {
                                            Ok((Message::Broadcast(b), consumed)) => {
                                                read_buf.drain(..consumed);
                                                match b {
                                                    Broadcast::State { data } => {
                                                        let _ = tx
                                                            .send(AppMessage::StateUpdated(data))
                                                            .await;
                                                    }
                                                    Broadcast::Icy { title } => {
                                                        let _ = tx
                                                            .send(AppMessage::IcyUpdated(title))
                                                            .await;
                                                    }
                                                    Broadcast::Log { message } => {
                                                        let _ = tx
                                                            .send(AppMessage::Log(message))
                                                            .await;
                                                    }
                                                    Broadcast::Error { message } => {
                                                        let _ = tx
                                                            .send(AppMessage::Log(format!(
                                                                "error: {}",
                                                                message
                                                            )))
                                                            .await;
                                                    }
                                                }
                                            }
                                            Ok((_, consumed)) => {
                                                read_buf.drain(..consumed);
                                            }
                                            Err(_) => break,
                                        }
                                    }
                                }
                                Err(ref e)
                                    if e.kind() == std::io::ErrorKind::WouldBlock => {}
                                Err(e) => {
                                    let _ = tx
                                        .send(AppMessage::DaemonDisconnected(format!(
                                            "read error: {}",
                                            e
                                        )))
                                        .await;
                                    break;
                                }
                            }
                        }

                        Some(cmd) = cmd_rx.recv() => {
                            let msg = Message::Command(cmd);
                            if let Ok(encoded) = msg.encode() {
                                write_buf.extend_from_slice(&encoded);
                            }

                            if !write_buf.is_empty() {
                                if stream.writable().await.is_ok() {
                                    match stream.try_write(&write_buf) {
                                        Ok(n) => {
                                            write_buf.drain(..n);
                                        }
                                        Err(ref e)
                                            if e.kind() == std::io::ErrorKind::WouldBlock => {}
                                        Err(e) => {
                                            warn!("Write error: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                // Daemon not reachable, try to start it
                if e.kind() == std::io::ErrorKind::ConnectionRefused {
                    info!("Daemon not running, starting it…");
                    let _ = tx.send(AppMessage::Log("Starting daemon…".to_string())).await;
                    if let Err(e) = start_daemon().await {
                        warn!("Failed to start daemon: {}", e);
                        let _ = tx.send(AppMessage::Log(format!("Failed to start daemon: {}", e))).await;
                    }
                    // Wait for daemon to start (longer on Windows)
                    #[cfg(windows)]
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    #[cfg(not(windows))]
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    retry_delay = Duration::from_millis(100);
                } else {
                    warn!("Failed to connect to daemon: {}", e);
                    let _ = tx.send(AppMessage::Log(format!("Connection error: {}", e))).await;
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(max_retry_delay);
                }
            }
        }
    }
}

async fn start_daemon() -> anyhow::Result<()> {
    let current_exe = std::env::current_exe()?;
    
    #[cfg(windows)]
    let daemon_name = "radio-daemon.exe";
    #[cfg(not(windows))]
    let daemon_name = "radio-daemon";
    
    let daemon_path = current_exe
        .parent()
        .map(|p| p.join(daemon_name))
        .unwrap_or_else(|| PathBuf::from(daemon_name));

    info!("Starting daemon from: {:?}", daemon_path);

    // Verify the daemon exists before trying to spawn
    if !daemon_path.exists() {
        anyhow::bail!("Daemon not found at: {}", daemon_path.display());
    }

    let mut cmd = tokio::process::Command::new(daemon_path);
    cmd.stdout(std::process::Stdio::null());

    // On Windows, capture stderr for debugging
    #[cfg(windows)]
    {
        cmd.stderr(std::process::Stdio::piped());
    }
    #[cfg(not(windows))]
    {
        cmd.stderr(std::process::Stdio::null());
    }

    let child = cmd.spawn()?;
    info!("Daemon spawned with PID: {:?}", child.id());

    Ok(())
}

// ── NTS API fetch ─────────────────────────────────────────────────────────────

async fn fetch_nts_channel(ch_idx: usize) -> anyhow::Result<NtsChannel> {
    let resp: serde_json::Value = reqwest::get("https://www.nts.live/api/v2/live")
        .await?
        .json()
        .await?;

    let channel = &resp["results"][ch_idx];

    let now_obj = &channel["now"];
    let now_show = parse_nts_show(now_obj)?;

    let mut upcoming = Vec::new();
    // next, next2 .. next17
    for i in 1usize..=17 {
        let key = if i == 1 {
            "next".to_string()
        } else {
            format!("next{}", i)
        };
        let show_obj = &channel[&key];
        if show_obj.is_null() {
            break;
        }
        if let Ok(show) = parse_nts_show(show_obj) {
            upcoming.push(show);
        }
    }

    Ok(NtsChannel {
        now: now_show,
        upcoming,
        fetched_at: chrono::Local::now(),
    })
}

fn parse_nts_show(obj: &serde_json::Value) -> anyhow::Result<NtsShow> {
    let broadcast_title = obj["broadcast_title"]
        .as_str()
        .unwrap_or("Unknown Show")
        .to_string();

    let parse_ts = |key: &str| -> chrono::DateTime<chrono::Local> {
        obj[key]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Local))
            .unwrap_or_else(chrono::Local::now)
    };

    let start = parse_ts("start_timestamp");
    let end = parse_ts("end_timestamp");

    let details = &obj["embeds"]["details"];

    let description = details["description"]
        .as_str()
        .unwrap_or("")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .to_string();

    let location_short = details["location_short"].as_str().unwrap_or("").to_string();
    let location_long  = details["location_long"].as_str().unwrap_or("").to_string();

    let genres: Vec<String> = details["genres"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|g| g["value"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let moods: Vec<String> = details["moods"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["value"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let is_replay = broadcast_title.contains("(R)");

    Ok(NtsShow {
        broadcast_title,
        start,
        end,
        location_short,
        location_long,
        description,
        genres,
        moods,
        is_replay,
    })
}

// ── Entry point ──────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let data_dir = radio_tui::shared::platform::data_dir();
    // Keep tui_data_dir consistent with original path for backwards compatibility
    let tui_data_dir = dirs::data_dir()
        .map(|p| p.join("radio-tui"))
        .unwrap_or_else(|| radio_tui::shared::platform::temp_dir().join("radio-tui"));

    std::fs::create_dir_all(&data_dir)?;
    std::fs::create_dir_all(&tui_data_dir)?;

    let log_path = data_dir.join("tui.log");
    let icy_log_path = data_dir.join("icyticker.log");

    let songs_csv_path = dirs::home_dir()
        .map(|p| p.join("songs.csv"))
        .unwrap_or_else(|| radio_tui::shared::platform::temp_dir().join("songs.csv"));
    let songs_vds_path = tui_data_dir.join("songs.vds");
    let downloads_dir = dirs::home_dir()
        .map(|p| p.join("nts-downloads"))
        .unwrap_or_else(|| radio_tui::shared::platform::temp_dir().join("nts-downloads"));
    let stars_path = tui_data_dir.join("starred.toml");
    let random_history_path = tui_data_dir.join("random_history.json");
    let recent_path = tui_data_dir.join("recent.toml");
    let file_positions_path = tui_data_dir.join("file_positions.toml");
    let ui_state_path = tui_data_dir.join("ui_state.json");

    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    tracing_subscriber::fmt()
        .with_writer(log_file)
        .with_env_filter("info")
        .init();

    info!("TUI starting…");

    let config = Config::load()?;
    let app = App::new(
        config,
        log_path,
        icy_log_path,
        songs_csv_path,
        songs_vds_path,
        downloads_dir,
        stars_path,
        random_history_path,
        recent_path,
        file_positions_path,
        ui_state_path,
    );
    app.run().await?;

    Ok(())
}
