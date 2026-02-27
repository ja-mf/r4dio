/// DaemonCore — single-owner event loop for all mutable state.
///
/// Runs embedded in the TUI process.  All tasks that need to mutate playback
/// state send `DaemonEvent` messages to this loop.  DaemonCore owns
/// `DaemonState` and `MpvDriver` exclusively; no other task touches them.
///
/// After each event that mutates state, DaemonCore broadcasts a
/// `BroadcastMessage::StateUpdated` (or `IcyUpdated`) to all listeners via
/// a `tokio::sync::broadcast` channel.
///
/// mpv integration is **property-observation-driven**: on every fresh
/// connection we send `observe_property` for core-idle, pause, icy-title,
/// time-pos, and duration.  mpv pushes a `property-change` event whenever any
/// of those values change.  We no longer poll; the 10-second heartbeat tick
/// only checks process liveness.
use std::sync::Arc;

use radio_proto::config::Config;
use radio_proto::protocol::{Command, MpvHealth, PlaybackStatus, Station};
use radio_proto::state::{
    load_stations_from_m3u, load_stations_from_toml, parse_m3u_from_str, StateManager,
};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

use crate::mpv::{
    MpvDriver, MpvEvent, MpvHandle, OBS_AUDIO_LEVEL, OBS_CORE_IDLE, OBS_DURATION, OBS_ICY_TITLE,
    OBS_PAUSE, OBS_TIME_POS,
};
use crate::BroadcastMessage;

// ── DaemonEvent ───────────────────────────────────────────────────────────────

/// All inputs into the DaemonCore loop.
#[derive(Debug)]
pub enum DaemonEvent {
    /// A command from the TUI or HTTP API.
    ClientCommand(Command),
    /// Heartbeat — check process liveness.
    HeartbeatTick,
    /// Raw mpv unsolicited event (forwarded from reader task).
    MpvEvent(MpvEvent),
    /// Shutdown requested.
    #[allow(dead_code)]
    Shutdown,
}

// ── DaemonCore ────────────────────────────────────────────────────────────────

pub struct DaemonCore {
    config: Config,
    state_manager: Arc<StateManager>,
    mpv_driver: MpvDriver,
    /// Live handle to the mpv IO tasks.  `None` when mpv is not yet connected.
    mpv_handle: Option<MpvHandle>,
    /// Handle for the dedicated audio-level observer task.
    audio_observer_handle: Option<tokio::task::JoinHandle<()>>,
    /// Handle for the ffmpeg PCM capture task (feeds pcm_ring for oscilloscope).
    vu_task_handle: Option<tokio::task::AbortHandle>,
    /// Channel to forward mpv events back into our own event loop.
    mpv_event_tx: mpsc::Sender<DaemonEvent>,
    broadcast_tx: broadcast::Sender<BroadcastMessage>,
    /// true when the user has requested playback (used to derive status).
    intend_playing: bool,
    /// Current tracked health of the mpv process.
    mpv_health: MpvHealth,
    /// Observed property values from mpv push events.
    obs_core_idle: Option<bool>,
    obs_pause: bool,
    obs_icy_title: Option<String>,
    obs_time_pos: Option<f64>,
    obs_duration: Option<f64>,
    /// When we started connecting/buffering (to detect timeout).
    connecting_since: Option<tokio::time::Instant>,
    /// Last derived playback status (to avoid redundant broadcasts).
    last_status: PlaybackStatus,
    /// Last ICY title broadcast (to avoid duplicate IcyUpdated).
    last_icy: Option<String>,
    /// Last source context (to reset connecting timer on source change).
    last_source: (Option<usize>, Option<String>),
}

