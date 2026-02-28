//! App — new component-based event loop.
//!
//! Architecture:
//! - `App` owns all components and `AppState` (shared read-only data for components).
//! - A `tokio::mpsc` channel carries `AppMessage` events in from background tasks.
//! - The event loop draws each frame, then awaits the next message.
//! - Components return `Vec<Action>`; App dispatches each Action.
//! - Commands to the daemon flow out through a separate `cmd_tx` channel.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::PathBuf;
use std::time::Duration;


use ratatui::crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    Terminal,
};
use reqwest::header::HeaderValue;
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, mpsc, Mutex as TokioMutex};
use tracing::{debug, info, warn};

use radio_proto::protocol::{Command, DaemonState, MpvHealth, Station};
use radio_proto::state::StateManager;

use crate::core::DaemonEvent;
use crate::BroadcastMessage;

use radio_proto::songs::{
    append_to_vds, load_vds, make_job_id, recognize_via_nts, recognize_via_nts_mixtape,
    recognize_via_vibra, vibra_rec_string, RecognitionResult, VdsPatch,
};

use crate::{
    action::{Action, ComponentId, StarContext, Workspace},
    app_state::{
        AppState, DownloadStatus, FileChapter, FileMetadata, LocalFileEntry, NtsChannel, NtsShow,
        RandomHistoryEntry, TickerEntry,
    },
    component::Component,
    components::{
        file_list::FileList, file_meta::FileMeta, header::Header, help_overlay::HelpOverlay,
        icy_ticker::IcyTicker, log_panel::LogPanel, nts_panel::NtsPanel, scope_panel::ScopePanel,
        songs_ticker::SongsTicker, station_list::StationList,
    },
    widgets::{
        status_bar::{self, InputMode},
        toast::{Severity, ToastManager},
    },
    workspace::{RightPane, WorkspaceManager},
};

// ── Internal event bus ────────────────────────────────────────────────────────

enum AppMessage {
    Event(Event),
    StateUpdated(DaemonState),
    IcyUpdated(Option<String>),
    Log(String),
    NtsUpdated(usize, NtsChannel),
    NtsError(usize, String),
    /// Initial recognition row (written immediately on 'i' press).
    RecognitionStarted(RecognitionResult),
    /// A VDS patch arrived from a background data-collection task.
    RecognitionPatch(String, VdsPatch),
    /// Vibra recognition succeeded.
    RecognitionComplete(String, String), // job_id, display string
    /// Vibra recognition produced no match (or stream_url was absent).
    RecognitionNoMatch,
    /// Fires after a 1-second delay to start the next queued recognition job.
    RecognitionQueueNext,
    /// Real-time audio RMS level from daemon (dBFS).
    AudioLevel(f32),
    /// Raw PCM chunk (mono f32 normalised -1..1, 44100 Hz) for scope display.
    PcmChunk(std::sync::Arc<Vec<f32>>),
    /// Independent render tick — drives VU-meter animation / peak decay.
    MeterTick,
    /// Download completed (success or failure).
    DownloadComplete {
        url: String,
        result: Result<PathBuf, String>,
    },
    PassivePollOutcome {
        cycle_id: u64,
        outcome: StationPollOutcome,
    },
    PassivePollCycleDone {
        cycle_id: u64,
        total: usize,
        elapsed_ms: u128,
    },
}

const STREAM_PCM_RATE_HZ: usize = 44_100;
const METER_FPS: usize = 25;
const STREAM_FRAME_SAMPLES: usize = STREAM_PCM_RATE_HZ / METER_FPS; // 1764 @ 44.1kHz
const PCM_RING_MAX: usize = STREAM_PCM_RATE_HZ * 2; // ~2 seconds for scope history
const PCM_JITTER_TARGET: usize = STREAM_FRAME_SAMPLES * 40; // ~1.6s target buffer
const PCM_JITTER_STOP: usize = STREAM_FRAME_SAMPLES * 4; // ~160ms stop threshold
const PCM_JITTER_MAX: usize = STREAM_FRAME_SAMPLES * 125; // ~5.0s cap
const VU_ATTACK_TAU_SECS: f32 = 0.045;
const VU_RELEASE_TAU_SECS: f32 = 0.24;
const PEAK_MINOR_HOLD_MS: u64 = 45;
const PEAK_MAJOR_HOLD_MS: u64 = 120;
const PEAK_HOLD_RESET_DB: f32 = 0.35;
const PEAK_RELEASE_TAU_SECS: f32 = 0.09;
const PEAK_FALL_DB_PER_SEC: f32 = 28.0;
const MIN_POLL_INTERVAL_SECS: u64 = 10;
const NTS_POLL_TASK_TIMEOUT_SECS: u64 = 12;
// Non-NTS ICY polling tuning:
//
//   concurrency=6   moderate; ~6 simultaneous TCP streams, cycle completes in ~12s for 71 stations
//   max_jobs=64     covers all ~71 non-NTS stations in a single cycle
//   connect=4s      Cambridge Radio stalls at TCP connect and hits exactly 4s — correct ceiling
//   request=8s      longest observed header round-trip is ~4s; 8s gives 2× headroom
//   metadata=5s     longest observed metadata read is ~3.1s (Skid Row Radio); 5s is safe margin
//   icy_blocks=4    needed by metaint=16000 stations (4×16KB=64KB); adaptive() reduces for large metaint
//   cycle_budget=30s safety backstop; cycles finish well within this in practice
const NON_NTS_MAX_CONCURRENCY: usize = 6;
const NON_NTS_MAX_JOBS_PER_CYCLE: usize = 64;
const NON_NTS_CYCLE_BUDGET_SECS: u64 = 30;
const NON_NTS_CONNECT_TIMEOUT_MS: u64 = 4_000;
const NON_NTS_REQUEST_TIMEOUT_MS: u64 = 8_000;
const NON_NTS_METADATA_TIMEOUT_MS: u64 = 5_000;
const NON_NTS_ICY_BLOCKS: usize = 4;

#[derive(Debug, Clone)]
enum StationPollTarget {
    NtsLive {
        station_name: String,
        channel_idx: usize,
    },
    NtsMixtape {
        station_name: String,
        mixtape_url: String,
    },
    NonNtsIcy {
        station_name: String,
        stream_url: String,
    },
}

impl StationPollTarget {
    fn resolver_label(&self) -> &'static str {
        match self {
            Self::NtsLive { .. } => "nts-live",
            Self::NtsMixtape { .. } => "nts-mixtape",
            Self::NonNtsIcy { .. } => "icy-probe",
        }
    }
}

#[derive(Debug, Clone)]
struct StationPollOutcome {
    station_name: String,
    resolver: String,
    show: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct NonNtsPollJob {
    ord: usize,
    station_name: String,
    stream_url: String,
}

// ── Persistence serde structs ─────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct UiSessionState {
    workspace: String,
    focused_component: String,
    selected_station_name: Option<String>,
    selected_file_path: Option<String>,
    files_right_maximized: bool,
    station_sort_order: String,
    file_sort_order: String,
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

// ── Pane area tracking ────────────────────────────────────────────────────────

/// Stores the last-drawn layout rects for each focusable pane.
/// Used by `handle_mouse` to do hit-testing without recomputing the layout.
#[derive(Default, Clone)]
struct PaneAreas {
    station_list: Rect,
    file_list: Rect,
    icy_ticker: Rect,
    songs_ticker: Rect,
    nts_panel: Rect,   // whichever NTS panel is currently shown
    nts_overlay: Rect, // hover overlay on top of station list (may be default/zero when hidden)
    file_meta: Rect,
    log_panel: Rect,
    scope: Rect, // scope panel in header (may be default/zero when hidden)
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct App {
    // ── Paths ─────────────────────────────────────────────────────────────────
    icy_log_path: PathBuf,
    songs_csv_path: PathBuf,
    songs_vds_path: PathBuf,
    tui_log_path: PathBuf,
    stars_path: PathBuf,
    random_history_path: PathBuf,
    recent_path: PathBuf,
    file_positions_path: PathBuf,
    ui_state_path: PathBuf,

    // ── Shared state (passed read-only to components) ─────────────────────────
    pub state: AppState,

    // ── Components ────────────────────────────────────────────────────────────
    header: Header,
    station_list: StationList,
    file_list: FileList,
    icy_ticker: IcyTicker,
    songs_ticker: SongsTicker,
    nts_panel_ch1: NtsPanel,
    nts_panel_ch2: NtsPanel,
    file_meta: FileMeta,
    log_panel: LogPanel,
    help_overlay: HelpOverlay,
    scope_panel: ScopePanel,

    // ── Workspace / layout ────────────────────────────────────────────────────
    wm: WorkspaceManager,

    // ── Session bookkeeping ───────────────────────────────────────────────────
    cmd_tx: mpsc::Sender<DaemonEvent>,
    state_manager: std::sync::Arc<StateManager>,
    initial_loaded: bool,
    pending_station_restore: Option<String>,
    last_station_name: Option<String>,
    last_file_path: Option<String>,
    last_file_pos: f64,
    pending_resume_file: Option<(String, f64)>,
    jump_from_station: Option<Option<usize>>,

    /// Whether to quit on next iteration.
    should_quit: bool,

    /// Last-drawn layout rects — used for mouse hit-testing.
    pane_areas: PaneAreas,

    /// Toast notification manager.
    toast: ToastManager,

    /// Previous mpv health — used to detect transitions for toast notifications.
    prev_mpv_health: MpvHealth,

    /// Last ICY title received for the currently-playing station, keyed by
    /// station name.  Updated only by `IcyUpdated` messages; cleared when the
    /// station changes.  Survives `StateUpdated` replacements so the value is
    /// always available when the user presses `i`, even if the daemon
    /// temporarily reports `icy_title: None` (e.g. right after a reconnect).
    last_known_icy: Option<(String, String)>, // (station_name, icy_title)

    /// Sender used by recognition background tasks to report results.
    recognition_tx: Option<mpsc::Sender<AppMessage>>,

    // ── Recognition safety queue ──────────────────────────────────────────────
    /// Pending recognition jobs
    /// (station_name, stream_url, icy_title, nts_ch, nts_mixtape_url). Max 3.
    /// Cleared automatically when the station changes.
    recognize_queue: std::collections::VecDeque<(
        Option<String>,
        Option<String>,
        Option<String>,
        Option<usize>,
        Option<String>,
    )>,
    /// True while a recognition job is in flight (vibra running).
    recognize_in_flight: bool,
    /// Station name that the current queue belongs to; used to detect station changes.
    recognize_active_station: Option<String>,

    // ── Download safety queue ─────────────────────────────────────────────────
    /// Pending downloads (url, display_name). Executed one at a time.
    download_queue: std::collections::VecDeque<(String, String)>,
    /// True while a download is in progress.
    download_in_flight: bool,

    // ── Pending-intent trackers ───────────────────────────────────────────────
    /// Intent tracker for play/pause state (true = playing).
    intent_pause: crate::intent::IntentState<bool>,
    /// Intent tracker for volume (0.0–1.0).
    intent_volume: crate::intent::IntentState<f32>,
    /// Intent tracker for current station index.
    intent_station: crate::intent::IntentState<Option<usize>>,

    // ── Passive background polling ───────────────────────────────────────────
    auto_polling_enabled: bool,
    auto_poll_interval: Duration,
    auto_poll_in_flight: bool,
    auto_poll_cycle_id: u64,
    auto_poll_cycle_total: usize,
    auto_poll_cycle_seen: usize,
    auto_poll_cycle_changed: usize,
    auto_poll_cycle_unchanged: usize,
    auto_poll_cycle_errors: usize,
    non_nts_poll_cursor: usize,
}

impl App {
    pub fn new(
        icy_log_path: PathBuf,
        songs_csv_path: PathBuf,
        songs_vds_path: PathBuf,
        tui_log_path: PathBuf,
        stars_path: PathBuf,
        random_history_path: PathBuf,
        recent_path: PathBuf,
        file_positions_path: PathBuf,
        ui_state_path: PathBuf,
        downloads_dir: PathBuf,
        cmd_tx: mpsc::Sender<DaemonEvent>,
        state_manager: std::sync::Arc<StateManager>,
        auto_polling_enabled: bool,
        poll_interval_secs: u64,
    ) -> Self {
        let icy_history = load_icy_log(&icy_log_path);
        let songs_history = load_vds(&songs_vds_path, 200);
        let files = load_local_files(&downloads_dir);
        let (station_stars, file_stars) = load_stars(&stars_path);
        let random_history = load_random_history(&random_history_path);
        let recent = load_recent_state(&recent_path);
        let file_positions = load_file_positions(&file_positions_path);
        let ui_state = load_ui_session_state(&ui_state_path);

        let mut file_metadata_cache: HashMap<String, FileMetadata> = HashMap::new();
        // Pre-probe files that are already in cache (will be picked up in refresh)
        let _ = &files; // make borrow checker happy

        let state = AppState {
            daemon_state: DaemonState::default(),
            connected: false,
            error_message: None,
            station_stars: station_stars.clone(),
            file_stars: file_stars.clone(),
            recent_station: recent.recent_station.clone(),
            recent_file: recent.recent_file.clone(),
            files: files.clone(),
            file_metadata_cache,
            file_positions: file_positions.clone(),
            icy_history: icy_history.clone(),
            last_known_icy: None,
            songs_history: songs_history.clone(),
            nts_hover_channel: None,
            nts_ch1: None,
            nts_ch2: None,
            nts_ch1_error: None,
            nts_ch2_error: None,
            station_poll_titles: HashMap::new(),
            workspace: Workspace::Radio,
            input_mode: InputMode::Normal,
            last_nonzero_volume: 0.7,
            logs: Vec::new(),
            tui_log_lines: Vec::new(),
            audio_level: -90.0,
            mpv_audio_level: -90.0,
            vu_level: -90.0,
            peak_level: -90.0,
            peak_hold_until: std::time::Instant::now(),
            peak_last_update: std::time::Instant::now(),
            last_audio_update: std::time::Instant::now(),
            meter_mean_db: -18.0,
            meter_spread_db: 6.0,
            pause_hint: crate::intent::RenderHint::Normal,
            volume_hint: crate::intent::RenderHint::Normal,
            station_hint: crate::intent::RenderHint::Normal,
            downloads_dir: downloads_dir.clone(),
            icy_log_path: icy_log_path.clone(),
            songs_csv_path: songs_csv_path.clone(),
            songs_vds_path: songs_vds_path.clone(),
            tui_log_path: tui_log_path.clone(),
            random_history,
            pcm_ring: std::collections::VecDeque::new(),
            pcm_pending: std::collections::VecDeque::new(),
            pcm_pending_started: false,
            download_statuses: HashMap::new(),
        };

        // Restore workspace/focus from session
        let workspace = if ui_state.workspace.eq_ignore_ascii_case("files") {
            Workspace::Files
        } else {
            Workspace::Radio
        };

        let mut wm = WorkspaceManager::new();
        wm.set_workspace(workspace);

        // Restore focused component
        match ui_state.focused_component.to_lowercase().as_str() {
            "icy" | "icyticker" => wm.focus_set(ComponentId::IcyTicker),
            "songs" | "songsticker" => wm.focus_set(ComponentId::SongsTicker),
            "meta" | "filemeta" => wm.focus_set(ComponentId::FileMeta),
            "filelist" => wm.focus_set(ComponentId::FileList),
            _ => {}
        };

        // Restore file selection
        let selected_file_path = ui_state
            .selected_file_path
            .clone()
            .or_else(|| ui_state.last_file_path.clone());

        let mut app = Self {
            icy_log_path,
            songs_csv_path,
            songs_vds_path,
            tui_log_path,
            stars_path,
            random_history_path,
            recent_path,
            file_positions_path,
            ui_state_path,
            state,
            header: Header::new(),
            station_list: StationList::new(),
            file_list: FileList::new(),
            icy_ticker: IcyTicker::new(),
            songs_ticker: SongsTicker::new(),
            nts_panel_ch1: NtsPanel::new(0),
            nts_panel_ch2: NtsPanel::new(1),
            file_meta: FileMeta::new(),
            log_panel: LogPanel::new(),
            help_overlay: HelpOverlay::new(),
            scope_panel: ScopePanel::default(),
            wm,
            cmd_tx,
            state_manager,
            initial_loaded: false,
            pending_station_restore: ui_state.selected_station_name.clone(),
            last_station_name: ui_state.last_station_name.clone(),
            last_file_path: ui_state.last_file_path.clone(),
            last_file_pos: ui_state.last_file_pos.max(0.0),
            pending_resume_file: ui_state
                .last_file_path
                .clone()
                .map(|p| (p, ui_state.last_file_pos.max(0.0))),
            jump_from_station: None,
            should_quit: false,
            pane_areas: PaneAreas::default(),
            toast: ToastManager::new(),
            prev_mpv_health: MpvHealth::Absent,
            last_known_icy: None,
            recognition_tx: None, // set in run()
            recognize_queue: std::collections::VecDeque::new(),
            recognize_in_flight: false,
            recognize_active_station: None,
            download_queue: std::collections::VecDeque::new(),
            download_in_flight: false,
            intent_pause: crate::intent::IntentState::new(false),
            intent_volume: crate::intent::IntentState::new(0.7),
            intent_station: crate::intent::IntentState::new(None),
            auto_polling_enabled,
            auto_poll_interval: Duration::from_secs(poll_interval_secs.max(MIN_POLL_INTERVAL_SECS)),
            auto_poll_in_flight: false,
            auto_poll_cycle_id: 0,
            auto_poll_cycle_total: 0,
            auto_poll_cycle_seen: 0,
            auto_poll_cycle_changed: 0,
            auto_poll_cycle_unchanged: 0,
            auto_poll_cycle_errors: 0,
            non_nts_poll_cursor: 0,
        };

        // Restore file selection in FileList component
        if let Some(path) = selected_file_path {
            if let Some(idx) = app
                .state
                .files
                .iter()
                .enumerate()
                .find(|(_, f)| f.path.to_string_lossy() == path.as_str())
                .map(|(i, _)| i)
            {
                app.file_list.set_selected(idx);
            }
        }

        // Restore sort orders
        app.station_list
            .set_sort_from_label(&ui_state.station_sort_order);
        app.file_list.set_sort_from_label(&ui_state.file_sort_order);

        // Initial file list sync (stations arrive later via daemon state update).
        app.file_list.sync_files(&app.state);

        app
    }

