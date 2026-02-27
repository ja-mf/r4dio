//! AppState — shared read-only data passed to all components during render/event.
//!
//! Components read this for daemon state, but never mutate it.
//! The App event-loop is the only thing that writes to AppState.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use radio_proto::protocol::{DaemonState, PlaybackStatus};
use radio_proto::songs::RecognitionResult;

use crate::action::Workspace;
use crate::intent::RenderHint;
use crate::widgets::status_bar::InputMode;

/// Data about the currently playing file (position, duration, etc.)
#[derive(Debug, Clone, Default)]
pub struct PlaybackInfo {
    pub time_pos_secs: Option<f64>,
    pub duration_secs: Option<f64>,
    pub is_playing: bool,
    pub is_paused: bool,
    pub status: PlaybackStatus,
}

/// NTS show information.
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

/// NTS channel (now + upcoming).
#[derive(Clone, Debug)]
pub struct NtsChannel {
    pub now: NtsShow,
    pub upcoming: Vec<NtsShow>,
    pub fetched_at: chrono::DateTime<chrono::Local>,
}

/// A ticker entry (ICY or songs list).
#[derive(Clone, Debug)]
pub struct TickerEntry {
    pub raw: String,
    pub display: String,
    pub station: Option<String>,
    pub show: Option<String>,
    pub url: Option<String>,
    pub comment: Option<String>,
}

/// Metadata for a local audio file.
#[derive(Clone, Debug, Default)]
pub struct FileChapter {
    pub title: String,
    pub start_secs: f64,
    pub end_secs: f64,
}

#[derive(Clone, Debug, Default)]
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

/// A local file entry.
#[derive(Clone, Debug)]
pub struct LocalFileEntry {
    pub path: PathBuf,
    pub name: String,
    pub size_bytes: u64,
    pub modified: Option<std::time::SystemTime>,
}

/// Random history entry (for R = go-back in file mode).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RandomHistoryEntry {
    pub path: String,
    pub start_secs: f64,
    pub saved_at_epoch: i64,
}

/// The full shared state of the application.
/// Components read this; only the App event-loop writes to it.
pub struct AppState {
    // ── Daemon ─────────────────────────────────────────────────────────────
    pub daemon_state: DaemonState,
    pub connected: bool,
    pub error_message: Option<String>,

    // ── Stars / recent ──────────────────────────────────────────────────────
    pub station_stars: HashMap<String, u8>,
    pub file_stars: HashMap<String, u8>,
    pub recent_station: HashMap<String, i64>,
    pub recent_file: HashMap<String, i64>,

    // ── Files ───────────────────────────────────────────────────────────────
    pub files: Vec<LocalFileEntry>,
    pub file_metadata_cache: HashMap<String, FileMetadata>,
    pub file_positions: HashMap<String, f64>,

    // ── ICY / songs ticker ──────────────────────────────────────────────────
    pub icy_history: Vec<TickerEntry>,
    /// Songs history loaded from songs.vds (newest last).
    pub songs_history: Vec<RecognitionResult>,
    /// When the station list cursor is on an NTS station, this holds the channel
    /// index (0 = NTS 1, 1 = NTS 2). None when not hovering an NTS row.
    pub nts_hover_channel: Option<usize>,

    // ── NTS ─────────────────────────────────────────────────────────────────
    pub nts_ch1: Option<NtsChannel>,
    pub nts_ch2: Option<NtsChannel>,
    pub nts_ch1_error: Option<String>,
    pub nts_ch2_error: Option<String>,

    // ── UI mode ─────────────────────────────────────────────────────────────
    pub workspace: Workspace,
    pub input_mode: InputMode,

    // ── Session ─────────────────────────────────────────────────────────────
    pub last_nonzero_volume: f32,
    /// IPC log messages (WARN/ERROR only, from daemon).
    pub logs: Vec<String>,
    /// Cached lines from tui.log (refreshed periodically by App).
    pub tui_log_lines: Vec<String>,

    // ── Audio levels ──────────────────────────────────────────────────────────
    /// Current main RMS level in dBFS (PCM-derived for stations, lavfi for files).
    /// -90.0 = silence / not playing.
    pub audio_level: f32,
    /// Debug RMS level in dBFS from mpv lavfi astats observer.
    pub mpv_audio_level: f32,
    /// Ballistic-smoothed level used by the VU meter body.
    pub vu_level: f32,
    /// Peak-hold RMS level in dBFS (decays over time).
    pub peak_level: f32,
    /// Hold timeout for the peak marker.
    pub peak_hold_until: std::time::Instant,
    /// Instant of last peak/decay update.
    pub peak_last_update: std::time::Instant,
    /// Instant of last real AudioLevel message (for silence decay).
    pub last_audio_update: std::time::Instant,
    /// Exponential moving average of the RMS level — the "centre" of the signal.
    /// Time constant ~4 s.  Initialised to -30.0.
    pub meter_mean_db: f32,
    /// Exponential moving average of |rms - mean| — the "spread" of the signal.
    /// Time constant ~8 s.  Initialised to 6.0 dB (minimum useful window half-width).
    pub meter_spread_db: f32,

    // ── Scope / oscilloscope PCM ring buffer ─────────────────────────────────
    /// Rolling buffer of normalised f32 PCM samples (mono, 44100 Hz).
    /// Holds ~2 seconds of audio (88200 samples).  The scope panel reads the
    /// most recent `SCOPE_SAMPLES` entries from the back.
    pub pcm_ring: VecDeque<f32>,
    /// Jitter buffer for bursty station PCM arrival; consumed at steady frame cadence.
    pub pcm_pending: VecDeque<f32>,
    /// True once enough samples are buffered to start stable scope/VU playback.
    pub pcm_pending_started: bool,

    // ── Intent render hints ──────────────────────────────────────────────────
    /// How to render the pause/play icon.
    pub pause_hint: RenderHint,
    /// How to render the volume indicator.
    pub volume_hint: RenderHint,
    /// How to render the current-station indicator.
    pub station_hint: RenderHint,

    // ── Paths ───────────────────────────────────────────────────────────────
    pub downloads_dir: PathBuf,
    pub icy_log_path: PathBuf,
    pub songs_csv_path: PathBuf,
    pub songs_vds_path: PathBuf,
    pub tui_log_path: PathBuf,
    pub random_history: Vec<RandomHistoryEntry>,
}

impl AppState {
    /// Convenience: currently playing station name.
    pub fn current_station_name(&self) -> Option<&str> {
        self.daemon_state
            .current_station
            .and_then(|i| self.daemon_state.stations.get(i))
            .map(|s| s.name.as_str())
    }

    /// Stars for a station by name.
    pub fn station_stars_for(&self, name: &str) -> u8 {
        self.station_stars.get(name).copied().unwrap_or(0)
    }

    /// Stars for a file by path.
    pub fn file_stars_for(&self, path: &str) -> u8 {
        self.file_stars.get(path).copied().unwrap_or(0)
    }

    /// Saved position for a file.
    pub fn file_position_for(&self, path: &str) -> f64 {
        self.file_positions.get(path).copied().unwrap_or(0.0)
    }
}