impl DaemonCore {
    pub async fn new(
        config: Config,
        broadcast_tx: broadcast::Sender<BroadcastMessage>,
        event_tx: mpsc::Sender<DaemonEvent>,
    ) -> anyhow::Result<Self> {
        let stations = load_stations(&config).await?;
        let state_manager = Arc::new(StateManager::new(
            config.daemon.state_file.clone(),
            stations,
        ));

        let initial_volume = state_manager.get_state().await.volume;
        let mut mpv_driver = MpvDriver::new();
        mpv_driver.last_volume = initial_volume;

        Ok(Self {
            config,
            state_manager,
            mpv_driver,
            mpv_handle: None,
            audio_observer_handle: None,
            vu_task_handle: None,
            mpv_event_tx: event_tx,
            broadcast_tx,
            intend_playing: false,
            mpv_health: MpvHealth::Absent,
            obs_core_idle: None,
            obs_pause: false,
            obs_icy_title: None,
            obs_time_pos: None,
            obs_duration: None,
            connecting_since: None,
            last_status: PlaybackStatus::Idle,
            last_icy: None,
            last_source: (None, None),
        })
    }

    /// Borrow the state manager (for use by the HTTP server).
    pub fn state_manager(&self) -> Arc<StateManager> {
        Arc::clone(&self.state_manager)
    }