    // ── Main run loop ─────────────────────────────────────────────────────────

    pub async fn run(
        mut self,
        mut broadcast_rx: broadcast::Receiver<BroadcastMessage>,
    ) -> anyhow::Result<()> {
        debug!("run(): enabling raw mode");
        enable_raw_mode()?;
        debug!("run(): raw mode enabled, entering alternate screen");
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        debug!("run(): alternate screen entered");
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        debug!("run(): terminal created, size={:?}", terminal.size());

        let (tx, mut rx) = mpsc::channel::<AppMessage>(1024);
        self.recognition_tx = Some(tx.clone());

        // Mark as connected immediately — we're running in-process.
        self.state.connected = true;
        self.push_log("r4dio started".to_string());

        // ── Background task: keyboard/mouse events ────────────────────────────
        let event_tx = tx.clone();
        tokio::task::spawn_blocking(move || loop {
            match event::read() {
                Ok(ev) => {
                    if event_tx.blocking_send(AppMessage::Event(ev)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        });

        // ── Background task: broadcast receiver (DaemonCore → AppMessage) ──────
        let bc_tx = tx.clone();
        let bc_state_manager = self.state_manager.clone();
        tokio::spawn(async move {
            loop {
                match broadcast_rx.recv().await {
                    Ok(msg) => {
                        let app_msg = match msg {
                            BroadcastMessage::StateUpdated => {
                                let state = bc_state_manager.get_state().await;
                                AppMessage::StateUpdated(state)
                            }
                            BroadcastMessage::IcyUpdated(title) => AppMessage::IcyUpdated(title),
                            BroadcastMessage::Log(s) => AppMessage::Log(s),
                            BroadcastMessage::AudioLevel(rms) => AppMessage::AudioLevel(rms),
                            BroadcastMessage::PcmChunk(chunk) => AppMessage::PcmChunk(chunk),
                        };
                        if bc_tx.send(app_msg).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("broadcast receiver lagged by {} messages", n);
                        // continue receiving
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
        });

        // ── Periodic timers ───────────────────────────────────────────────────
        let mut files_refresh = tokio::time::interval(Duration::from_secs(5));
        files_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut nts_refresh = tokio::time::interval(Duration::from_secs(60));
        nts_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut auto_poll_refresh = tokio::time::interval(self.auto_poll_interval);
        auto_poll_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Toast expiry check + spinner animation: 100ms for smooth braille animation
        let mut toast_tick = tokio::time::interval(Duration::from_millis(100));
        toast_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Component maintenance tick (filter cursors, lightweight expiries, etc.).
        let mut ui_tick = tokio::time::interval(Duration::from_millis(100));
        ui_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // tui.log tail refresh: every 2s, only when log panel is open
        let mut log_refresh = tokio::time::interval(Duration::from_secs(2));
        log_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // VU-meter/scope render tick: 25 Hz for stability over max smoothness.
        let mut meter_tick =
            tokio::time::interval(Duration::from_millis((1000 / METER_FPS) as u64));
        meter_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // ── Main loop ─────────────────────────────────────────────────────────
        let mut needs_redraw = true;

        if self.auto_polling_enabled {
            info!(
                "[poll] auto polling enabled (interval={}s)",
                self.auto_poll_interval.as_secs()
            );
            self.spawn_passive_poll_task(tx.clone(), "startup");
        } else {
            info!("[poll] auto polling disabled at startup");
        }

        loop {
            // Draw only when something changed (PCM accumulation doesn't need a redraw)
            if needs_redraw {
                terminal.draw(|f| self.draw(f))?;
            }
            needs_redraw = false;

            if self.should_quit {
                break;
            }

            // Wait for next event
            tokio::select! {
                Some(msg) = rx.recv() => {
                    const MAX_DRAIN: usize = 256;
                    let mut redraw = self.handle_message(msg).await;
                    let mut drained = 0usize;
                    let mut latest_audio: Option<f32> = None;

                    while drained < MAX_DRAIN {
                        let next = match rx.try_recv() {
                            Ok(v) => v,
                            Err(_) => break,
                        };
                        drained += 1;
                        match next {
                            AppMessage::AudioLevel(rms) => latest_audio = Some(rms),
                            AppMessage::PcmChunk(chunk) => {
                                let _ = self.handle_message(AppMessage::PcmChunk(chunk)).await;
                            }
                            other => {
                                if let Some(rms) = latest_audio.take() {
                                    let _ = self.handle_message(AppMessage::AudioLevel(rms)).await;
                                }
                                redraw |= self.handle_message(other).await;
                            }
                        }
                    }
                    if let Some(rms) = latest_audio {
                        let _ = self.handle_message(AppMessage::AudioLevel(rms)).await;
                    }
                    needs_redraw = redraw;
                }

                _ = files_refresh.tick() => {
                    let new_files = load_local_files(&self.state.downloads_dir);
                    self.state.files = new_files;
                    // Background-index a few files per tick
                    self.index_file_metadata_chunk(8);
                    // Re-sync the file list component.
                    self.file_list.sync_files(&self.state);
                    needs_redraw = true;
                }

                _ = ui_tick.tick() => {
                    let tick_actions: Vec<Action> = {
                        let s = &self.state;
                        let mut all = Vec::new();
                        all.extend(self.station_list.tick(s));
                        all.extend(self.file_list.tick(s));
                        all.extend(self.icy_ticker.tick(s));
                        all.extend(self.songs_ticker.tick(s));
                        all.extend(self.nts_panel_ch1.tick(s));
                        all.extend(self.nts_panel_ch2.tick(s));
                        all.extend(self.file_meta.tick(s));
                        all.extend(self.log_panel.tick(s));
                        all.extend(self.help_overlay.tick(s));
                        all
                    };
                    for action in tick_actions {
                        self.dispatch(action).await;
                    }
                    needs_redraw = true;
                }

                _ = nts_refresh.tick() => {
                    let nts_tx = tx.clone();
                    tokio::spawn(async move {
                        for ch_idx in 0usize..2 {
                            match fetch_nts_channel(ch_idx).await {
                                Ok(ch) => {
                                    let _ = nts_tx.send(AppMessage::NtsUpdated(ch_idx, ch)).await;
                                }
                                Err(e) => {
                                    warn!("[nts] ch{} error: {}", ch_idx + 1, e);
                                    let _ = nts_tx.send(AppMessage::NtsError(ch_idx, e.to_string())).await;
                                }
                            }
                        }
                    });
                }

                _ = auto_poll_refresh.tick() => {
                    if self.auto_polling_enabled {
                        if self.auto_poll_in_flight {
                            debug!("[poll] interval tick — cycle #{} still in flight, skipping", self.auto_poll_cycle_id);
                        }
                        self.spawn_passive_poll_task(tx.clone(), "interval");
                    }
                }

                _ = toast_tick.tick() => {
                    self.toast.tick();
                    // Tick intents (checks for timeouts) and propagate hints
                    self.intent_pause.tick();
                    self.intent_volume.tick();
                    self.intent_station.tick();
                    self.state.pause_hint = self.intent_pause.render_state();
                    self.state.volume_hint = self.intent_volume.render_state();
                    self.state.station_hint = self.intent_station.render_state();
                    needs_redraw = true;
                }

                _ = log_refresh.tick() => {
                    if self.wm.show_log_panel {
                        self.reload_tui_log();
                        needs_redraw = true;
                    }
                }

                _ = meter_tick.tick() => {
                    needs_redraw = self.handle_message(AppMessage::MeterTick).await;
                }
            }

            if self.should_quit {
                break;
            }
        }

        // ── Teardown ──────────────────────────────────────────────────────────
        self.save_ui_session_state();
        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        Ok(())
    }

    // ── Audio tracker helper ──────────────────────────────────────────────────

    /// Update audio_level, peak_level, meter_mean_db, meter_spread_db from a
    /// fresh RMS dBFS measurement. Called from MeterTick (streams via jitter
    /// buffer) and AudioLevel (local files via lavfi).
    fn update_audio_trackers(&mut self, rms_db: f32) {
        let now = std::time::Instant::now();
        let elapsed = now
            .duration_since(self.state.peak_last_update)
            .as_secs_f32()
            .min(0.5);

        self.state.audio_level = rms_db;
        self.state.last_audio_update = now;
        self.state.peak_last_update = now;

        // VU body ballistics: fast attack + medium release (DAW-like feel).
        let attack = (1.0 - (-elapsed / VU_ATTACK_TAU_SECS).exp()).min(0.85);
        let release = (1.0 - (-elapsed / VU_RELEASE_TAU_SECS).exp()).min(0.45);
        if rms_db > self.state.vu_level {
            self.state.vu_level += attack * (rms_db - self.state.vu_level);
        } else {
            self.state.vu_level += release * (rms_db - self.state.vu_level);
        }

        // Peak marker: short hold, only large rises refresh the full hold window.
        if rms_db > self.state.peak_level {
            let rise = rms_db - self.state.peak_level;
            self.state.peak_level = rms_db;
            let hold_ms = if rise >= PEAK_HOLD_RESET_DB {
                PEAK_MAJOR_HOLD_MS
            } else {
                PEAK_MINOR_HOLD_MS
            };
            self.state.peak_hold_until = now + std::time::Duration::from_millis(hold_ms);
        }

        // Mean EMA τ ≈ 4 s
        let alpha_mean = (1.0 - (-elapsed / 4.0_f32).exp()).min(0.15);
        self.state.meter_mean_db += alpha_mean * (rms_db - self.state.meter_mean_db);

        // Spread EMA τ ≈ 8 s, minimum 2 dB
        let deviation = (rms_db - self.state.meter_mean_db).abs();
        let alpha_spread = (1.0 - (-elapsed / 8.0_f32).exp()).min(0.08);
        self.state.meter_spread_db += alpha_spread * (deviation - self.state.meter_spread_db);
        self.state.meter_spread_db = self.state.meter_spread_db.max(2.0);
    }

    // ── Message handler ───────────────────────────────────────────────────────

    /// Returns `true` if the message requires a redraw, `false` if it only
    /// updates internal data that will be consumed on the next scheduled frame
    /// (e.g. PCM ring buffer accumulation).
    async fn handle_message(&mut self, msg: AppMessage) -> bool {
        match msg {
            AppMessage::Event(ev) => match ev {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Release {
                        return true;
                    }
                    let actions = self.handle_key(key);
                    for a in actions {
                        self.dispatch(a).await;
                    }
                    self.save_ui_session_state();
                }
                Event::Mouse(mouse) => {
                    let actions = self.handle_mouse(mouse);
                    for a in actions {
                        self.dispatch(a).await;
                    }
                }
                Event::Resize(w, h) => {
                    self.dispatch(Action::Resize(w, h)).await;
                }
                _ => {}
            },

            AppMessage::StateUpdated(daemon_state) => {
                self.on_state_updated(daemon_state).await;
            }

            AppMessage::IcyUpdated(title) => {
                self.on_icy_updated(title).await;
            }

            AppMessage::Log(msg) => {
                self.push_log(msg);
            }

            AppMessage::NtsUpdated(ch, data) => {
                // Log only when the current show title changes (one line per channel).
                let prev_title = if ch == 0 {
                    self.state
                        .nts_ch1
                        .as_ref()
                        .map(|c| c.now.broadcast_title.as_str())
                } else {
                    self.state
                        .nts_ch2
                        .as_ref()
                        .map(|c| c.now.broadcast_title.as_str())
                };
                if prev_title != Some(data.now.broadcast_title.as_str()) {
                    debug!("[nts] ch{}: {:?}", ch + 1, data.now.broadcast_title);
                }

                // Update station city from NTS show location
                let station_name = if ch == 0 { "NTS 1" } else { "NTS 2" };
                let loc = data.now.location_long.clone();
                if !loc.is_empty() {
                    if let Some(s) = self
                        .state
                        .daemon_state
                        .stations
                        .iter_mut()
                        .find(|s| s.name == station_name)
                    {
                        if s.city != loc {
                            s.city = loc;
                        }
                    }
                }
                if ch == 0 {
                    self.state.nts_ch1 = Some(data);
                    self.state.nts_ch1_error = None;
                } else {
                    self.state.nts_ch2 = Some(data);
                    self.state.nts_ch2_error = None;
                }
            }

            AppMessage::NtsError(ch, msg) => {
                let ch_label = if ch == 0 { "NTS 1" } else { "NTS 2" };
                self.toast
                    .warning(format!("{} fetch error: {}", ch_label, msg));
                if ch == 0 {
                    self.state.nts_ch1_error = Some(msg);
                } else {
                    self.state.nts_ch2_error = Some(msg);
                }
            }

            AppMessage::PassivePollOutcome { cycle_id, outcome } => {
                if cycle_id != self.auto_poll_cycle_id {
                    debug!(
                        "[poll] ignoring stale outcome cycle={} active={} station={}",
                        cycle_id, self.auto_poll_cycle_id, outcome.station_name
                    );
                    return false;
                }

                self.auto_poll_cycle_seen += 1;

                if let Some(err) = outcome.error {
                    self.auto_poll_cycle_errors += 1;
                    warn!(
                        "[poll] {} resolver={} error={} ",
                        outcome.station_name, outcome.resolver, err
                    );
                    return false;
                }

                let active_station_name = self
                    .state
                    .daemon_state
                    .current_station
                    .and_then(|i| self.state.daemon_state.stations.get(i))
                    .map(|s| s.name.clone());
                if active_station_name.as_deref() == Some(outcome.station_name.as_str()) {
                    if let Some(active_icy) = self.state.daemon_state.icy_title.clone() {
                        let trimmed = active_icy.trim().to_string();
                        if !trimmed.is_empty() {
                            self.state
                                .station_poll_titles
                                .insert(outcome.station_name.clone(), trimmed);
                            self.auto_poll_cycle_unchanged += 1;
                            debug!(
                                "[poll] {} resolver={} ignored (active station has fresher ICY)",
                                outcome.station_name, outcome.resolver
                            );
                            return false;
                        }
                    }
                }

                let before = self
                    .state
                    .station_poll_titles
                    .get(&outcome.station_name)
                    .cloned();

                match outcome.show {
                    Some(show) => {
                        self.state
                            .station_poll_titles
                            .insert(outcome.station_name.clone(), show);
                    }
                    None => {
                        // Keep last-known non-NTS ICY title when a probe returns
                        // no metadata for this cycle (common for flaky/mixed
                        // endpoints). This avoids list entries disappearing and
                        // reappearing between cycles.
                        if outcome.resolver != "icy-probe" {
                            self.state.station_poll_titles.remove(&outcome.station_name);
                        }
                    }
                }

                let after = self
                    .state
                    .station_poll_titles
                    .get(&outcome.station_name)
                    .cloned();

                if before != after {
                    self.auto_poll_cycle_changed += 1;
                    info!(
                        "[poll] {} resolver={} changed: {:?} -> {:?}",
                        outcome.station_name, outcome.resolver, before, after
                    );
                    return true;
                }

                self.auto_poll_cycle_unchanged += 1;
                return false;
            }

            AppMessage::PassivePollCycleDone {
                cycle_id,
                total,
                elapsed_ms,
            } => {
                if cycle_id != self.auto_poll_cycle_id {
                    debug!(
                        "[poll] ignoring stale cycle done cycle={} active={}",
                        cycle_id, self.auto_poll_cycle_id
                    );
                    return false;
                }

                self.auto_poll_in_flight = false;
                let missing = (self.auto_poll_cycle_total as isize) - (self.auto_poll_cycle_seen as isize);
                if missing > 0 {
                    warn!(
                        "[poll] cycle #{} complete in {}ms — MISSING {} outcomes (targets={} seen={}, changed={}, unchanged={}, errors={})",
                        cycle_id,
                        elapsed_ms,
                        missing,
                        self.auto_poll_cycle_total,
                        self.auto_poll_cycle_seen,
                        self.auto_poll_cycle_changed,
                        self.auto_poll_cycle_unchanged,
                        self.auto_poll_cycle_errors,
                    );
                } else {
                    info!(
                        "[poll] cycle #{} complete in {}ms (targets={} seen={}, changed={}, unchanged={}, errors={})",
                        cycle_id,
                        elapsed_ms,
                        self.auto_poll_cycle_total,
                        self.auto_poll_cycle_seen,
                        self.auto_poll_cycle_changed,
                        self.auto_poll_cycle_unchanged,
                        self.auto_poll_cycle_errors,
                    );
                }
                return false;
            }

            AppMessage::RecognitionStarted(result) => {
                info!(
                    "[app] Recognition started job_id={} station={:?}",
                    result.job_id, result.station
                );
                // Add placeholder row to in-memory history immediately
                self.state.songs_history.push(result.clone());
                if self.state.songs_history.len() > 500 {
                    self.state.songs_history.remove(0);
                }
                // Write initial row to VDS file
                let vds_path = self.songs_vds_path.clone();
                tokio::spawn(async move {
                    if let Err(e) = append_to_vds(&vds_path, &result).await {
                        warn!("[vds] Initial write error: {}", e);
                    }
                });
            }

            AppMessage::RecognitionPatch(job_id, patch) => {
                info!("[app] Recognition patch job_id={}", job_id);
                // Update in-memory history
                if let Some(entry) = self
                    .state
                    .songs_history
                    .iter_mut()
                    .rev()
                    .find(|e| e.job_id == job_id)
                {
                    if let Some(v) = &patch.icy_info {
                        entry.icy_info = Some(v.clone());
                    }
                    if let Some(v) = &patch.nts_show {
                        entry.nts_show = Some(v.clone());
                    }
                    if let Some(v) = &patch.nts_tag {
                        entry.nts_tag = Some(v.clone());
                    }
                    if let Some(v) = &patch.nts_url {
                        entry.nts_url = Some(v.clone());
                    }
                    if let Some(v) = &patch.vibra_rec {
                        entry.vibra_rec = Some(v.clone());
                    }
                }
                // Patch VDS file on disk
                let vds_path = self.songs_vds_path.clone();
                let job_id_owned = job_id.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        radio_proto::songs::patch_vds_by_job_id(&vds_path, &job_id_owned, patch)
                            .await
                    {
                        warn!("[vds] Patch error: {}", e);
                    }
                });
            }

            AppMessage::RecognitionComplete(job_id, rec_display) => {
                info!(
                    "[app] Recognition complete job_id={} display={:?}",
                    job_id, rec_display
                );
                self.recognize_in_flight = false;
                self.toast.resolve_spinner(
                    crate::widgets::toast::Severity::Success,
                    format!("identified: {}", rec_display),
                    std::time::Duration::from_secs(5),
                );
                // Schedule next queued job after a 1-second safety gap.
                if !self.recognize_queue.is_empty() {
                    if let Some(tx) = self.recognition_tx.clone() {
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            let _ = tx.send(AppMessage::RecognitionQueueNext).await;
                        });
                    }
                }
            }

            AppMessage::RecognitionNoMatch => {
                info!("[app] Recognition: no match from vibra");
                self.recognize_in_flight = false;
                self.toast.resolve_spinner(
                    crate::widgets::toast::Severity::Warning,
                    "no match",
                    std::time::Duration::from_secs(3),
                );
                // Schedule next queued job after a 1-second safety gap.
                if !self.recognize_queue.is_empty() {
                    if let Some(tx) = self.recognition_tx.clone() {
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            let _ = tx.send(AppMessage::RecognitionQueueNext).await;
                        });
                    }
                }
            }
            AppMessage::RecognitionQueueNext => {
                // Pop the next queued recognition job and start it.
                if let Some((station_name, stream_url, icy_title, nts_ch, nts_mixtape_url)) =
                    self.recognize_queue.pop_front()
                {
                    info!(
                        "[app] Starting next queued recognition ({} remaining)",
                        self.recognize_queue.len()
                    );
                    self.recognize_in_flight = true;
                    self.spawn_recognition_job(
                        station_name,
                        stream_url,
                        icy_title,
                        nts_ch,
                        nts_mixtape_url,
                    );
                    self.toast.spinner("identifying…");
                }
            }
            AppMessage::AudioLevel(rms_db) => {
                // Keep mpv-lavfi RMS for debug bulbs on all sources.
                self.state.mpv_audio_level = rms_db;
                // Main VU/scope path remains PCM for stations; files still use lavfi.
                if self.state.daemon_state.current_file.is_some()
                    || self.state.daemon_state.current_station.is_none()
                {
                    self.update_audio_trackers(rms_db);
                }
                return false;
            }

            AppMessage::PcmChunk(chunk) => {
                // Station PCM arrives in bursts; stage it in a jitter buffer and
                // consume it on MeterTick at steady cadence.
                for &s in chunk.iter() {
                    self.state.pcm_pending.push_back(s);
                }
                if self.state.pcm_pending.len() > PCM_JITTER_MAX {
                    let keep = (PCM_JITTER_TARGET + STREAM_FRAME_SAMPLES * 8)
                        .min(self.state.pcm_pending.len());
                    let drop_n = self.state.pcm_pending.len().saturating_sub(keep);
                    for _ in 0..drop_n {
                        self.state.pcm_pending.pop_front();
                    }
                }
                return false;
            }

            AppMessage::MeterTick => {
                // Smooth station scope/VU by consuming PCM at fixed 50 Hz cadence.
                if self.state.daemon_state.current_station.is_some()
                    && self.state.daemon_state.current_file.is_none()
                {
                    if !self.state.pcm_pending_started
                        && self.state.pcm_pending.len() >= PCM_JITTER_TARGET
                    {
                        self.state.pcm_pending_started = true;
                    } else if self.state.pcm_pending_started
                        && self.state.pcm_pending.len() < PCM_JITTER_STOP
                    {
                        self.state.pcm_pending_started = false;
                    }

                    if self.state.pcm_pending_started {
                        let available = self.state.pcm_pending.len();
                        let mut take = STREAM_FRAME_SAMPLES;
                        if available > PCM_JITTER_TARGET + STREAM_FRAME_SAMPLES {
                            let catch_up = (available - PCM_JITTER_TARGET) / 2;
                            take = (STREAM_FRAME_SAMPLES + catch_up).min(STREAM_FRAME_SAMPLES * 5);
                        }
                        let mut consumed = 0usize;
                        let mut sum_sq = 0.0_f64;
                        let mut hold = *self.state.pcm_ring.back().unwrap_or(&0.0);

                        for i in 0..take {
                            if let Some(s) = self.state.pcm_pending.pop_front() {
                                hold = s;
                                consumed += 1;
                                let sf = s as f64;
                                sum_sq += sf * sf;
                                self.state.pcm_ring.push_back(s);
                                continue;
                            }
                            if i < STREAM_FRAME_SAMPLES {
                                // Short under-runs: hold last sample to keep cadence smooth.
                                let sf = hold as f64;
                                sum_sq += sf * sf;
                                self.state.pcm_ring.push_back(hold);
                            } else {
                                break;
                            }
                        }
                        while self.state.pcm_ring.len() > PCM_RING_MAX {
                            self.state.pcm_ring.pop_front();
                        }

                        let rms_n = STREAM_FRAME_SAMPLES.max(consumed) as f64;
                        if rms_n > 0.0 {
                            let rms = (sum_sq / rms_n).sqrt();
                            let rms_db = if rms < 1e-10 {
                                -90.0_f32
                            } else {
                                (20.0 * rms.log10()) as f32
                            };
                            self.update_audio_trackers(rms_db);
                        }
                    }
                } else {
                    self.state.pcm_pending.clear();
                    self.state.pcm_pending_started = false;
                }

                let now = std::time::Instant::now();
                let elapsed = now
                    .duration_since(self.state.peak_last_update)
                    .as_secs_f32()
                    .min(0.5);

                // Peak release starts after hold timer and drops quickly toward body.
                if now >= self.state.peak_hold_until && self.state.peak_level > -90.0 {
                    let target = self.state.vu_level.max(-90.0);
                    let prev_peak = self.state.peak_level;
                    let release = (1.0 - (-elapsed / PEAK_RELEASE_TAU_SECS).exp()).min(0.95);
                    self.state.peak_level += release * (target - self.state.peak_level);
                    let forced_max = (prev_peak - elapsed * PEAK_FALL_DB_PER_SEC).max(target);
                    if self.state.peak_level > forced_max {
                        self.state.peak_level = forced_max;
                    }
                }

                // If silent for >200 ms, fade levels to floor.
                let audio_age = now
                    .duration_since(self.state.last_audio_update)
                    .as_secs_f32();
                if audio_age > 0.2 && self.state.audio_level > -90.0 {
                    self.state.audio_level = (self.state.audio_level - elapsed * 20.0).max(-90.0);
                }
                if audio_age > 0.2 && self.state.vu_level > -90.0 {
                    self.state.vu_level = (self.state.vu_level - elapsed * 20.0).max(-90.0);
                }
                if audio_age > 2.0 {
                    // Relax spread toward 4 dB (typical measured steady-state for
                    // NTS/compressed streams) rather than the old 6 dB default.
                    self.state.meter_spread_db +=
                        elapsed * 2.0 * (4.0 - self.state.meter_spread_db).signum();
                    self.state.meter_spread_db = self.state.meter_spread_db.max(2.0);
                }

                self.state.peak_last_update = now;
            }

            AppMessage::DownloadComplete { url, result } => {
                self.download_in_flight = false;
                match result {
                    Ok(_) => {
                        self.state
                            .download_statuses
                            .insert(url, DownloadStatus::Downloaded);
                        self.toast.resolve_spinner(
                            Severity::Success,
                            "download complete".to_string(),
                            std::time::Duration::from_secs(5),
                        );
                    }
                    Err(e) => {
                        self.state
                            .download_statuses
                            .insert(url, DownloadStatus::Failed(e.clone()));
                        self.toast.resolve_spinner(
                            Severity::Error,
                            format!("download failed: {}", e),
                            std::time::Duration::from_secs(5),
                        );
                    }
                }
                // Start next queued download immediately.
                if let Some((next_url, next_display)) = self.download_queue.pop_front() {
                    self.start_download(next_url, next_display);
                }
            }
        }
        true
    }

    // ── Daemon state update ───────────────────────────────────────────────────

    async fn on_state_updated(&mut self, new_state: DaemonState) {
        let was_empty = self.state.daemon_state.stations.is_empty();
        let prev_station = self.state.daemon_state.current_station;
        let prev_file = self.state.daemon_state.current_file.clone();

        // Preserve NTS city overrides
        let nts1_city = self
            .state
            .daemon_state
            .stations
            .iter()
            .find(|s| s.name == "NTS 1")
            .map(|s| s.city.clone());
        let nts2_city = self
            .state
            .daemon_state
            .stations
            .iter()
            .find(|s| s.name == "NTS 2")
            .map(|s| s.city.clone());

        self.state.daemon_state = new_state;

        // Drop stale passive-poll labels for stations no longer present.
        let station_names: std::collections::HashSet<String> = self
            .state
            .daemon_state
            .stations
            .iter()
            .map(|s| s.name.clone())
            .collect();
        self.state
            .station_poll_titles
            .retain(|name, _| station_names.contains(name));

        // Keep station-list annotation in sync with daemon ICY source for the
        // currently playing station (polling is not the only source of truth).
        if let Some(st_name) = self
            .state
            .daemon_state
            .current_station
            .and_then(|i| self.state.daemon_state.stations.get(i))
            .map(|s| s.name.clone())
        {
            if let Some(icy) = self.state.daemon_state.icy_title.clone() {
                let trimmed = icy.trim().to_string();
                if !trimmed.is_empty() {
                    self.state.station_poll_titles.insert(st_name, trimmed);
                }
            }
        }

        let source_changed = self.state.daemon_state.current_station != prev_station
            || self.state.daemon_state.current_file != prev_file;
        if source_changed {
            self.state.pcm_ring.clear();
            self.state.pcm_pending.clear();
            self.state.pcm_pending_started = false;
        }

        // Clear last_known_icy when the station changes — the new station's
        // ICY title will arrive via IcyUpdated once the stream connects.
        if self.state.daemon_state.current_station != prev_station {
            self.last_known_icy = None;
            self.state.last_known_icy = None;
        }

        // ── mpv health transition toasts ──────────────────────────────────────
        let new_health = self.state.daemon_state.mpv_health.clone();
        if new_health != self.prev_mpv_health {
            match &new_health {
                MpvHealth::Dead => {
                    self.toast.error("mpv process died");
                }
                MpvHealth::Restarting => {
                    self.toast.warning("mpv restarting...");
                }
                MpvHealth::Running if self.prev_mpv_health != MpvHealth::Absent => {
                    // Only toast "recovered" if we came from a bad state
                    if self.prev_mpv_health.is_unhealthy() {
                        self.toast.success("mpv recovered");
                    }
                }
                MpvHealth::Degraded(reason) => {
                    self.toast.warning(format!("mpv degraded: {}", reason));
                }
                _ => {}
            }
            self.prev_mpv_health = new_health;
        }

        // ── Intent confirmation ───────────────────────────────────────────────
        self.intent_pause
            .on_confirmed(self.state.daemon_state.is_playing);
        self.intent_volume
            .on_confirmed(self.state.daemon_state.volume);
        // For station: any station change (including Next/Prev/Random) confirms
        if self.intent_station.is_pending() || self.intent_station.is_timed_out() {
            self.intent_station
                .on_confirmed(self.state.daemon_state.current_station);
        } else {
            self.intent_station
                .on_confirmed(self.state.daemon_state.current_station);
        }
        // Propagate render hints into AppState so components can read them
        self.state.pause_hint = self.intent_pause.render_state();
        self.state.volume_hint = self.intent_volume.render_state();
        self.state.station_hint = self.intent_station.render_state();

        if let Some(city) = nts1_city {
            if let Some(s) = self
                .state
                .daemon_state
                .stations
                .iter_mut()
                .find(|s| s.name == "NTS 1")
            {
                s.city = city;
            }
        }
        if let Some(city) = nts2_city {
            if let Some(s) = self
                .state
                .daemon_state
                .stations
                .iter_mut()
                .find(|s| s.name == "NTS 2")
            {
                s.city = city;
            }
        }

        let now_ts = chrono::Local::now().timestamp();

        // Track recently-played stations
        if let Some(i) = self.state.daemon_state.current_station {
            if let Some(st) = self.state.daemon_state.stations.get(i) {
                self.last_station_name = Some(st.name.clone());
                self.state.recent_station.insert(st.name.clone(), now_ts);
            }
        }

        // Track recently-played files and positions
        if let Some(path) = self.state.daemon_state.current_file.clone() {
            self.last_file_path = Some(path.clone());
            self.state.recent_file.insert(path.clone(), now_ts);
            if let Some(pos) = self.state.daemon_state.time_pos_secs {
                self.last_file_pos = pos.max(0.0);
                self.state.file_positions.insert(path, self.last_file_pos);
            }
        }

        if self.state.daemon_state.volume > 0.001 {
            self.state.last_nonzero_volume = self.state.daemon_state.volume;
        }

        // Feed stations into the station_list component's ScrollableList.
        // This must happen whenever daemon_state changes.
        self.station_list.sync_stations(&self.state);

        if self.auto_polling_enabled && was_empty && !self.state.daemon_state.stations.is_empty() {
            if let Some(tx) = self.recognition_tx.clone() {
                self.spawn_passive_poll_task(tx, "stations-loaded");
            }
        }

        if !self.state.daemon_state.stations.is_empty() {
            // Restore selected station from session
            if let Some(name) = self.pending_station_restore.take() {
                if let Some(idx) = self
                    .state
                    .daemon_state
                    .stations
                    .iter()
                    .position(|s| s.name == name)
                {
                    self.station_list.select_by_station_idx(idx);
                }
            }

            // Jump to currently playing station on initial load
            if was_empty && !self.initial_loaded {
                if let Some(idx) = self.state.daemon_state.current_station {
                    self.station_list.select_by_station_idx(idx);
                }
                self.initial_loaded = true;
            }

            // After any selection restore / initial jump, sync the NTS hover channel
            // so the overlay shows immediately if cursor lands on NTS 1/2.
            self.sync_nts_hover();

            // Track jump_from_station (for shuffle/next/prev)
            if let Some(from) = self.jump_from_station {
                if self.state.daemon_state.current_station != from {
                    if let Some(idx) = self.state.daemon_state.current_station {
                        self.station_list.select_by_station_idx(idx);
                    }
                    self.jump_from_station = None;
                }
            } else if self.state.daemon_state.current_station != prev_station {
                // Daemon changed current_station independently (e.g. error fallback) —
                // sync the highlight so the UI reflects the actual playing station.
                if let Some(idx) = self.state.daemon_state.current_station {
                    self.station_list.select_by_station_idx(idx);
                }
            }

            // Auto-show NTS panel when switching to/from NTS 1/2
            if self.state.daemon_state.current_station != prev_station {
                let name = self
                    .state
                    .daemon_state
                    .current_station
                    .and_then(|i| self.state.daemon_state.stations.get(i))
                    .map(|s| s.name.as_str());
                // If we were showing an NTS right-pane and switched away, revert to tickers
                if matches!(name, Some("NTS 1") | Some("NTS 2")) {
                    // don't auto-switch — user controls right pane with ! and @
                } else if self.wm.workspace == Workspace::Radio
                    && matches!(self.wm.radio_right_pane, RightPane::Nts1 | RightPane::Nts2)
                {
                    self.wm.radio_right_pane = RightPane::Tickers;
                    self.wm.rebuild_focus_ring();
                }
            }
        }

        // Auto-resume last file if idle after reconnect
        if self.state.daemon_state.current_station.is_none()
            && self.state.daemon_state.current_file.is_none()
            && !self.state.daemon_state.is_playing
        {
            if let Some((path, pos)) = self.pending_resume_file.take() {
                self.wm.set_workspace(Workspace::Files);
                self.state.workspace = Workspace::Files;
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
                recent_station: self.state.recent_station.clone(),
                recent_file: self.state.recent_file.clone(),
            },
        );
        let _ = save_file_positions(&self.file_positions_path, &self.state.file_positions);
    }