    /// Run the core event loop.  Returns when a `Shutdown` event is received
    /// or the event channel is closed (TUI exited).
    pub async fn run(mut self, mut event_rx: mpsc::Receiver<DaemonEvent>) -> anyhow::Result<()> {
        info!("DaemonCore: starting event loop");

        // Kick off the heartbeat ticker — used for process liveness checks.
        let heartbeat_tx = self.mpv_event_tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                if heartbeat_tx.send(DaemonEvent::HeartbeatTick).await.is_err() {
                    break;
                }
            }
        });

        loop {
            let evt = event_rx.recv().await;
            match evt {
                None => {
                    info!("DaemonCore: event channel closed, shutting down");
                    break;
                }

                Some(DaemonEvent::Shutdown) => {
                    info!("DaemonCore: shutdown requested");
                    break;
                }

                Some(DaemonEvent::ClientCommand(cmd)) => {
                    info!("DaemonCore: command {:?}", cmd);
                    if let Err(e) = self.handle_command(cmd).await {
                        error!("DaemonCore: command error: {}", e);
                    }
                }

                Some(DaemonEvent::MpvEvent(evt)) => {
                    self.handle_mpv_event(evt).await;
                }

                Some(DaemonEvent::HeartbeatTick) => {
                    // Check process liveness — if mpv died, degrade health
                    if self.mpv_handle.is_some() && !self.mpv_driver.process_alive() {
                        warn!("DaemonCore: heartbeat: mpv process died");
                        self.mpv_handle = None;
                        if let Some(obs) = self.audio_observer_handle.take() {
                            obs.abort();
                        }
                        self.set_mpv_health(MpvHealth::Dead).await;
                        // Reset observed state
                        self.reset_observed_state();
                    }

                    // Also check connecting timeout (in case property events never arrive)
                    if self.intend_playing && !self.obs_pause {
                        self.maybe_update_status().await;
                    }
                }
            }
        }

        self.cleanup().await?;
        Ok(())
    }

    // ── mpv event handler ─────────────────────────────────────────────────────

    async fn handle_mpv_event(&mut self, evt: MpvEvent) {
        debug!("mpv event: {:?}", evt.raw);

        if let Some((obs_id, data)) = evt.as_property_change() {
            match obs_id {
                OBS_CORE_IDLE => {
                    let val = data.as_bool();
                    if val != self.obs_core_idle {
                        debug!("mpv: core-idle → {:?}", val);
                        self.obs_core_idle = val;
                        self.maybe_update_status().await;
                        // push timeline immediately too
                        self.state_manager
                            .set_timeline(self.obs_time_pos, self.obs_duration)
                            .await;
                        let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
                    }
                }
                OBS_PAUSE => {
                    let val = data.as_bool().unwrap_or(false);
                    if val != self.obs_pause {
                        debug!("mpv: pause → {}", val);
                        self.obs_pause = val;
                        self.maybe_update_status().await;
                    }
                }
                OBS_ICY_TITLE | 6 => {
                    // Both obs id 3 (metadata/by-key/icy-title) and id 6 (icy-title direct)
                    let raw_val = match data {
                        serde_json::Value::String(s) => Some(s.clone()),
                        serde_json::Value::Null => None,
                        _ => data.as_str().map(|s| s.to_string()),
                    };
                    // Filter trivial values
                    let val = raw_val.and_then(|t| {
                        let trimmed = t.trim().trim_matches('-').trim().to_string();
                        if trimmed.is_empty() {
                            None
                        } else {
                            Some(t)
                        }
                    });
                    if val != self.obs_icy_title {
                        info!("mpv: icy-title {:?} → {:?}", self.obs_icy_title, val);
                        self.obs_icy_title = val.clone();
                        self.state_manager.set_icy_title(val.clone()).await;
                        if val != self.last_icy {
                            self.last_icy = val.clone();
                            let _ = self.broadcast_tx.send(BroadcastMessage::IcyUpdated(val));
                        }
                    }
                }
                OBS_TIME_POS => {
                    let val = if data.is_null() { None } else { data.as_f64() };
                    self.obs_time_pos = val;
                    self.state_manager
                        .set_timeline(self.obs_time_pos, self.obs_duration)
                        .await;
                    let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
                }
                OBS_DURATION => {
                    let val = if data.is_null() { None } else { data.as_f64() };
                    if val != self.obs_duration {
                        self.obs_duration = val;
                        self.state_manager
                            .set_timeline(self.obs_time_pos, self.obs_duration)
                            .await;
                        let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
                    }
                }
                OBS_AUDIO_LEVEL => {
                    // Fallback only: when the dedicated audio observer is not running,
                    // use the main mpv event path for RMS updates.
                    if self.audio_observer_handle.is_none() {
                        if let Some(obj) = data.as_object() {
                            let rms = obj
                                .get("lavfi.astats.Overall.RMS_level")
                                .and_then(|v| v.as_str())
                                .and_then(|s| s.parse::<f32>().ok())
                                .unwrap_or(-90.0);
                            let _ = self.broadcast_tx.send(BroadcastMessage::AudioLevel(rms));
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        // Handle named events (non-property-change)
        match evt.event_name() {
            Some("end-file") => {
                let reason = evt
                    .raw
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                info!("mpv: end-file reason={}", reason);
                if reason == "error" || reason == "network" || reason == "quit" {
                    // If we intended to play, mark error
                    if self.intend_playing && !self.obs_pause {
                        warn!("mpv: stream ended with error/network reason, marking Error");
                        self.state_manager
                            .set_playback_status(PlaybackStatus::Error)
                            .await;
                        let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
                        self.last_status = PlaybackStatus::Error;
                        self.connecting_since = None;
                    }
                }
                // Clear ICY on stream end
                if self.obs_icy_title.is_some() {
                    self.obs_icy_title = None;
                    self.state_manager.set_icy_title(None).await;
                    if self.last_icy.is_some() {
                        self.last_icy = None;
                        let _ = self.broadcast_tx.send(BroadcastMessage::IcyUpdated(None));
                    }
                }
                // Reset timeline
                self.obs_time_pos = None;
                self.obs_duration = None;
                self.state_manager.set_timeline(None, None).await;
                self.obs_core_idle = Some(true);
                self.maybe_update_status().await;
            }
            Some("start-file") => {
                info!("mpv: start-file");
                self.connecting_since = None;
                self.obs_core_idle = Some(true); // will flip to false when audio flows
                self.maybe_update_status().await;
            }
            Some("file-loaded") => {
                info!("mpv: file-loaded — re-issuing observe_property and audio filter");
                // Wait 50ms before re-observing so mpv has settled on the new file,
                // then re-register observations so mpv pushes current values immediately.
                if let Some(h) = self.mpv_handle.clone() {
                    tokio::spawn(async move {
                        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                        h.observe_all_properties().await;
                        h.set_audio_filter().await;
                    });
                }
            }
            _ => {}
        }
    }

    /// Derive PlaybackStatus from observed state and update if changed.
    async fn maybe_update_status(&mut self) {
        let status = if !self.intend_playing {
            self.connecting_since = None;
            PlaybackStatus::Idle
        } else if self.obs_pause {
            self.connecting_since = None;
            PlaybackStatus::Paused
        } else {
            match self.obs_core_idle {
                Some(false) => {
                    self.connecting_since = None;
                    PlaybackStatus::Playing
                }
                other => {
                    let since = self
                        .connecting_since
                        .get_or_insert_with(tokio::time::Instant::now);
                    let elapsed = since.elapsed().as_secs();
                    debug!(
                        "mpv: waiting for playback core_idle={:?} elapsed={}s",
                        other, elapsed
                    );
                    if elapsed >= 15 {
                        warn!("mpv: no audio after {}s, marking Error", elapsed);
                        PlaybackStatus::Error
                    } else {
                        PlaybackStatus::Connecting
                    }
                }
            }
        };

        if status != self.last_status {
            info!("DaemonCore: status {:?} → {:?}", self.last_status, status);
            self.last_status = status.clone();
            self.state_manager.set_playback_status(status).await;
            let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
        }
    }

    fn reset_observed_state(&mut self) {
        self.obs_core_idle = None;
        self.obs_pause = false;
        self.obs_icy_title = None;
        self.obs_time_pos = None;
        self.obs_duration = None;
        self.connecting_since = None;
    }

    // ── mpv handle management ─────────────────────────────────────────────────

    /// Update tracked mpv health and broadcast state if it changed.
    async fn set_mpv_health(&mut self, health: MpvHealth) {
        if self.mpv_health != health {
            info!(
                "DaemonCore: mpv health {:?} → {:?}",
                self.mpv_health, health
            );
            self.mpv_health = health.clone();
            self.state_manager.set_mpv_health(health).await;
            let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
        }
    }

    async fn ensure_mpv_handle(&mut self) -> Option<MpvHandle> {
        // If we have a handle, check that the process is still alive
        if self.mpv_handle.is_some() {
            if !self.mpv_driver.process_alive() {
                warn!("DaemonCore: mpv process died, dropping handle");
                self.mpv_handle = None;
                self.set_mpv_health(MpvHealth::Dead).await;
                self.reset_observed_state();
            }
        }

        if self.mpv_handle.is_none() {
            // Single channel + single forwarder task for this connection.
            // Both try_reconnect and spawn_and_connect receive a clone of the
            // same sender so only one forwarder is ever running.
            let (event_tx, event_rx) = mpsc::channel::<MpvEvent>(64);
            let core_tx = self.mpv_event_tx.clone();
            tokio::spawn(async move {
                let mut rx = event_rx;
                while let Some(evt) = rx.recv().await {
                    if core_tx.send(DaemonEvent::MpvEvent(evt)).await.is_err() {
                        break;
                    }
                }
            });

            // Try to reconnect to an existing socket first, then spawn fresh.
            let handle = match self.mpv_driver.try_reconnect(event_tx.clone()).await {
                Some(h) => {
                    info!("DaemonCore: reconnected to existing mpv socket");
                    h
                }
                None => {
                    self.set_mpv_health(MpvHealth::Starting).await;
                    match self.mpv_driver.spawn_and_connect(event_tx).await {
                        Ok(h) => h,
                        Err(e) => {
                            warn!("DaemonCore: failed to start mpv: {}", e);
                            self.set_mpv_health(MpvHealth::Dead).await;
                            return None;
                        }
                    }
                }
            };

            self.set_mpv_health(MpvHealth::Running).await;

            // Register property observations + audio filter on the fresh handle.
            let h_clone = handle.clone();
            tokio::spawn(async move {
                h_clone.observe_all_properties().await;
                h_clone.set_audio_filter().await;
            });

            // Audio observer (lavfi) — only used for local file playback.
            // Abort previous one unconditionally; it will be re-spawned only
            // when a file (not a station) starts playing.
            if let Some(prev) = self.audio_observer_handle.take() {
                prev.abort();
            }

            self.mpv_handle = Some(handle);
        }

        self.mpv_handle.clone()
    }

    // ── command handlers ──────────────────────────────────────────────────────

    async fn handle_command(&mut self, cmd: Command) -> anyhow::Result<()> {
        match cmd {
            Command::Play { station_idx } => self.play_station(station_idx).await?,
            Command::PlayFile { path } => self.play_file(path, None, false).await?,
            Command::PlayFileAt { path, start_secs } => {
                self.play_file(path, Some(start_secs), false).await?
            }
            Command::PlayFilePausedAt { path, start_secs } => {
                self.play_file(path, Some(start_secs), true).await?
            }
            Command::Stop => self.stop().await?,
            Command::Next => self.next().await?,
            Command::Prev => self.prev().await?,
            Command::Random => self.random().await?,
            Command::TogglePause => self.toggle_pause().await?,
            Command::Volume { value } => self.set_volume(value).await?,
            Command::SeekRelative { seconds } => self.seek_relative(seconds).await?,
            Command::SeekTo { seconds } => self.seek_to(seconds).await?,
            Command::GetState => {
                // State will be broadcast automatically
            }
        }
        Ok(())
    }

    async fn play_station(&mut self, idx: usize) -> anyhow::Result<()> {
        let (station, volume) = {
            let state = self.state_manager.get_state().await;
            (state.stations.get(idx).cloned(), state.volume)
        };

        if let Some(station) = station {
            info!("Playing station: {}", station.name);

            // Abort any running VU ffmpeg task and lavfi observer before starting a new one.
            if let Some(h) = self.vu_task_handle.take() {
                h.abort();
            }
            if let Some(h) = self.audio_observer_handle.take() {
                h.abort();
            }

            // Always reset the connecting timer when a new play command arrives,
            // even if the same station is being replayed (user pressed Enter twice).
            let new_source = (Some(idx), None);
            self.last_source = new_source;
            self.connecting_since = None;
            self.obs_core_idle = None;

            self.intend_playing = true;
            self.state_manager.set_playing(idx).await?;
            let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);

            match self.ensure_mpv_handle().await {
                Some(handle) => {
                    let mut stream_url = station.url.clone();
                    let wants_proxy = !station.url.to_ascii_lowercase().contains(".m3u8");
                    let mut used_proxy = false;
                    if wants_proxy {
                        let proxy_url = crate::proxy::proxy_url(idx);
                        if let Err(e) = handle.load_stream(&proxy_url, volume).await {
                            warn!("Failed to load proxy stream '{}': {}", station.name, e);
                            if let Err(e2) = handle.load_stream(&stream_url, volume).await {
                                warn!("Failed to load direct stream '{}': {}", station.name, e2);
                                self.intend_playing = false;
                                self.state_manager
                                    .set_playback_status(PlaybackStatus::Error)
                                    .await;
                                let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
                                return Ok(());
                            }
                        } else {
                            stream_url = proxy_url;
                            used_proxy = true;
                        }
                    } else if let Err(e) = handle.load_stream(&stream_url, volume).await {
                        warn!("Failed to load direct stream '{}': {}", station.name, e);
                        self.intend_playing = false;
                        self.state_manager
                            .set_playback_status(PlaybackStatus::Error)
                            .await;
                        let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
                        return Ok(());
                    }
                    if used_proxy {
                        info!(
                            "Playing '{}' via shared proxy: {}",
                            station.name, stream_url
                        );
                    } else if wants_proxy {
                        info!("Playing '{}' direct URL (proxy unavailable)", station.name);
                    } else {
                        info!("Playing '{}' direct URL (HLS stream)", station.name);
                    }

                    // Spawn mpv lavfi observer for debug RMS bulb (independent
                    // from the PCM-driven VU/scope path used for streams).
                    let obs = crate::mpv::spawn_audio_observer(
                        self.mpv_driver.socket_name.clone(),
                        self.broadcast_tx.clone(),
                    );
                    self.audio_observer_handle = Some(obs);

                    // Spawn ffmpeg PCM task for oscilloscope.
                    // Prefer proxy URL so mpv + ffmpeg share one upstream source.
                    let tx = self.broadcast_tx.clone();
                    let handle = tokio::spawn(async move {
                        loop {
                            if let Err(e) = run_vu_ffmpeg(&stream_url, &tx).await {
                                debug!("VU ffmpeg exited: {e}");
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        }
                    });
                    self.vu_task_handle = Some(handle.abort_handle());
                }
                None => {
                    warn!("No mpv handle available for station '{}'", station.name);
                    self.intend_playing = false;
                    self.state_manager
                        .set_playback_status(PlaybackStatus::Error)
                        .await;
                    let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
                }
            }
        }
        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        info!("Stopping playback");
        self.intend_playing = false;
        self.connecting_since = None;
        if let Some(h) = self.vu_task_handle.take() {
            h.abort();
        }
        if let Some(h) = self.audio_observer_handle.take() {
            h.abort();
        }
        if let Some(handle) = self.mpv_handle.as_ref() {
            handle.stop().await?;
        }
        self.state_manager.set_stopped().await?;
        // Clear ICY
        self.obs_icy_title = None;
        self.state_manager.set_icy_title(None).await;
        if self.last_icy.is_some() {
            self.last_icy = None;
            let _ = self.broadcast_tx.send(BroadcastMessage::IcyUpdated(None));
        }
        let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
        Ok(())
    }

    async fn play_file(
        &mut self,
        path: String,
        start_secs: Option<f64>,
        pause_after: bool,
    ) -> anyhow::Result<()> {
        let volume = self.state_manager.get_state().await.volume;
        info!("Playing local file: {}", path);

        let new_source = (None, Some(path.clone()));
        // Always reset the connecting timer on a new play command.
        self.last_source = new_source;
        self.connecting_since = None;
        self.obs_core_idle = None;

        self.intend_playing = true;
        self.state_manager.set_playing_file(path.clone()).await?;
        let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);

        match self.ensure_mpv_handle().await {
            Some(handle) => {
                if let Err(e) = handle.load_stream(&path, volume).await {
                    warn!("Failed to load file '{}': {}", path, e);
                    self.intend_playing = false;
                    self.state_manager
                        .set_playback_status(PlaybackStatus::Error)
                        .await;
                    let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
                } else {
                    if let Some(start) = start_secs {
                        if start > 0.0 {
                            if let Err(e) = handle.seek_to(start).await {
                                warn!("Failed to seek '{}' to {:.1}s: {}", path, start, e);
                            }
                        }
                    }
                    if pause_after {
                        let _ = handle.set_pause(true).await;
                    }
                    // File playback: use lavfi observer (main meter source for files).
                    // Kill any lingering VU ffmpeg task (used for stations).
                    if let Some(h) = self.vu_task_handle.take() {
                        h.abort();
                    }
                    if let Some(prev) = self.audio_observer_handle.take() {
                        prev.abort();
                    }
                    let obs = crate::mpv::spawn_audio_observer(
                        self.mpv_driver.socket_name.clone(),
                        self.broadcast_tx.clone(),
                    );
                    self.audio_observer_handle = Some(obs);
                }
            }
            None => {
                warn!("No mpv handle available for file '{}'", path);
                self.intend_playing = false;
                self.state_manager
                    .set_playback_status(PlaybackStatus::Error)
                    .await;
                let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
            }
        }
        Ok(())
    }

    async fn next(&mut self) -> anyhow::Result<()> {
        self.state_manager.next_station().await?;
        if let Some(idx) = self.state_manager.get_state().await.current_station {
            self.play_station(idx).await?;
        }
        Ok(())
    }

    async fn prev(&mut self) -> anyhow::Result<()> {
        self.state_manager.prev_station().await?;
        if let Some(idx) = self.state_manager.get_state().await.current_station {
            self.play_station(idx).await?;
        }
        Ok(())
    }

    async fn random(&mut self) -> anyhow::Result<()> {
        self.state_manager.random_station().await?;
        if let Some(idx) = self.state_manager.get_state().await.current_station {
            self.play_station(idx).await?;
        }
        Ok(())
    }

    async fn set_volume(&mut self, value: f32) -> anyhow::Result<()> {
        self.state_manager.set_volume(value).await?;
        self.mpv_driver.last_volume = value;
        if let Some(handle) = self.mpv_handle.as_ref() {
            handle.set_volume(value).await?;
        }
        let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
        Ok(())
    }

    async fn toggle_pause(&mut self) -> anyhow::Result<()> {
        let state = self.state_manager.get_state().await;
        if state.current_station.is_none() && state.current_file.is_none() {
            return Ok(());
        }
        if let Some(handle) = self.mpv_handle.as_ref() {
            // Use the locally-observed pause state rather than an IPC round-trip
            // (avoids a 5-second timeout if mpv is buffering).
            handle.set_pause(!self.obs_pause).await?;
        }
        let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
        Ok(())
    }

    async fn seek_relative(&mut self, seconds: f64) -> anyhow::Result<()> {
        let state = self.state_manager.get_state().await;
        if state.current_file.is_none() {
            return Ok(());
        }
        if let Some(handle) = self.mpv_handle.as_ref() {
            handle.seek_relative(seconds).await?;
        }
        Ok(())
    }

    async fn seek_to(&mut self, seconds: f64) -> anyhow::Result<()> {
        let state = self.state_manager.get_state().await;
        if state.current_file.is_none() {
            return Ok(());
        }
        if let Some(handle) = self.mpv_handle.as_ref() {
            handle.seek_to(seconds).await?;
        }
        Ok(())
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    async fn cleanup(&mut self) -> anyhow::Result<()> {
        info!("DaemonCore: cleanup — killing mpv");
        if let Some(handle) = self.mpv_handle.take() {
            let _ = handle.stop().await;
        }
        self.mpv_driver.kill().await;
        Ok(())
    }
}

// ── station loader ────────────────────────────────────────────────────────────

pub async fn load_stations(config: &Config) -> anyhow::Result<Vec<Station>> {
    use std::path::PathBuf;

    // 1. User config dir (highest priority — user's custom stations)
    let toml_path = &config.stations.stations_toml;
    if toml_path.exists() {
        match load_stations_from_toml(toml_path) {
            Ok(s) => {
                info!(
                    "Loaded {} stations from TOML: {}",
                    s.len(),
                    toml_path.display()
                );
                return Ok(s);
            }
            Err(e) => warn!("Failed to parse TOML stations: {}", e),
        }
    }

    // 1.5. stations.toml beside executable (bundled distribution)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside = dir.join("stations.toml");
            if beside.exists() {
                match load_stations_from_toml(&beside) {
                    Ok(s) => {
                        info!(
                            "Loaded {} stations from beside-exe: {}",
                            s.len(),
                            beside.display()
                        );
                        return Ok(s);
                    }
                    Err(e) => warn!("Failed to parse beside-exe stations.toml: {}", e),
                }
            }
        }
    }

    // 2. stations.toml in working directory
    let local_toml = PathBuf::from("stations.toml");
    if local_toml.exists() {
        match load_stations_from_toml(&local_toml) {
            Ok(s) => {
                info!("Loaded {} stations from local stations.toml", s.len());
                return Ok(s);
            }
            Err(e) => warn!("Failed to parse local stations.toml: {}", e),
        }
    }

    // 3. m3u URL or file
    let source = &config.stations.m3u_url;
    info!("Loading stations from m3u: {}", source);

    if source.starts_with("http://") || source.starts_with("https://") {
        match fetch_m3u_url(source).await {
            Ok(s) => {
                info!("Loaded {} stations from URL", s.len());
                return Ok(s);
            }
            Err(e) => warn!("Failed to fetch stations from URL: {}", e),
        }
    } else {
        let path = PathBuf::from(source);
        if path.exists() {
            match load_stations_from_m3u(&path) {
                Ok(s) => {
                    info!("Loaded {} stations from m3u file", s.len());
                    return Ok(s);
                }
                Err(e) => warn!("Failed to read m3u file: {}", e),
            }
        }
    }

    // 4. Last-resort local m3u files
    for filename in &["jamf_radios.m3u", "radios.m3u"] {
        let path = PathBuf::from(filename);
        if path.exists() {
            if let Ok(s) = load_stations_from_m3u(&path) {
                info!(
                    "Loaded {} stations from local fallback {}",
                    s.len(),
                    filename
                );
                return Ok(s);
            }
        }
    }

    info!("No station source available, starting with empty list");
    Ok(Vec::new())
}

async fn fetch_m3u_url(url: &str) -> anyhow::Result<Vec<Station>> {
    let response = reqwest::get(url).await?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP {}", response.status());
    }
    let text = response.text().await?;
    parse_m3u_from_str(&text)
}

// ── VU meter / PCM capture ────────────────────────────────────────────────────

const VU_WINDOW_SAMPLES: usize = 1024;
const VU_SAMPLE_RATE: u32 = 44100;

/// Spawn ffmpeg, decode mono s16le PCM, broadcast only PcmChunk.
/// RMS / AudioLevel is computed from PcmChunk in the app handler — single source of truth.
async fn run_vu_ffmpeg(
    url: &str,
    broadcast_tx: &tokio::sync::broadcast::Sender<BroadcastMessage>,
) -> anyhow::Result<()> {
    use std::path::PathBuf;
    use tokio::io::AsyncReadExt;
    use tokio::process::Command;

    let rate = VU_SAMPLE_RATE.to_string();
    let ffmpeg_bin =
        radio_proto::platform::find_ffmpeg_binary().unwrap_or_else(|| PathBuf::from("ffmpeg"));
    let mut child = Command::new(ffmpeg_bin)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-fflags",
            "nobuffer",
            "-flags",
            "low_delay",
            "-probesize",
            "64k",
            "-analyzeduration",
            "200000",
            "-i",
            url,
            "-vn",
            "-ac",
            "1",
            "-ar",
            &rate,
            "-f",
            "s16le",
            "pipe:1",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    let mut stdout = child.stdout.take().expect("ffmpeg stdout");
    let buf_bytes = VU_WINDOW_SAMPLES * 2;
    let mut buf = vec![0u8; buf_bytes];
    let mut sample_buf: Vec<i16> = Vec::with_capacity(VU_WINDOW_SAMPLES);

    loop {
        let n = stdout.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let samples_in = n / 2;
        for i in 0..samples_in {
            let sample = i16::from_le_bytes([buf[i * 2], buf[i * 2 + 1]]);
            sample_buf.push(sample);
            if sample_buf.len() >= VU_WINDOW_SAMPLES {
                let pcm: Vec<f32> = sample_buf.iter().map(|&s| s as f32 / 32768.0).collect();
                let _ = broadcast_tx.send(BroadcastMessage::PcmChunk(std::sync::Arc::new(pcm)));
                sample_buf.clear();
            }
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("ffmpeg exited: {}", status);
    }
    Ok(())
}

fn pcm_rms_db(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return -90.0;
    }
    let sum_sq: f64 = samples
        .iter()
        .map(|&s| {
            let f = s as f64 / 32768.0;
            f * f
        })
        .sum();
    let rms = (sum_sq / samples.len() as f64).sqrt();
    if rms < 1e-10 {
        -90.0
    } else {
        (20.0 * rms.log10()) as f32
    }
}