    // ── ICY update ────────────────────────────────────────────────────────────

    async fn on_icy_updated(&mut self, title: Option<String>) {
        if let Some(ref t) = title {
            let last_raw = self
                .state
                .icy_history
                .last()
                .map(|e| e.raw.as_str())
                .unwrap_or("");
            if last_raw != t.as_str() {
                let now = chrono::Local::now();
                let ts_str = format_timestamp(now);
                let display = format!("{}  {}", ts_str, t);
                let station = self
                    .state
                    .daemon_state
                    .current_station
                    .and_then(|i| self.state.daemon_state.stations.get(i))
                    .map(|s| s.name.clone());

                // Update recent for the station
                if let Some(st_name) = station.as_deref() {
                    self.state
                        .recent_station
                        .insert(st_name.to_string(), chrono::Local::now().timestamp());
                    let _ = save_recent_state(
                        &self.recent_path,
                        &RecentState {
                            recent_station: self.state.recent_station.clone(),
                            recent_file: self.state.recent_file.clone(),
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
                self.state.icy_history.push(entry);
                if self.state.icy_history.len() > 200 {
                    self.state.icy_history.remove(0);
                }

                // Persist to icyticker.log
                let log_line = format!("{}\n", display);
                if let Some(parent) = self.state.icy_log_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                if let Ok(mut f) = tokio::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.state.icy_log_path)
                    .await
                {
                    let _ = f.write_all(log_line.as_bytes()).await;
                }
            }
        }
        self.state.daemon_state.icy_title = title.clone();

        // Keep last_known_icy in sync: update on arrival, clear on None.
        // This field is never overwritten by StateUpdated so it survives
        // transient None states from the daemon.
        match title {
            Some(ref t) => {
                let station = self
                    .state
                    .daemon_state
                    .current_station
                    .and_then(|i| self.state.daemon_state.stations.get(i))
                    .map(|s| s.name.clone());
                if let Some(st) = station {
                    let entry = (st.clone(), t.clone());
                    self.last_known_icy = Some(entry.clone());
                    self.state.last_known_icy = Some(entry);
                    self.state.station_poll_titles.insert(st, t.clone());
                }
            }
            None => {
                if let Some(st) = self
                    .state
                    .daemon_state
                    .current_station
                    .and_then(|i| self.state.daemon_state.stations.get(i))
                    .map(|s| s.name.clone())
                {
                    self.state.station_poll_titles.remove(&st);
                }
                self.last_known_icy = None;
                self.state.last_known_icy = None;
            }
        }
    }

    // ── Key handling ──────────────────────────────────────────────────────────

    fn handle_key(&mut self, key: KeyEvent) -> Vec<Action> {
        // Global keys — always active regardless of focus/mode
        match key.code {
            KeyCode::Char('q') if key.modifiers == KeyModifiers::NONE => {
                if self.state.input_mode == InputMode::Normal {
                    return vec![Action::Quit];
                }
            }
            KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                return vec![Action::Quit];
            }
            KeyCode::Char('?') if self.state.input_mode == InputMode::Normal => {
                return vec![Action::ToggleHelp];
            }
            KeyCode::Char('L') if self.state.input_mode == InputMode::Normal => {
                return vec![Action::ToggleLogs];
            }
            _ => {}
        }

        // Help overlay captures all keys when visible
        if self.wm.show_help {
            let actions = self.help_overlay.handle_key(key, &self.state);
            if !actions.is_empty() {
                return actions;
            }
            // Any other key closes the overlay
            return vec![Action::ToggleHelp];
        }

        // Tab / Shift-Tab always cycle focus (even in filter mode, it closes filter first)
        match key.code {
            KeyCode::Tab => {
                if self.state.input_mode == InputMode::Filter {
                    return vec![Action::CloseFilter, Action::FocusNext];
                }
                return vec![Action::FocusNext];
            }
            KeyCode::BackTab => {
                if self.state.input_mode == InputMode::Filter {
                    return vec![Action::CloseFilter, Action::FocusPrev];
                }
                return vec![Action::FocusPrev];
            }
            _ => {}
        }

        // Global playback keys (Normal mode only)
        if self.state.input_mode == InputMode::Normal {
            match key.code {
                KeyCode::Char(' ') => return vec![Action::TogglePause],
                KeyCode::Char('n') => return vec![Action::Next],
                KeyCode::Char('p') => return vec![Action::ToggleAutoPolling],
                KeyCode::Char('P') => return vec![Action::Prev],
                KeyCode::Char('r') => return vec![Action::Random],
                KeyCode::Char('R') => return vec![Action::RandomBack],
                KeyCode::Char('m') => return vec![Action::Mute],
                // Volume: arrow keys or +/-
                KeyCode::Right | KeyCode::Char('+') | KeyCode::Char('=') => {
                    let new_vol = (self.state.daemon_state.volume + 0.05).min(1.0);
                    return vec![Action::Volume(new_vol)];
                }
                KeyCode::Left | KeyCode::Char('-') => {
                    let new_vol = (self.state.daemon_state.volume - 0.05).max(0.0);
                    return vec![Action::Volume(new_vol)];
                }
                // Seek: comma/period = 30s, shift+comma/shift+period = 5min
                KeyCode::Char(',') => {
                    let seek_secs = if key.modifiers.contains(KeyModifiers::SHIFT) {
                        -300.0
                    } else {
                        -30.0
                    };
                    return vec![Action::SeekRelative(seek_secs)];
                }
                KeyCode::Char('.') => {
                    let seek_secs = if key.modifiers.contains(KeyModifiers::SHIFT) {
                        300.0
                    } else {
                        30.0
                    };
                    return vec![Action::SeekRelative(seek_secs)];
                }
                KeyCode::Char('f') => {
                    // Switch workspace
                    return vec![Action::SwitchWorkspace(match self.wm.workspace {
                        Workspace::Radio => Workspace::Files,
                        Workspace::Files => Workspace::Radio,
                    })];
                }
                KeyCode::Char('1') => {
                    self.wm.focus_nth(0);
                    return vec![];
                }
                KeyCode::Char('2') => {
                    self.wm.focus_nth(1);
                    return vec![];
                }
                KeyCode::Char('3') => {
                    self.wm.focus_nth(2);
                    return vec![];
                }
                KeyCode::Char('4') => {
                    self.wm.focus_nth(3);
                    return vec![];
                }
                KeyCode::Char('!') => return vec![Action::ToggleNts(0)],
                KeyCode::Char('@') => return vec![Action::ToggleNts(1)],
                KeyCode::Char('o') => return vec![Action::ToggleScope],
                KeyCode::Char('v') => return vec![Action::CycleVuMeterStyle],
                KeyCode::Char('_') | KeyCode::Char('|') => return vec![Action::ToggleFullWidth],
                KeyCode::Char('K') => {
                    // toggle keybinding bar
                    return vec![Action::ToggleKeys];
                }
                KeyCode::Char('J') => return vec![Action::JumpToCurrent],
                KeyCode::Char('c') => return vec![Action::ToggleCollapse],
                // Song recognition — global, works from any pane
                KeyCode::Char('i') | KeyCode::Char('I') => return vec![Action::RecognizeSong],
                _ => {}
            }
        }

        // Dispatch to the focused component
        let focused = self.wm.focused();
        let s = &self.state;
        match focused {
            Some(ComponentId::StationList) => self.station_list.handle_key(key, s),
            Some(ComponentId::FileList) => self.file_list.handle_key(key, s),
            Some(ComponentId::IcyTicker) => self.icy_ticker.handle_key(key, s),
            Some(ComponentId::SongsTicker) => self.songs_ticker.handle_key(key, s),
            Some(ComponentId::NtsPanel) => {
                // Dispatch to whichever NTS panel is visible
                match self.wm.radio_right_pane {
                    RightPane::Nts2 => self.nts_panel_ch2.handle_key(key, s),
                    _ => self.nts_panel_ch1.handle_key(key, s),
                }
            }
            Some(ComponentId::FileMeta) => self.file_meta.handle_key(key, s),
            Some(ComponentId::LogPanel) => self.log_panel.handle_key(key, s),
            Some(ComponentId::HelpOverlay) => self.help_overlay.handle_key(key, s),
            Some(ComponentId::ScopePanel) => {
                self.scope_panel.handle_key(key);
                vec![]
            }
            None => vec![],
        }
    }

    // ── Mouse handling ────────────────────────────────────────────────────────

    fn handle_mouse(&mut self, event: MouseEvent) -> Vec<Action> {
        let is_click = matches!(
            event.kind,
            MouseEventKind::Down(_) | MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        );
        if !is_click {
            return vec![];
        }

        let col = event.column;
        let row = event.row;

        // Helper: check if (col, row) is inside a Rect
        fn hit(r: Rect, col: u16, row: u16) -> bool {
            r.width > 0
                && r.height > 0
                && col >= r.x
                && col < r.x + r.width
                && row >= r.y
                && row < r.y + r.height
        }

        let areas = self.pane_areas.clone();
        let s = &self.state;

        // Determine which pane was clicked and dispatch to it.
        // Also return a FocusPane action so focus follows the click.
        macro_rules! click_pane {
            ($id:expr, $component:expr, $area:expr) => {{
                let mut actions = $component.handle_mouse(event, $area, s);
                // Prepend focus if not already focused
                if self.wm.focused() != Some($id) {
                    actions.insert(0, Action::FocusPane($id));
                }
                return actions;
            }};
        }

        // Check each pane in z-order (most specific / front first)
        // NTS hover overlay is drawn on top of station list — check it first.
        if hit(areas.nts_overlay, col, row) {
            let area = areas.nts_overlay;
            let ch = self.state.nts_hover_channel.unwrap_or(0);
            let mut actions = if ch == 1 {
                self.nts_panel_ch2.handle_mouse(event, area, s)
            } else {
                self.nts_panel_ch1.handle_mouse(event, area, s)
            };
            if self.wm.focused() != Some(ComponentId::NtsPanel) {
                actions.insert(0, Action::FocusPane(ComponentId::NtsPanel));
            }
            return actions;
        }
        if hit(areas.station_list, col, row) {
            click_pane!(
                ComponentId::StationList,
                self.station_list,
                areas.station_list
            );
        }
        if hit(areas.file_list, col, row) {
            click_pane!(ComponentId::FileList, self.file_list, areas.file_list);
        }
        if hit(areas.nts_panel, col, row) {
            let area = areas.nts_panel;
            let mut actions = match self.wm.radio_right_pane {
                RightPane::Nts2 => self.nts_panel_ch2.handle_mouse(event, area, s),
                _ => self.nts_panel_ch1.handle_mouse(event, area, s),
            };
            if self.wm.focused() != Some(ComponentId::NtsPanel) {
                actions.insert(0, Action::FocusPane(ComponentId::NtsPanel));
            }
            return actions;
        }
        if hit(areas.scope, col, row) {
            if self.wm.focused() != Some(ComponentId::ScopePanel) {
                return vec![Action::FocusPane(ComponentId::ScopePanel)];
            }
            return vec![];
        }
        if hit(areas.icy_ticker, col, row) {
            click_pane!(ComponentId::IcyTicker, self.icy_ticker, areas.icy_ticker);
        }
        if hit(areas.songs_ticker, col, row) {
            click_pane!(
                ComponentId::SongsTicker,
                self.songs_ticker,
                areas.songs_ticker
            );
        }
        if hit(areas.file_meta, col, row) {
            click_pane!(ComponentId::FileMeta, self.file_meta, areas.file_meta);
        }
        if hit(areas.log_panel, col, row) {
            click_pane!(ComponentId::LogPanel, self.log_panel, areas.log_panel);
        }

        vec![]
    }

    // ── Action dispatcher ─────────────────────────────────────────────────────

    async fn dispatch(&mut self, action: Action) {
        // Broadcast action to all components first (so they can react to e.g. stars, playback changes)
        let secondary: Vec<Action> = {
            let s = &self.state;
            let mut out = Vec::new();
            out.extend(self.station_list.on_action(&action, s));
            out.extend(self.file_list.on_action(&action, s));
            out.extend(self.icy_ticker.on_action(&action, s));
            out.extend(self.songs_ticker.on_action(&action, s));
            out.extend(self.nts_panel_ch1.on_action(&action, s));
            out.extend(self.nts_panel_ch2.on_action(&action, s));
            out.extend(self.file_meta.on_action(&action, s));
            out.extend(self.log_panel.on_action(&action, s));
            out.extend(self.help_overlay.on_action(&action, s));
            out
        };

        // Handle the action at the app level
        self.apply_action(action).await;

        // Dispatch any secondary actions (depth-limited to 1 level)
        for a in secondary {
            self.apply_action(a).await;
        }
    }

    async fn apply_action(&mut self, action: Action) {
        // Skip logging high-frequency no-op actions
        match &action {
            Action::HoverNts(None) | Action::Tick | Action::Render | Action::Noop => {}
            _ => debug!("apply_action: {:?}", action),
        }
        match action {
            // ── Playback ──────────────────────────────────────────────────────
            Action::Play(idx) => {
                self.jump_from_station = Some(self.state.daemon_state.current_station);
                self.intent_station.set_intent(Some(idx));
                self.send_cmd(Command::Play { station_idx: idx }).await;
            }
            Action::PlayFile(path) => {
                self.last_file_path = Some(path.clone());
                let pos = self.state.file_positions.get(&path).copied().unwrap_or(0.0);
                self.send_cmd(Command::PlayFileAt {
                    path,
                    start_secs: pos,
                })
                .await;
            }
            Action::PlayFileAt(path, secs) => {
                self.last_file_path = Some(path.clone());
                self.send_cmd(Command::PlayFileAt {
                    path,
                    start_secs: secs,
                })
                .await;
            }
            Action::PlayFilePaused(path, secs) => {
                self.send_cmd(Command::PlayFilePausedAt {
                    path,
                    start_secs: secs,
                })
                .await;
            }
            Action::Stop => {
                self.send_cmd(Command::Stop).await;
            }
            Action::TogglePause => {
                // Intent: flip the current is_playing state
                let currently_playing = self.state.daemon_state.is_playing;
                self.intent_pause.set_intent(!currently_playing);
                self.send_cmd(Command::TogglePause).await;
            }
            Action::Next => {
                self.jump_from_station = Some(self.state.daemon_state.current_station);
                self.intent_station.set_intent(None); // unknown target
                self.send_cmd(Command::Next).await;
            }
            Action::Prev => {
                self.jump_from_station = Some(self.state.daemon_state.current_station);
                self.intent_station.set_intent(None); // unknown target
                self.send_cmd(Command::Prev).await;
            }
            Action::Random => {
                self.jump_from_station = Some(self.state.daemon_state.current_station);
                self.intent_station.set_intent(None); // unknown target
                self.send_cmd(Command::Random).await;
            }
            Action::RandomBack => {
                if let Some(entry) = self.state.random_history.pop() {
                    let _ =
                        save_random_history(&self.random_history_path, &self.state.random_history);
                    self.send_cmd(Command::PlayFileAt {
                        path: entry.path,
                        start_secs: entry.start_secs,
                    })
                    .await;
                }
            }
            Action::Volume(v) => {
                if v > 0.001 {
                    self.state.last_nonzero_volume = v;
                }
                self.intent_volume.set_intent(v);
                self.send_cmd(Command::Volume { value: v }).await;
            }
            Action::SeekRelative(delta) => {
                self.send_cmd(Command::SeekRelative { seconds: delta })
                    .await;
            }
            Action::SeekTo(pos) => {
                self.send_cmd(Command::SeekTo { seconds: pos }).await;
            }
            Action::Mute => {
                let current = self.state.daemon_state.volume;
                let new_vol = if current < 0.01 {
                    self.state.last_nonzero_volume.max(0.1)
                } else {
                    0.0
                };
                self.send_cmd(Command::Volume { value: new_vol }).await;
            }

            // ── Navigation ────────────────────────────────────────────────────
            Action::FocusNext => {
                self.wm.focus_next();
                self.sync_input_mode();
            }
            Action::FocusPrev => {
                self.wm.focus_prev();
                self.sync_input_mode();
            }
            Action::FocusPane(id) => {
                self.wm.focus_set(id);
                self.sync_input_mode();
            }

            // ── Filter ────────────────────────────────────────────────────────
            Action::OpenFilter => {
                self.state.input_mode = InputMode::Filter;
            }
            Action::CloseFilter => {
                self.state.input_mode = InputMode::Normal;
            }

            // ── Workspace ─────────────────────────────────────────────────────
            Action::SwitchWorkspace(ws) => {
                self.wm.set_workspace(ws);
                self.state.workspace = ws;
                self.sync_input_mode();
            }
            Action::ToggleFullWidth => {
                self.wm.toggle_right_maximized();
            }
            Action::ToggleRightMaximized => {
                self.wm.toggle_right_maximized();
            }

            // ── NTS ───────────────────────────────────────────────────────────
            Action::ToggleNts(ch) => {
                if ch == 0 {
                    self.wm.toggle_nts1();
                } else {
                    self.wm.toggle_nts2();
                }
            }
            Action::HoverNts(ch) => {
                self.state.nts_hover_channel = ch;
                self.wm
                    .rebuild_focus_ring_with(self.state.nts_hover_channel);
            }

            // ── Scope ─────────────────────────────────────────────────────────
            Action::ToggleScope => {
                self.wm.toggle_scope();
            }

            // ── VU Meter ───────────────────────────────────────────────────────
            Action::CycleVuMeterStyle => {
                let new_style = self.header.cycle_meter_style();
                let style_name = match new_style {
                    crate::components::vu_meter::MeterStyle::Studio => "Studio",
                    crate::components::vu_meter::MeterStyle::Led => "LED",
                    crate::components::vu_meter::MeterStyle::Analog => "Analog",
                };
                self.toast.info(format!("VU meter: {}", style_name));
            }

            // ── Stars ─────────────────────────────────────────────────────────
            Action::SetStar(n, ctx) => match ctx {
                StarContext::Station(name) => {
                    if n == 0 {
                        self.state.station_stars.remove(&name);
                    } else {
                        self.state.station_stars.insert(name.clone(), n);
                    }
                    let _ = save_stars(
                        &self.stars_path,
                        &self.state.station_stars,
                        &self.state.file_stars,
                    );
                    if n == 0 {
                        self.toast.info(format!("unstarred {}", name));
                    } else {
                        self.toast
                            .success(format!("{} {}", "✹".repeat(n as usize), name));
                    }
                }
                StarContext::File(path) => {
                    let label = std::path::Path::new(&path)
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.clone());
                    if n == 0 {
                        self.state.file_stars.remove(&path);
                    } else {
                        self.state.file_stars.insert(path, n);
                    }
                    let _ = save_stars(
                        &self.stars_path,
                        &self.state.station_stars,
                        &self.state.file_stars,
                    );
                    if n == 0 {
                        self.toast.info(format!("unstarred {}", label));
                    } else {
                        self.toast
                            .success(format!("{} {}", "★".repeat(n as usize), label));
                    }
                }
            },
            Action::ToggleStar => {
                // Handled by the component
            }

            // ── Song recognition ──────────────────────────────────────────────
            Action::RecognizeSong => {
                info!("[app] RecognizeSong action triggered");

                let station = self
                    .state
                    .daemon_state
                    .current_station
                    .and_then(|i| self.state.daemon_state.stations.get(i))
                    .cloned();
                let station_name = station.as_ref().map(|s| s.name.clone());
                let stream_url = station.as_ref().map(|s| s.url.clone());

                // Best-effort ICY resolution — three tiers, in order of freshness:
                //
                // 1. daemon_state.icy_title  — live value from the latest StateUpdated
                //    or IcyUpdated message.  Most up-to-date but can be None if the
                //    ICY hasn't arrived yet (fresh daemon start, station just switched).
                //
                // 2. last_known_icy           — sticky field updated by IcyUpdated only,
                //    never overwritten by StateUpdated.  Survives transient None states
                //    as long as the station hasn't changed.
                //
                // 3. icy_history (session)    — most recent entry tagged to this station
                //    recorded during this session (has station tag, unlike log-loaded
                //    entries).  Covers the case where daemon dedup prevented a
                //    re-broadcast but the title is in recent history.
                let icy_title = self
                    .state
                    .daemon_state
                    .icy_title
                    .clone()
                    .or_else(|| {
                        // Tier 2: last_known_icy for the current station
                        let name = station_name.as_deref()?;
                        self.last_known_icy
                            .as_ref()
                            .filter(|(st, _)| st.as_str() == name)
                            .map(|(_, t)| t.clone())
                    })
                    .or_else(|| {
                        // Tier 3: most recent icy_history entry for this station
                        let name = station_name.as_deref()?;
                        self.state
                            .icy_history
                            .iter()
                            .rev()
                            .find(|e| e.station.as_deref() == Some(name))
                            .map(|e| e.raw.clone())
                    });

                info!(
                    "[app] Recognition context: station={:?}, icy={:?}",
                    station_name, icy_title
                );

                let nts_ch = station_name.as_deref().and_then(|n| {
                    if n.eq_ignore_ascii_case("nts 1") {
                        Some(0)
                    } else if n.eq_ignore_ascii_case("nts 2") {
                        Some(1)
                    } else {
                        None
                    }
                });

                let nts_mixtape_url = station.as_ref().and_then(|s| {
                    if !s.name.starts_with("NTS:") {
                        return None;
                    }
                    let u = s.mixtape_url.trim();
                    if u.is_empty() {
                        None
                    } else {
                        Some(u.to_string())
                    }
                });

                if station_name.is_none() && icy_title.is_none() {
                    warn!("[app] Cannot start recognition: nothing playing");
                    self.toast.warning("nothing playing — can't identify");
                } else {
                    // If the station changed, discard the old queue entirely.
                    if self.recognize_active_station.as_deref() != station_name.as_deref() {
                        self.recognize_queue.clear();
                        self.recognize_active_station = station_name.clone();
                    }

                    if self.recognize_in_flight {
                        // Deduplicate & cap at 3 queued jobs.
                        if self.recognize_queue.len() < 3 {
                            self.recognize_queue.push_back((
                                station_name,
                                stream_url,
                                icy_title,
                                nts_ch,
                                nts_mixtape_url,
                            ));
                            info!(
                                "[app] Recognition queued ({} in queue)",
                                self.recognize_queue.len()
                            );
                        } else {
                            info!("[app] Recognition queue full (3), ignoring duplicate press");
                        }
                    } else {
                        // Start immediately.
                        self.recognize_in_flight = true;
                        self.recognize_active_station = station_name.clone();
                        self.spawn_recognition_job(
                            station_name,
                            stream_url,
                            icy_title,
                            nts_ch,
                            nts_mixtape_url,
                        );
                        self.toast.spinner("identifying…");
                    }
                }
            }

            // ── UI toggles ────────────────────────────────────────────────────
            Action::ToggleLogs => {
                self.wm.show_log_panel = !self.wm.show_log_panel;
                if self.wm.show_log_panel {
                    // Load log immediately on open so it's not blank
                    self.reload_tui_log();
                    // Focus the log panel when opening
                    self.wm.focus_set(ComponentId::LogPanel);
                    self.log_panel.expanded = true;
                    self.log_panel.scroll = usize::MAX; // jump to bottom
                } else {
                    // Return focus to the main pane
                    self.wm.focus_set(ComponentId::StationList);
                    self.log_panel.expanded = false;
                }
            }
            Action::ToggleHelp => {
                self.wm.show_help = !self.wm.show_help;
            }
            Action::ToggleKeys => {
                self.wm.show_keys_bar = !self.wm.show_keys_bar;
            }
            Action::ToggleAutoPolling => {
                self.auto_polling_enabled = !self.auto_polling_enabled;
                if self.auto_polling_enabled {
                    info!(
                        "[poll] auto polling enabled (interval={}s)",
                        self.auto_poll_interval.as_secs()
                    );
                    self.toast.info(format!(
                        "auto polling: on ({}s)",
                        self.auto_poll_interval.as_secs()
                    ));
                    if let Some(tx) = self.recognition_tx.clone() {
                        self.spawn_passive_poll_task(tx, "manual-toggle");
                    }
                } else {
                    info!("[poll] auto polling disabled");
                    self.toast
                        .info("auto polling: off (press p to re-enable)".to_string());
                }
            }

            // ── Collapse ──────────────────────────────────────────────────────
            Action::ToggleCollapse => {
                if let Some(id) = self.wm.focused() {
                    self.wm.toggle_collapse(id);
                }
            }

            // ── System ────────────────────────────────────────────────────────
            Action::SendCommand(cmd) => {
                self.send_cmd(cmd).await;
            }
            Action::Quit => {
                self.should_quit = true;
            }

            // Handled at component level / no-op here
            Action::JumpToCurrent
            | Action::CycleSort
            | Action::CycleSortReverse
            | Action::SelectUp(_)
            | Action::SelectDown(_)
            | Action::SelectFirst
            | Action::SelectLast
            | Action::ScrollUp(_)
            | Action::ScrollDown(_)
            | Action::FilterChanged(_)
            | Action::ClearFilter
            | Action::Tick
            | Action::Render
            | Action::Resize(_, _)
            | Action::Noop => {}

            Action::Download => {
                // Get selected song entry and download if it has an NTS URL
                if let Some(entry) = self.get_selected_song_entry() {
                    if let Some(url) = entry.nts_url.clone() {
                        let display = entry.display().to_string();
                        if self.download_in_flight {
                            // Queue only if this URL isn't already pending.
                            if !self.download_queue.iter().any(|(u, _)| u == &url) {
                                self.download_queue.push_back((url, display));
                                info!(
                                    "[app] Download queued ({} in queue)",
                                    self.download_queue.len()
                                );
                            } else {
                                info!("[app] Download already queued, ignoring duplicate press");
                            }
                        } else {
                            self.start_download(url, display);
                        }
                    } else {
                        self.toast.error("No NTS URL available for download");
                    }
                }
            }

            Action::CopyToClipboard(text) => {
                match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text.clone())) {
                    Ok(()) => {
                        // Truncate for toast display
                        let display = if text.chars().count() > 40 {
                            format!("{}…", text.chars().take(40).collect::<String>())
                        } else {
                            text.clone()
                        };
                        self.toast.success(format!("copied: {}", display));
                    }
                    Err(e) => {
                        warn!("clipboard error: {}", e);
                        self.toast.error(format!("clipboard error: {}", e));
                    }
                }
            }
        }
    }

    // ── Drawing ───────────────────────────────────────────────────────────────

    fn draw(&mut self, frame: &mut ratatui::Frame) {
        use crate::theme::C_BG;
        use ratatui::widgets::Block;
        let area = frame.area();

        // Fill the entire terminal with the base background colour so that
        // any unstyled cells (gaps between panes) appear black rather than
        // whatever the terminal default is.
        frame.render_widget(
            Block::default().style(ratatui::style::Style::default().bg(C_BG)),
            area,
        );

        // ── Outer layout: header | body | (log) | (statusbar) ────────────────
        let header_h = 2u16;
        let status_h = if self.wm.show_keys_bar { 1u16 } else { 0 };
        let log_h = if self.wm.show_log_panel { 10u16 } else { 0 }; // 0 = fully hidden

        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(header_h),
                Constraint::Min(0),
                Constraint::Length(log_h),
                Constraint::Length(status_h),
            ])
            .split(area);

        let header_area = outer[0];
        let body_area = outer[1];
        let log_area = outer[2];
        let status_area = outer[3];

        // ── Header ────────────────────────────────────────────────────────────
        // When scope is active, split the 2-row header: left half = header info,
        // right half = oscilloscope.
        if self.wm.radio_right_pane == RightPane::Scope
            && matches!(self.wm.workspace, Workspace::Radio)
        {
            let halves = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(header_area);
            self.header.draw(frame, halves[0], false, &self.state);
            let scope_focused = self.wm.focused() == Some(ComponentId::ScopePanel);
            self.scope_panel.draw(frame, halves[1], &self.state);
            self.pane_areas.scope = halves[1];
            let _ = scope_focused; // focus highlight handled separately if needed
        } else {
            self.header.draw(frame, header_area, false, &self.state);
            self.pane_areas.scope = Rect::default();
        }

        // ── Status bar ────────────────────────────────────────────────────────
        if self.wm.show_keys_bar {
            status_bar::draw_keys_bar(
                frame,
                status_area,
                self.state.input_mode,
                self.wm.workspace,
                self.state.mpv_audio_level,
                self.auto_polling_enabled,
            );
        }

        // ── Log panel ─────────────────────────────────────────────────────────
        if self.wm.show_log_panel {
            let log_focused = self.wm.focused() == Some(ComponentId::LogPanel);
            use ratatui::widgets::Borders;
            // Expanded: omit top border (body above has its own bottom)
            self.log_panel.borders = Borders::LEFT | Borders::BOTTOM | Borders::RIGHT;
            self.log_panel
                .draw(frame, log_area, log_focused, &self.state);
            self.pane_areas.log_panel = log_area;
        } else {
            self.pane_areas.log_panel = Rect::default();
        }

        // ── Body layout depends on workspace ─────────────────────────────────
        match self.wm.workspace {
            Workspace::Radio => self.draw_radio(frame, body_area),
            Workspace::Files => self.draw_files(frame, body_area),
        }

        // ── Help overlay (on top of everything) ──────────────────────────────
        if self.wm.show_help {
            self.help_overlay.draw(frame, area, false, &self.state);
        }

        // ── Toast notifications (topmost layer) ──────────────────────────────
        self.toast.draw(frame, area);
    }

    fn draw_radio(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        use ratatui::widgets::Borders;

        let right_maximized = self.wm.radio_right_maximized;
        let has_overlay = self.state.nts_hover_channel.is_some()
            && matches!(self.wm.radio_right_pane, RightPane::Tickers);

        // Assign fixed pane number keys: StationList=1, Icy=2, Songs=3, NtsPanel=4
        self.icy_ticker.number_key = Some('2');
        self.songs_ticker.number_key = Some('3');
        self.nts_panel_ch1.number_key = Some('4');
        self.nts_panel_ch2.number_key = Some('4');

        // ── Scope mode: scope is rendered in the header; body = full-width station list ──
        if self.wm.radio_right_pane == RightPane::Scope {
            let station_focused = self.wm.focused() == Some(ComponentId::StationList);
            self.station_list.borders = Borders::empty();
            self.station_list
                .draw(frame, area, station_focused, &self.state);
            self.pane_areas.station_list = area;
            self.pane_areas.nts_panel = Rect::default();
            return;
        }

        // Duplicate number key assignments removed (already set above).

        // Split into left (station list) and right (tickers / NTS)
        let (left_pct, right_pct) = if right_maximized { (30, 70) } else { (55, 45) };

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(left_pct),
                Constraint::Percentage(right_pct),
            ])
            .split(area);

        let left_area = cols[0];
        let right_area = cols[1];

        let station_collapsed = self.wm.is_collapsed(ComponentId::StationList);
        let station_focused = self.wm.focused() == Some(ComponentId::StationList);

        if station_collapsed {
            use crate::widgets::pane_chrome::draw_collapsed_pane;
            let summary = self.station_list.collapse_summary(&self.state);
            draw_collapsed_pane(
                frame,
                left_area,
                "stations",
                summary.as_deref(),
                station_focused,
            );
            self.pane_areas.station_list = left_area;
        } else {
            // Left pane: omit right border — right pane's left border is the shared divider
            self.station_list.borders = Borders::TOP | Borders::LEFT | Borders::BOTTOM;
            self.station_list
                .draw(frame, left_area, station_focused, &self.state);
            self.pane_areas.station_list = left_area;
        }

        // Right pane
        match self.wm.radio_right_pane {
            RightPane::Tickers => {
                let icy_collapsed = self.wm.is_collapsed(ComponentId::IcyTicker);
                let songs_collapsed = self.wm.is_collapsed(ComponentId::SongsTicker);

                // Compute heights: collapsed = 1 row, expanded = equal split of remaining
                let rows = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(match (icy_collapsed, songs_collapsed) {
                        (true, true) => vec![Constraint::Length(1), Constraint::Length(1)],
                        (true, false) => vec![Constraint::Length(1), Constraint::Min(0)],
                        (false, true) => vec![Constraint::Min(0), Constraint::Length(1)],
                        (false, false) => {
                            vec![Constraint::Percentage(50), Constraint::Percentage(50)]
                        }
                    })
                    .split(right_area);

                let icy_focused = self.wm.focused() == Some(ComponentId::IcyTicker);
                let songs_focused = self.wm.focused() == Some(ComponentId::SongsTicker);

                if icy_collapsed {
                    use crate::widgets::pane_chrome::draw_collapsed_pane;
                    let summary = self.icy_ticker.collapse_summary(&self.state);
                    draw_collapsed_pane(frame, rows[0], "icy", summary.as_deref(), icy_focused);
                } else {
                    self.icy_ticker.borders = Borders::ALL;
                    self.icy_ticker
                        .draw(frame, rows[0], icy_focused, &self.state);
                }

                if songs_collapsed {
                    use crate::widgets::pane_chrome::draw_collapsed_pane;
                    let summary = self.songs_ticker.collapse_summary(&self.state);
                    draw_collapsed_pane(frame, rows[1], "songs", summary.as_deref(), songs_focused);
                } else {
                    // Songs: omit top border only if ICY is expanded above it
                    self.songs_ticker.borders = if icy_collapsed {
                        Borders::ALL
                    } else {
                        Borders::LEFT | Borders::BOTTOM | Borders::RIGHT
                    };
                    self.songs_ticker
                        .draw(frame, rows[1], songs_focused, &self.state);
                }

                self.pane_areas.icy_ticker = rows[0];
                self.pane_areas.songs_ticker = rows[1];
                self.pane_areas.nts_panel = Rect::default();
            }
            RightPane::Nts1 => {
                let nts_collapsed = self.wm.is_collapsed(ComponentId::NtsPanel);
                let nts_focused = self.wm.focused() == Some(ComponentId::NtsPanel);
                if nts_collapsed {
                    use crate::widgets::pane_chrome::draw_collapsed_pane;
                    let summary = self.nts_panel_ch1.collapse_summary(&self.state);
                    draw_collapsed_pane(
                        frame,
                        right_area,
                        "nts 1",
                        summary.as_deref(),
                        nts_focused,
                    );
                } else {
                    self.nts_panel_ch1.borders = Borders::ALL;
                    self.nts_panel_ch1
                        .draw(frame, right_area, nts_focused, &self.state);
                }
                self.pane_areas.nts_panel = right_area;
            }
            RightPane::Nts2 => {
                let nts_collapsed = self.wm.is_collapsed(ComponentId::NtsPanel);
                let nts_focused = self.wm.focused() == Some(ComponentId::NtsPanel);
                if nts_collapsed {
                    use crate::widgets::pane_chrome::draw_collapsed_pane;
                    let summary = self.nts_panel_ch2.collapse_summary(&self.state);
                    draw_collapsed_pane(
                        frame,
                        right_area,
                        "nts 2",
                        summary.as_deref(),
                        nts_focused,
                    );
                } else {
                    self.nts_panel_ch2.borders = Borders::ALL;
                    self.nts_panel_ch2
                        .draw(frame, right_area, nts_focused, &self.state);
                }
                self.pane_areas.nts_panel = right_area;
            }
            RightPane::Scope => {
                // Handled by early-return scope layout above; unreachable here.
                unreachable!("RightPane::Scope should have returned early")
            }
        }

        // ── NTS hover overlay ─────────────────────────────────────────────────
        // When the cursor is on an NTS row in the station list we draw a compact
        // NTS info panel as a floating overlay covering the bottom of the left
        // (station list) pane, sized to fit its content exactly.
        if let Some(hover_ch) = self.state.nts_hover_channel {
            // Only show overlay when the full NTS right-pane is NOT already open.
            if matches!(self.wm.radio_right_pane, RightPane::Tickers) {
                let base = self.pane_areas.station_list;
                if base.height > 4 {
                    let panel = if hover_ch == 0 {
                        &mut self.nts_panel_ch1
                    } else {
                        &mut self.nts_panel_ch2
                    };

                    // Compute content height: border(2) + inner rows needed
                    let overlay_width = base.width;
                    let content_rows =
                        panel.compact_content_height_for_state(&self.state, overlay_width);
                    // +2 for top/bottom borders, capped to available space
                    let overlay_height = (content_rows + 2).min(base.height.saturating_sub(1));
                    let overlay_y = base.y + base.height - overlay_height;
                    let overlay = Rect {
                        x: base.x,
                        y: overlay_y,
                        width: overlay_width,
                        height: overlay_height,
                    };
                    let overlay_focused = self.wm.focused() == Some(ComponentId::NtsPanel);
                    panel.borders = Borders::ALL;
                    panel.draw_compact(frame, overlay, overlay_focused, &self.state);
                    self.pane_areas.nts_overlay = overlay;
                }
            } else {
                self.pane_areas.nts_overlay = Rect::default();
            }
        } else {
            self.pane_areas.nts_overlay = Rect::default();
        }
    }

    fn draw_files(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        use ratatui::widgets::Borders;

        // Files focus ring: FileList=1, FileMeta=2, IcyTicker=3, SongsTicker=4
        self.icy_ticker.number_key = Some('3');
        self.songs_ticker.number_key = Some('4');

        let right_maximized = self.wm.files_right_maximized;

        let (left_pct, right_pct) = if right_maximized { (30, 70) } else { (45, 55) };

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(left_pct),
                Constraint::Percentage(right_pct),
            ])
            .split(area);

        let left_area = cols[0];
        let right_area = cols[1];

        let file_collapsed = self.wm.is_collapsed(ComponentId::FileList);
        let file_focused = self.wm.focused() == Some(ComponentId::FileList);

        if file_collapsed {
            use crate::widgets::pane_chrome::draw_collapsed_pane;
            let summary = self.file_list.collapse_summary(&self.state);
            draw_collapsed_pane(frame, left_area, "files", summary.as_deref(), file_focused);
        } else {
            self.file_list.borders = Borders::TOP | Borders::LEFT | Borders::BOTTOM;
            self.file_list
                .draw(frame, left_area, file_focused, &self.state);
        }
        self.pane_areas.file_list = left_area;

        // Right column: determine collapse state for each pane
        let meta_collapsed = self.wm.is_collapsed(ComponentId::FileMeta);
        let icy_collapsed = self.wm.is_collapsed(ComponentId::IcyTicker);
        let songs_collapsed = self.wm.is_collapsed(ComponentId::SongsTicker);

        let meta_focused = self.wm.focused() == Some(ComponentId::FileMeta);
        let icy_focused = self.wm.focused() == Some(ComponentId::IcyTicker);
        let songs_focused = self.wm.focused() == Some(ComponentId::SongsTicker);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                if meta_collapsed {
                    Constraint::Length(1)
                } else {
                    Constraint::Percentage(50)
                },
                if icy_collapsed {
                    Constraint::Length(1)
                } else {
                    Constraint::Percentage(25)
                },
                if songs_collapsed {
                    Constraint::Length(1)
                } else {
                    Constraint::Percentage(25)
                },
            ])
            .split(right_area);

        if meta_collapsed {
            use crate::widgets::pane_chrome::draw_collapsed_pane;
            let summary = self.file_meta.collapse_summary(&self.state);
            draw_collapsed_pane(frame, rows[0], "meta", summary.as_deref(), meta_focused);
        } else {
            self.file_meta.borders = Borders::ALL;
            self.file_meta
                .draw(frame, rows[0], meta_focused, &self.state);
        }

        if icy_collapsed {
            use crate::widgets::pane_chrome::draw_collapsed_pane;
            let summary = self.icy_ticker.collapse_summary(&self.state);
            draw_collapsed_pane(frame, rows[1], "icy", summary.as_deref(), icy_focused);
        } else {
            // Omit top border if meta is expanded above (shares bottom/top edge)
            self.icy_ticker.borders = if meta_collapsed {
                Borders::ALL
            } else {
                Borders::LEFT | Borders::BOTTOM | Borders::RIGHT
            };
            self.icy_ticker
                .draw(frame, rows[1], icy_focused, &self.state);
        }

        if songs_collapsed {
            use crate::widgets::pane_chrome::draw_collapsed_pane;
            let summary = self.songs_ticker.collapse_summary(&self.state);
            draw_collapsed_pane(frame, rows[2], "songs", summary.as_deref(), songs_focused);
        } else {
            // Omit top border if icy is expanded above
            self.songs_ticker.borders = if icy_collapsed {
                Borders::ALL
            } else {
                Borders::LEFT | Borders::BOTTOM | Borders::RIGHT
            };
            self.songs_ticker
                .draw(frame, rows[2], songs_focused, &self.state);
        }

        self.pane_areas.file_meta = rows[0];
        self.pane_areas.icy_ticker = rows[1];
        self.pane_areas.songs_ticker = rows[2];
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    async fn send_cmd(&self, cmd: Command) {
        let _ = self.cmd_tx.send(DaemonEvent::ClientCommand(cmd)).await;
    }

    fn push_log(&mut self, msg: String) {
        self.state.logs.push(msg);
        if self.state.logs.len() > 500 {
            self.state.logs.remove(0);
        }
    }

    /// Read the last 500 lines of tui.log into state.tui_log_lines (synchronous, cheap).
    fn reload_tui_log(&mut self) {
        let path = &self.tui_log_path;
        if let Ok(content) = std::fs::read_to_string(path) {
            let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
            let start = lines.len().saturating_sub(500);
            self.state.tui_log_lines = lines[start..].to_vec();
        }
    }

    fn spawn_passive_poll_task(&mut self, tx: mpsc::Sender<AppMessage>, reason: &str) {
        if self.auto_poll_in_flight {
            info!("[poll] skip cycle (already in flight)");
            return;
        }

        let stations = self.state.daemon_state.stations.clone();
        let targets = build_station_poll_targets(&stations, &mut self.non_nts_poll_cursor);
        let target_count = targets.len();
        if target_count == 0 {
            info!("[poll] skip cycle (no resolvable polling targets)");
            return;
        }

        self.auto_poll_cycle_id = self.auto_poll_cycle_id.saturating_add(1);
        self.auto_poll_cycle_total = target_count;
        self.auto_poll_cycle_seen = 0;
        self.auto_poll_cycle_changed = 0;
        self.auto_poll_cycle_unchanged = 0;
        self.auto_poll_cycle_errors = 0;
        self.auto_poll_in_flight = true;

        let cycle_id = self.auto_poll_cycle_id;
        let mut nts_live_count = 0usize;
        let mut nts_mixtape_count = 0usize;
        let mut non_nts_count = 0usize;
        let target_labels: Vec<String> = targets
            .iter()
            .map(|t| match t {
                StationPollTarget::NtsLive { station_name, .. } => {
                    nts_live_count += 1;
                    format!("nts-live:{}", station_name)
                }
                StationPollTarget::NtsMixtape { station_name, .. } => {
                    nts_mixtape_count += 1;
                    format!("nts-mixtape:{}", station_name)
                }
                StationPollTarget::NonNtsIcy { station_name, .. } => {
                    non_nts_count += 1;
                    format!("icy-probe:{}", station_name)
                }
            })
            .collect();

        let why = reason.to_string();
        info!(
            "[poll] cycle #{} start reason={} targets={} (nts-live={}, nts-mixtape={}, non-nts={})",
            cycle_id, why, target_count, nts_live_count, nts_mixtape_count, non_nts_count,
        );
        debug!(
            "[poll] cycle #{} target order: {}",
            cycle_id,
            target_labels.join(", ")
        );

        tokio::spawn(async move {
            let started = std::time::Instant::now();
            debug!("[poll] cycle #{} task spawned", cycle_id);
            run_station_poll_cycle(targets, tx.clone(), cycle_id).await;
            let elapsed_ms = started.elapsed().as_millis();
            let _ = tx
                .send(AppMessage::PassivePollCycleDone {
                    cycle_id,
                    total: target_count,
                    elapsed_ms,
                })
                .await;
        });
    }

    /// Maximum file metadata cache entries to prevent unbounded growth
    const MAX_METADATA_CACHE_SIZE: usize = 1000;

    /// Spawn an async recognition job (fire-and-forget, patch-in-place).
    ///
    /// 1. Immediately sends `RecognitionStarted` with initial row (job_id + station + icy).
    /// 2. Spawns three concurrent tasks:
    ///    a. ICY patch — immediate if icy_title is Some.
    ///    b. NTS patch — async API call (NTS 1/2 or NTS Infinite Mixtape).
    ///    c. vibra patch — silent mpv 10s capture + vibra fingerprint.
    fn spawn_recognition_job(
        &mut self,
        station_name: Option<String>,
        stream_url: Option<String>,
        icy_title: Option<String>,
        nts_ch: Option<usize>,
        nts_mixtape_url: Option<String>,
    ) {
        let Some(tx) = self.recognition_tx.clone() else {
            warn!("[app] Cannot spawn recognition job: recognition_tx not initialized");
            return;
        };

        let now = chrono::Local::now();
        let job_id = make_job_id(&now, station_name.as_deref());
        info!(
            "[app] Spawning recognition job_id={} station={:?} icy={:?} nts_ch={:?} nts_mixtape_url={:?} url={:?}",
            job_id, station_name, icy_title, nts_ch, nts_mixtape_url, stream_url
        );

        // Initial row — sent immediately so the UI shows something right away
        let initial = RecognitionResult {
            job_id: job_id.clone(),
            timestamp: Some(now),
            station: station_name.clone(),
            icy_info: icy_title.clone(),
            ..Default::default()
        };
        let tx2 = tx.clone();
        let tx3 = tx.clone();
        let tx4 = tx.clone();
        let job_id2 = job_id.clone();
        let job_id3 = job_id.clone();
        let job_id4 = job_id.clone();

        // Send initial row now (synchronous channel send in async context)
        let tx_init = tx.clone();
        tokio::spawn(async move {
            let _ = tx_init.send(AppMessage::RecognitionStarted(initial)).await;
        });

        // ── Task A: ICY patch (immediate) ─────────────────────────────────────
        if let Some(icy) = icy_title {
            let patch = VdsPatch {
                icy_info: Some(icy.clone()),
                ..Default::default()
            };
            tokio::spawn(async move {
                let _ = tx2.send(AppMessage::RecognitionPatch(job_id2, patch)).await;
            });
        }

        // ── Task B: NTS patch (async) ─────────────────────────────────────────
        if let Some(ch) = nts_ch {
            tokio::spawn(async move {
                if let Some((show, tag, url)) = recognize_via_nts(ch).await {
                    info!("[recognition] nts ch{}: show={:?}", ch + 1, show);
                    let patch = VdsPatch {
                        nts_show: Some(show.clone()),
                        nts_tag: tag,
                        nts_url: url,
                        ..Default::default()
                    };
                    let _ = tx3.send(AppMessage::RecognitionPatch(job_id3, patch)).await;
                } else {
                    warn!("[recognition] nts ch{}: no result", ch + 1);
                }
            });
        } else if let Some(mixtape_url) = nts_mixtape_url {
            tokio::spawn(async move {
                if let Some((show, url)) = recognize_via_nts_mixtape(&mixtape_url).await {
                    info!("[recognition] nts mixtape: show={:?}", show);
                    let patch = VdsPatch {
                        nts_show: Some(show),
                        nts_url: url,
                        ..Default::default()
                    };
                    let _ = tx3.send(AppMessage::RecognitionPatch(job_id3, patch)).await;
                } else {
                    info!("[recognition] nts mixtape: no announced show");
                }
            });
        }

        // ── Task C: vibra patch (async, ~10s) ────────────────────────────────
        if let Some(url) = stream_url {
            tokio::spawn(async move {
                info!("[recognition] vibra task started for url={}", url);
                let vibra_result = recognize_via_vibra(&url).await;
                if let Some(json) = vibra_result {
                    let rec_str = vibra_rec_string(&json);
                    info!("[recognition] vibra result: {:?}", rec_str);
                    let display = rec_str.clone().unwrap_or_else(|| "?".to_string());
                    let patch = VdsPatch {
                        vibra_rec: rec_str,
                        ..Default::default()
                    };
                    let _ = tx4
                        .send(AppMessage::RecognitionPatch(job_id4.clone(), patch))
                        .await;
                    let _ = tx4
                        .send(AppMessage::RecognitionComplete(job_id4, display))
                        .await;
                } else {
                    warn!("[recognition] vibra returned nothing");
                    let _ = tx4.send(AppMessage::RecognitionNoMatch).await;
                }
            });
        } else {
            // No stream URL — vibra can't run.  The spinner was already shown;
            // dismiss it immediately so it doesn't hang indefinitely.
            let tx_no_url = tx.clone();
            tokio::spawn(async move {
                let _ = tx_no_url.send(AppMessage::RecognitionNoMatch).await;
            });
        }
    }

    fn sync_input_mode(&mut self) {
        // When focus changes away from a component with an active filter, close it
        // Components manage their own filter state; we just reset mode here
        self.state.input_mode = InputMode::Normal;
    }

    /// Sync `state.nts_hover_channel` from the current station-list cursor.
    /// Called after programmatic cursor moves (session restore, initial load)
    /// so the overlay appears immediately without requiring a keypress.
    fn sync_nts_hover(&mut self) {
        let hover = self
            .station_list
            .selected_station_idx()
            .and_then(|orig_idx| self.state.daemon_state.stations.get(orig_idx))
            .and_then(|s| match s.name.as_str() {
                "NTS 1" => Some(0usize),
                "NTS 2" => Some(1usize),
                _ => None,
            });
        self.state.nts_hover_channel = hover;
        self.wm.rebuild_focus_ring_with(hover);
    }

    fn save_ui_session_state(&self) {
        let selected_station_name = self
            .station_list
            .selected_name(&self.state.daemon_state.stations);
        let selected_file_path = self
            .file_list
            .selected_path()
            .map(|p| p.to_string_lossy().to_string());

        let ui_state = UiSessionState {
            workspace: match self.wm.workspace {
                Workspace::Radio => "radio".to_string(),
                Workspace::Files => "files".to_string(),
            },
            focused_component: match self.wm.focused() {
                Some(ComponentId::StationList) => "stationlist".to_string(),
                Some(ComponentId::FileList) => "filelist".to_string(),
                Some(ComponentId::IcyTicker) => "icyticker".to_string(),
                Some(ComponentId::SongsTicker) => "songsticker".to_string(),
                Some(ComponentId::FileMeta) => "filemeta".to_string(),
                Some(ComponentId::NtsPanel) => "ntspanel".to_string(),
                _ => "stationlist".to_string(),
            },
            selected_station_name,
            selected_file_path,
            files_right_maximized: self.wm.files_right_maximized,
            station_sort_order: self.station_list.sort_label().to_string(),
            file_sort_order: self.file_list.sort_label().to_string(),
            last_station_name: self.last_station_name.clone(),
            last_file_path: self.last_file_path.clone(),
            last_file_pos: self.last_file_pos,
        };
        let _ = save_ui_session_state(&self.ui_state_path, &ui_state);
    }

    fn index_file_metadata_chunk(&mut self, n: usize) {
        let len = self.state.files.len();
        if len == 0 {
            return;
        }
        // Use file_list's internal cursor via a simple round-robin
        // For now probe files that aren't yet in cache
        let mut count = 0;
        let keys: Vec<String> = self
            .state
            .files
            .iter()
            .map(|f| f.path.to_string_lossy().to_string())
            .collect();
        for key in &keys {
            if count >= n {
                break;
            }
            if !self.state.file_metadata_cache.contains_key(key) {
                let path = std::path::Path::new(key);
                if let Some(meta) = probe_file_metadata(path) {
                    // Evict old entries if cache is full (simple FIFO)
                    if self.state.file_metadata_cache.len() >= Self::MAX_METADATA_CACHE_SIZE {
                        let keys_to_remove: Vec<String> = self
                            .state
                            .file_metadata_cache
                            .keys()
                            .take(100)
                            .cloned()
                            .collect();
                        for k in keys_to_remove {
                            self.state.file_metadata_cache.remove(&k);
                        }
                    }
                    self.state.file_metadata_cache.insert(key.clone(), meta);
                    count += 1;
                }
            }
        }
    }

    /// Get the currently selected song entry from songs ticker
    fn get_selected_song_entry(&self) -> Option<radio_proto::songs::RecognitionResult> {
        // Get the selected index from songs_ticker
        let selected = self.songs_ticker.selected;
        if selected < self.state.songs_history.len() {
            Some(self.state.songs_history[selected].clone())
        } else {
            None
        }
    }

    /// Start an NTS download immediately (no queue check — caller is responsible).
    fn start_download(&mut self, url: String, display: String) {
        info!("[app] Starting download for: {}", url);
        self.download_in_flight = true;
        self.toast.spinner(format!("downloading {}…", display));
        self.state
            .download_statuses
            .insert(url.clone(), DownloadStatus::Downloading(0.0));
        let download_dir = self.state.downloads_dir.clone();
        let tx = self.recognition_tx.clone();
        tokio::spawn(async move {
            let result = Self::download_nts_show(&url, &download_dir).await;
            if let Some(tx) = tx {
                let _ = tx.send(AppMessage::DownloadComplete { url, result }).await;
            }
        });
    }

    /// Download an NTS show
    async fn download_nts_show(
        url: &str,
        download_dir: &std::path::Path,
    ) -> Result<std::path::PathBuf, String> {
        // Find yt-dlp
        let yt_dlp_path = radio_proto::platform::find_yt_dlp_binary().ok_or("yt-dlp not found")?;

        // Use nts_download module
        crate::nts_download::download_episode(url, download_dir, &yt_dlp_path)
            .await
            .map_err(|e| e.to_string())
    }
}

// ── Daemon connection handler ─────────────────────────────────────────────────

fn build_station_poll_targets(
    stations: &[Station],
    non_nts_cursor: &mut usize,
) -> Vec<StationPollTarget> {
    let mut targets = Vec::new();
    let mut non_nts = Vec::new();
    for station in stations {
        if station.name.eq_ignore_ascii_case("NTS 1") {
            targets.push(StationPollTarget::NtsLive {
                station_name: station.name.clone(),
                channel_idx: 0,
            });
            continue;
        }
        if station.name.eq_ignore_ascii_case("NTS 2") {
            targets.push(StationPollTarget::NtsLive {
                station_name: station.name.clone(),
                channel_idx: 1,
            });
            continue;
        }

        let mixtape_url = station.mixtape_url.trim();
        if !mixtape_url.is_empty() {
            targets.push(StationPollTarget::NtsMixtape {
                station_name: station.name.clone(),
                mixtape_url: mixtape_url.to_string(),
            });
            continue;
        }

        if station.url.starts_with("http://") || station.url.starts_with("https://") {
            non_nts.push(StationPollTarget::NonNtsIcy {
                station_name: station.name.clone(),
                stream_url: station.url.clone(),
            });
        }
    }

    targets.extend(select_round_robin_non_nts(
        non_nts,
        non_nts_cursor,
        NON_NTS_MAX_JOBS_PER_CYCLE,
    ));

    targets
}

fn select_round_robin_non_nts(
    pool: Vec<StationPollTarget>,
    cursor: &mut usize,
    max_take: usize,
) -> Vec<StationPollTarget> {
    if pool.is_empty() || max_take == 0 {
        return Vec::new();
    }

    let len = pool.len();
    let start = *cursor % len;
    let take = max_take.min(len);

    let mut out = Vec::with_capacity(take);
    for i in 0..take {
        let idx = (start + i) % len;
        out.push(pool[idx].clone());
    }

    *cursor = (start + take) % len;
    out
}

async fn run_station_poll_cycle(
    targets: Vec<StationPollTarget>,
    tx: mpsc::Sender<AppMessage>,
    cycle_id: u64,
) {
    let total = targets.len();
    let mut nts_join = tokio::task::JoinSet::new();
    let mut non_nts_queue: VecDeque<NonNtsPollJob> = VecDeque::new();

    for (idx, target) in targets.into_iter().enumerate() {
        let ord = idx + 1;
        match target {
            StationPollTarget::NonNtsIcy {
                station_name,
                stream_url,
            } => {
                non_nts_queue.push_back(NonNtsPollJob {
                    ord,
                    station_name,
                    stream_url,
                });
            }
            nts_target => {
                let txn = tx.clone();
                nts_join.spawn(async move {
                    let outcome = poll_nts_target(nts_target, ord, total).await;
                    let _ = txn
                        .send(AppMessage::PassivePollOutcome { cycle_id, outcome })
                        .await;
                });
            }
        }
    }

    let non_nts_total = non_nts_queue.len();
    let nts_total = total.saturating_sub(non_nts_total);
    debug!(
        "[poll] cycle #{} scheduling: nts={} non-nts={} workers={}",
        cycle_id,
        nts_total,
        non_nts_total,
        NON_NTS_MAX_CONCURRENCY.min(non_nts_total).max(1),
    );

    // Launch non-NTS worker pool immediately so it runs concurrently with NTS polling.
    let mut non_nts_workers = tokio::task::JoinSet::new();
    if non_nts_total > 0 {
        match reqwest::Client::builder()
            .user_agent("r4dio-passive-icy-poller/0.1")
            .connect_timeout(Duration::from_millis(NON_NTS_CONNECT_TIMEOUT_MS))
            .timeout(Duration::from_millis(NON_NTS_REQUEST_TIMEOUT_MS))
            .build()
        {
            Ok(client) => {
                let queue = std::sync::Arc::new(TokioMutex::new(non_nts_queue));
                let cycle_start = std::time::Instant::now();
                let deadline = cycle_start + Duration::from_secs(NON_NTS_CYCLE_BUDGET_SECS);
                let worker_count = NON_NTS_MAX_CONCURRENCY.min(non_nts_total).max(1);
                debug!(
                    "[poll] cycle #{} launching {} workers for {} non-nts jobs (budget={}s connect={}ms req={}ms meta={}ms)",
                    cycle_id,
                    worker_count,
                    non_nts_total,
                    NON_NTS_CYCLE_BUDGET_SECS,
                    NON_NTS_CONNECT_TIMEOUT_MS,
                    NON_NTS_REQUEST_TIMEOUT_MS,
                    NON_NTS_METADATA_TIMEOUT_MS,
                );
                for worker_idx in 0..worker_count {
                    let queue = queue.clone();
                    let txw = tx.clone();
                    let cw = client.clone();
                    non_nts_workers.spawn(async move {
                        let mut jobs_done = 0usize;
                        loop {
                            let budget_remaining_ms = deadline
                                .saturating_duration_since(std::time::Instant::now())
                                .as_millis();
                            if std::time::Instant::now() >= deadline {
                                debug!(
                                    "[poll] worker={} budget exhausted after {} jobs, stopping",
                                    worker_idx, jobs_done
                                );
                                break;
                            }

                            let job = {
                                let mut q = queue.lock().await;
                                q.pop_front()
                            };
                            let Some(job) = job else {
                                debug!(
                                    "[poll] worker={} queue empty after {} jobs",
                                    worker_idx, jobs_done
                                );
                                break;
                            };

                            debug!(
                                "[poll] worker={} start job {}/{} '{}' (budget_remaining={}ms)",
                                worker_idx, job.ord, total, job.station_name, budget_remaining_ms
                            );
                            let job_start = std::time::Instant::now();
                            let outcome = poll_non_nts_station_icy(
                                cw.clone(),
                                job.station_name.clone(),
                                job.stream_url,
                                job.ord,
                                total,
                            )
                            .await;
                            let job_ms = job_start.elapsed().as_millis();
                            let result_tag = if outcome.error.is_some() {
                                "error"
                            } else if outcome.show.is_some() {
                                "title"
                            } else {
                                "no-title"
                            };
                            debug!(
                                "[poll] worker={} done job {}/{} '{}' result={} elapsed={}ms",
                                worker_idx, job.ord, total, job.station_name, result_tag, job_ms
                            );
                            jobs_done += 1;
                            let _ = txw
                                .send(AppMessage::PassivePollOutcome { cycle_id, outcome })
                                .await;
                        }
                        debug!(
                            "[poll] worker={} exited after {} jobs, total_elapsed={}ms",
                            worker_idx,
                            jobs_done,
                            cycle_start.elapsed().as_millis(),
                        );
                    });
                }

                // Wait for all workers and log final queue state.
                while let Some(joined) = non_nts_workers.join_next().await {
                    if let Err(e) = joined {
                        warn!("[poll] icy worker join error: {}", e);
                    }
                }
                let deferred = {
                    let q = queue.lock().await;
                    q.len()
                };
                if deferred > 0 {
                    info!(
                        "[poll] cycle #{} non-nts done: {} jobs deferred to next cycle (budget/concurrency)",
                        cycle_id, deferred
                    );
                } else {
                    debug!(
                        "[poll] cycle #{} non-nts done: all {} jobs completed in {}ms",
                        cycle_id,
                        non_nts_total,
                        cycle_start.elapsed().as_millis(),
                    );
                }
            }
            Err(e) => {
                warn!("[poll] failed to build icy probe client: {}", e);
                for job in non_nts_queue {
                    let _ = tx
                        .send(AppMessage::PassivePollOutcome {
                            cycle_id,
                            outcome: StationPollOutcome {
                                station_name: job.station_name,
                                resolver: "icy-probe".to_string(),
                                show: None,
                                error: Some("icy-probe-client-error".to_string()),
                            },
                        })
                        .await;
                }
            }
        }
    }

    // Drain any remaining NTS tasks (they run concurrently with non-NTS workers above).
    let nts_pending = nts_join.len();
    if nts_pending > 0 {
        debug!("[poll] cycle #{} waiting on {} remaining nts tasks", cycle_id, nts_pending);
    }
    while let Some(joined) = nts_join.join_next().await {
        if let Err(e) = joined {
            warn!("[poll] nts task join error: {}", e);
        }
    }
    debug!("[poll] cycle #{} all tasks finished", cycle_id);
}

async fn poll_nts_target(
    target: StationPollTarget,
    ord: usize,
    total: usize,
) -> StationPollOutcome {
    let resolver = target.resolver_label().to_string();
    match target {
        StationPollTarget::NtsLive {
            station_name,
            channel_idx,
        } => match tokio::time::timeout(
            Duration::from_secs(NTS_POLL_TASK_TIMEOUT_SECS),
            fetch_nts_channel(channel_idx),
        )
        .await
        {
            Ok(Ok(ch)) => {
                let show = ch.now.broadcast_title.trim().to_string();
                let show = if show.is_empty() { None } else { Some(show) };
                info!(
                    "[poll] [{}/{}] {} resolver=nts-live show={:?}",
                    ord, total, station_name, show
                );
                StationPollOutcome {
                    station_name,
                    resolver,
                    show,
                    error: None,
                }
            }
            Ok(Err(e)) => {
                warn!(
                    "[poll] [{}/{}] {} resolver=nts-live error={}",
                    ord, total, station_name, e
                );
                StationPollOutcome {
                    station_name,
                    resolver,
                    show: None,
                    error: Some(e.to_string()),
                }
            }
            Err(_) => {
                warn!(
                    "[poll] [{}/{}] {} resolver=nts-live timeout",
                    ord, total, station_name
                );
                StationPollOutcome {
                    station_name,
                    resolver,
                    show: None,
                    error: Some("timeout".to_string()),
                }
            }
        },

        StationPollTarget::NtsMixtape {
            station_name,
            mixtape_url,
        } => {
            let (show, timeout_err) = match tokio::time::timeout(
                Duration::from_secs(NTS_POLL_TASK_TIMEOUT_SECS),
                recognize_via_nts_mixtape(&mixtape_url),
            )
            .await
            {
                Ok(v) => (v.map(|(title, _url)| title), None),
                Err(_) => {
                    warn!(
                        "[poll] [{}/{}] {} resolver=nts-mixtape timeout",
                        ord, total, station_name
                    );
                    (None, Some("timeout".to_string()))
                }
            };
            info!(
                "[poll] [{}/{}] {} resolver=nts-mixtape show={:?}",
                ord, total, station_name, show
            );
            StationPollOutcome {
                station_name,
                resolver,
                show,
                error: timeout_err,
            }
        }

        StationPollTarget::NonNtsIcy { station_name, .. } => StationPollOutcome {
            station_name,
            resolver,
            show: None,
            error: Some("invalid-nts-target".to_string()),
        },
    }
}

async fn poll_non_nts_station_icy(
    client: reqwest::Client,
    station_name: String,
    stream_url: String,
    ord: usize,
    total: usize,
) -> StationPollOutcome {
    let started = std::time::Instant::now();
    let mut effective_url = stream_url.clone();
    let mut detail_prefix = String::new();

    if is_playlist_url(&effective_url) {
        match fetch_playlist_target(&client, &effective_url).await {
            Ok(Some(next)) => {
                detail_prefix = format!("playlist->{}", next);
                effective_url = next;
            }
            Ok(None) => {
                info!(
                    "[poll] [{}/{}] {} resolver=icy-probe playlist-no-target",
                    ord, total, station_name
                );
                return StationPollOutcome {
                    station_name,
                    resolver: "icy-probe".to_string(),
                    show: None,
                    error: None,
                };
            }
            Err(e) => {
                warn!(
                    "[poll] [{}/{}] {} resolver=icy-probe playlist-error={} ",
                    ord, total, station_name, e
                );
                return StationPollOutcome {
                    station_name,
                    resolver: "icy-probe".to_string(),
                    show: None,
                    error: Some(e),
                };
            }
        }
    }

    if looks_hls_url(&effective_url) {
        info!(
            "[poll] [{}/{}] {} resolver=icy-probe hls-skip {:?}",
            ord, total, station_name, effective_url
        );
        return StationPollOutcome {
            station_name,
            resolver: "icy-probe".to_string(),
            show: None,
            error: None,
        };
    }

    let mut req = client
        .get(&effective_url)
        .header("Icy-MetaData", HeaderValue::from_static("1"));
    if looks_hls_url(&effective_url) {
        req = req.header(
            "Accept",
            HeaderValue::from_static("application/vnd.apple.mpegurl,*/*"),
        );
    }

    let mut resp = match req.send().await {
        Ok(r) => match r.error_for_status() {
            Ok(ok) => ok,
            Err(e) => {
                return StationPollOutcome {
                    station_name,
                    resolver: "icy-probe".to_string(),
                    show: None,
                    error: Some(e.to_string()),
                };
            }
        },
        Err(e) => {
            return StationPollOutcome {
                station_name,
                resolver: "icy-probe".to_string(),
                show: None,
                error: Some(e.to_string()),
            };
        }
    };

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if content_type.contains("mpegurl") || looks_hls_url(&effective_url) {
        info!(
            "[poll] [{}/{}] {} resolver=icy-probe non-icy content-type={}",
            ord, total, station_name, content_type
        );
        return StationPollOutcome {
            station_name,
            resolver: "icy-probe".to_string(),
            show: None,
            error: None,
        };
    }

    let metaint = resp
        .headers()
        .get("icy-metaint")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok());

    let Some(metaint) = metaint else {
        info!(
            "[poll] [{}/{}] {} resolver=icy-probe no icy-metaint{}",
            ord,
            total,
            station_name,
            if detail_prefix.is_empty() {
                "".to_string()
            } else {
                format!(" ({})", detail_prefix)
            }
        );
        return StationPollOutcome {
            station_name,
            resolver: "icy-probe".to_string(),
            show: None,
            error: None,
        };
    };

    let icy_blocks = adaptive_icy_blocks(metaint);
    debug!(
        "[poll] [{}/{}] {} resolver=icy-probe metaint={} blocks={}",
        ord, total, station_name, metaint, icy_blocks
    );
    match read_icy_stream_title(
        &mut resp,
        metaint,
        Duration::from_millis(NON_NTS_METADATA_TIMEOUT_MS),
        icy_blocks,
    )
    .await
    {
        Ok((Some(title), bytes_read)) => {
            info!(
                "[poll] [{}/{}] {} resolver=icy-probe show={:?} bytes={} elapsed={}ms",
                ord,
                total,
                station_name,
                title,
                bytes_read,
                started.elapsed().as_millis()
            );
            StationPollOutcome {
                station_name,
                resolver: "icy-probe".to_string(),
                show: Some(title),
                error: None,
            }
        }
        Ok((None, bytes_read)) => {
            info!(
                "[poll] [{}/{}] {} resolver=icy-probe no-title bytes={} elapsed={}ms",
                ord,
                total,
                station_name,
                bytes_read,
                started.elapsed().as_millis()
            );
            StationPollOutcome {
                station_name,
                resolver: "icy-probe".to_string(),
                show: None,
                error: None,
            }
        }
        Err(e) => {
            warn!(
                "[poll] [{}/{}] {} resolver=icy-probe error={}",
                ord, total, station_name, e
            );
            StationPollOutcome {
                station_name,
                resolver: "icy-probe".to_string(),
                show: None,
                error: Some(e),
            }
        }
    }
}

async fn read_icy_stream_title(
    resp: &mut reqwest::Response,
    metaint: usize,
    timeout_total: Duration,
    max_blocks: usize,
) -> Result<(Option<String>, usize), String> {
    if !(1..=256_000).contains(&metaint) {
        return Err(format!("invalid icy-metaint={}", metaint));
    }

    let deadline = std::time::Instant::now() + timeout_total;
    let mut buf: Vec<u8> = Vec::with_capacity((metaint + 1024).min(128 * 1024));
    let mut cursor = 0usize;

    for _ in 0..max_blocks {
        let required_for_len = cursor + metaint + 1;
        while buf.len() < required_for_len {
            let now = std::time::Instant::now();
            if now >= deadline {
                return Err("timed out waiting for ICY metadata length byte".to_string());
            }
            let remain = deadline.saturating_duration_since(now);
            let next = tokio::time::timeout(remain, resp.chunk())
                .await
                .map_err(|_| "timed out waiting for stream chunk".to_string())?
                .map_err(|e| e.to_string())?;

            match next {
                Some(chunk) => buf.extend_from_slice(&chunk),
                None => return Err("stream ended before ICY metadata block".to_string()),
            }
        }

        let len_byte = buf[cursor + metaint] as usize;
        let meta_len = len_byte * 16;
        if meta_len > 16 * 255 {
            return Err(format!("metadata block too large: {}", meta_len));
        }

        let block_total = metaint + 1 + meta_len;
        let required_total = cursor + block_total;
        while buf.len() < required_total {
            let now = std::time::Instant::now();
            if now >= deadline {
                return Err("timed out waiting for ICY metadata payload".to_string());
            }
            let remain = deadline.saturating_duration_since(now);
            let next = tokio::time::timeout(remain, resp.chunk())
                .await
                .map_err(|_| "timed out waiting for stream chunk".to_string())?
                .map_err(|e| e.to_string())?;
            match next {
                Some(chunk) => buf.extend_from_slice(&chunk),
                None => return Err("stream ended before ICY metadata payload".to_string()),
            }
        }

        if meta_len > 0 {
            let meta_start = cursor + metaint + 1;
            let meta_end = meta_start + meta_len;
            let meta = &buf[meta_start..meta_end];
            if let Some(title) = parse_stream_title(meta) {
                return Ok((Some(title), required_total));
            }
        }

        cursor = required_total;
    }

    Ok((None, cursor))
}

fn parse_stream_title(meta: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(meta)
        .trim_matches(char::from(0))
        .trim()
        .to_string();
    if text.is_empty() {
        return None;
    }

    if let Some(start) = text.find("StreamTitle='") {
        let rest = &text[start + "StreamTitle='".len()..];
        if let Some(end) = rest.find("';") {
            let title = rest[..end].trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }

    if let Some(start) = text.find("StreamTitle=\"") {
        let rest = &text[start + "StreamTitle=\"".len()..];
        if let Some(end) = rest.find("\";") {
            let title = rest[..end].trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }

    None
}

async fn fetch_playlist_target(
    client: &reqwest::Client,
    url: &str,
) -> Result<Option<String>, String> {
    let body = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;

    if url.to_ascii_lowercase().ends_with(".pls") {
        for line in body.lines() {
            let l = line.trim();
            if l.to_ascii_lowercase().starts_with("file") {
                if let Some((_, v)) = l.split_once('=') {
                    return resolve_relative_url(url, v.trim()).map(Some);
                }
            }
        }
        return Ok(None);
    }

    for line in body.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        return resolve_relative_url(url, l).map(Some);
    }
    Ok(None)
}

fn resolve_relative_url(base: &str, candidate: &str) -> Result<String, String> {
    if candidate.starts_with("http://") || candidate.starts_with("https://") {
        return Ok(candidate.to_string());
    }
    let base_url = reqwest::Url::parse(base).map_err(|e| e.to_string())?;
    base_url
        .join(candidate)
        .map(|u| u.to_string())
        .map_err(|e| e.to_string())
}

fn is_playlist_url(url: &str) -> bool {
    let l = url.to_ascii_lowercase();
    l.ends_with(".m3u") || l.ends_with(".m3u8") || l.ends_with(".pls")
}

fn looks_hls_url(url: &str) -> bool {
    url.to_ascii_lowercase().contains(".m3u8")
}

fn adaptive_icy_blocks(metaint: usize) -> usize {
    if metaint > 96_000 {
        1
    } else if metaint > 48_000 {
        2
    } else {
        NON_NTS_ICY_BLOCKS
    }
}


// ── NTS fetch ─────────────────────────────────────────────────────────────────

async fn fetch_nts_channel(ch_idx: usize) -> anyhow::Result<NtsChannel> {
    let resp: serde_json::Value = reqwest::get("https://www.nts.live/api/v2/live")
        .await?
        .json()
        .await?;
    let channel = &resp["results"][ch_idx];
    let now_show = parse_nts_show(&channel["now"])?;
    let mut upcoming = Vec::new();
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
    let location_long = details["location_long"].as_str().unwrap_or("").to_string();
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

// ── Persistence helpers ───────────────────────────────────────────────────────

fn format_timestamp(ts: chrono::DateTime<chrono::Local>) -> String {
    let today = chrono::Local::now().date_naive();
    let ts_date = ts.date_naive();
    if ts_date == today {
        ts.format("%H:%M").to_string()
    } else {
        ts.format("%d/%m/%Y %H:%M").to_string()
    }
}

fn load_icy_log(path: &PathBuf) -> Vec<TickerEntry> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(200);
    lines[start..]
        .iter()
        .map(|line| TickerEntry {
            raw: line.trim().to_string(),
            display: line.trim().to_string(),
            station: None,
            show: None,
            url: None,
            comment: None,
        })
        .collect()
}

fn load_stars(path: &PathBuf) -> (HashMap<String, u8>, HashMap<String, u8>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return (HashMap::new(), HashMap::new());
    };
    match toml::from_str::<StarredState>(&content) {
        Ok(s) => (s.station_stars, s.file_stars),
        Err(_) => (HashMap::new(), HashMap::new()),
    }
}

fn save_stars(
    path: &PathBuf,
    station_stars: &HashMap<String, u8>,
    file_stars: &HashMap<String, u8>,
) -> anyhow::Result<()> {
    let state = StarredState {
        station_stars: station_stars.clone(),
        file_stars: file_stars.clone(),
    };
    let toml_str = toml::to_string_pretty(&state)?;
    std::fs::write(path, toml_str)?;
    Ok(())
}

fn load_random_history(path: &PathBuf) -> Vec<RandomHistoryEntry> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn save_random_history(path: &PathBuf, history: &[RandomHistoryEntry]) -> anyhow::Result<()> {
    std::fs::write(path, serde_json::to_string(history)?)?;
    Ok(())
}

fn load_recent_state(path: &PathBuf) -> RecentState {
    let Ok(content) = std::fs::read_to_string(path) else {
        return RecentState::default();
    };
    toml::from_str(&content).unwrap_or_default()
}

fn save_recent_state(path: &PathBuf, state: &RecentState) -> anyhow::Result<()> {
    std::fs::write(path, toml::to_string_pretty(state)?)?;
    Ok(())
}

fn load_file_positions(path: &PathBuf) -> HashMap<String, f64> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    toml::from_str(&content).unwrap_or_default()
}

fn save_file_positions(path: &PathBuf, positions: &HashMap<String, f64>) -> anyhow::Result<()> {
    std::fs::write(path, toml::to_string_pretty(positions)?)?;
    Ok(())
}

fn load_ui_session_state(path: &PathBuf) -> UiSessionState {
    let Ok(content) = std::fs::read_to_string(path) else {
        return UiSessionState::default();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn save_ui_session_state(path: &PathBuf, state: &UiSessionState) -> anyhow::Result<()> {
    std::fs::write(path, serde_json::to_string_pretty(state)?)?;
    Ok(())
}

pub fn normalize_search_text(input: &str) -> String {
    input.to_lowercase()
}

pub fn search_matches(query: &str, text: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let text_low = text.to_lowercase();
    for word in query.split_whitespace() {
        if !text_low.contains(&word.to_lowercase()) {
            return false;
        }
    }
    true
}

fn load_local_files(dir: &PathBuf) -> Vec<LocalFileEntry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<LocalFileEntry> = rd
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            if !is_playable_audio_path(&path) {
                return None;
            }
            let meta = entry.metadata().ok()?;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            Some(LocalFileEntry {
                path,
                name,
                size_bytes: meta.len(),
                modified: meta.modified().ok(),
            })
        })
        .collect();

    // Default sort: newest modified first
    files.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.name.cmp(&b.name))
    });

    files
}

fn is_playable_audio_path(path: &std::path::Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase());
    matches!(
        ext.as_deref(),
        Some(
            "mp3"
                | "flac"
                | "ogg"
                | "opus"
                | "m4a"
                | "aac"
                | "wav"
                | "aiff"
                | "wv"
                | "ape"
                | "mka"
                | "webm"
                | "mkv"
                | "mp4"
                | "m4b"
        )
    )
}

fn probe_file_metadata(path: &std::path::Path) -> Option<FileMetadata> {
    // Use ffprobe / ffmpeg to extract metadata via a simple JSON call
    // This mirrors the logic in the old main.rs
    let ffprobe_bin = radio_proto::platform::find_ffprobe_binary()
        .unwrap_or_else(|| std::path::PathBuf::from("ffprobe"));
    let output = std::process::Command::new(ffprobe_bin)
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_chapters",
            path.to_str()?,
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let format = &json["format"];
    let tags = &format["tags"];

    fn tag(tags: &serde_json::Value, keys: &[&str]) -> Option<String> {
        for k in keys {
            if let Some(v) = tags[k]
                .as_str()
                .or_else(|| tags[&k.to_uppercase()].as_str())
            {
                let s = v.trim().to_string();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
        None
    }

    let duration_secs = format["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok());
    let bitrate_kbps = format["bit_rate"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|b| b / 1000);

    // Parse chapters
    let chapters: Vec<FileChapter> = json["chapters"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|ch| {
                    let start = ch["start_time"]
                        .as_str()
                        .and_then(|s| s.parse::<f64>().ok())?;
                    let end = ch["end_time"]
                        .as_str()
                        .and_then(|s| s.parse::<f64>().ok())?;
                    let title = ch["tags"]["title"]
                        .as_str()
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if title.is_empty() {
                        return None;
                    }
                    Some(FileChapter {
                        title,
                        start_secs: start,
                        end_secs: end,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // Parse tracklist from description/comment tag
    let description = tag(tags, &["description", "comment", "DESCRIPTION", "COMMENT"]);
    let tracklist = description
        .as_deref()
        .map(extract_tracklist_lines)
        .unwrap_or_default();

    Some(FileMetadata {
        title: tag(tags, &["title"]),
        artist: tag(tags, &["artist"]),
        album: tag(tags, &["album"]),
        date: tag(tags, &["date", "year"]),
        description,
        genre: tag(tags, &["genre"]),
        duration_secs,
        codec: format["format_name"].as_str().map(|s| s.to_string()),
        bitrate_kbps,
        sample_rate_hz: None,
        channels: None,
        chapters,
        tracklist,
    })
}

fn extract_tracklist_lines(text: &str) -> Vec<String> {
    // Heuristic: lines that look like tracklist entries
    // e.g. "00:00 Artist - Title" or "1. Artist - Title"
    let mut lines = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Accept lines starting with HH:MM or MM:SS timestamp or a number+dot
        let looks_like_track = trimmed.len() > 3 && {
            let first = trimmed.split_whitespace().next().unwrap_or("");
            first.contains(':') || first.ends_with('.') || first.chars().all(|c| c.is_ascii_digit())
        };
        if looks_like_track {
            lines.push(trimmed.to_string());
        }
    }
    lines
}
