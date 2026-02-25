mod mpv;
mod socket;
mod http;

use radio_tui::shared::config::Config;
use radio_tui::shared::protocol::{Command, PlaybackStatus, Station};
use radio_tui::shared::state::{
    load_stations_from_m3u, load_stations_from_toml, parse_m3u_from_str, StateManager,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

pub struct Daemon {
    config: Config,
    state_manager: Arc<StateManager>,
    mpv: Arc<tokio::sync::Mutex<mpv::MpvController>>,
    clients: Arc<tokio::sync::RwLock<Vec<socket::ClientHandle>>>,
    broadcast_tx: broadcast::Sender<BroadcastMessage>,
    command_tx: tokio::sync::mpsc::Sender<Command>,
    /// true when the user has requested playback (used by monitor)
    intend_playing: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub enum BroadcastMessage {
    StateUpdated,
    IcyUpdated(Option<String>),
    Log(String),
}

impl Daemon {
    pub async fn new(config: Config) -> anyhow::Result<(Self, tokio::sync::mpsc::Receiver<Command>)> {
        let stations = Self::load_stations(&config).await?;
        let state_manager = Arc::new(StateManager::new(
            config.daemon.state_file.clone(),
            stations,
        ));
        
        let (broadcast_tx, _) = broadcast::channel(100);
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(100);

        let initial_volume = state_manager.get_state().await.volume;
        let mut mpv_ctrl = mpv::MpvController::new().await?;
        mpv_ctrl.set_last_volume(initial_volume);
        let mpv = Arc::new(tokio::sync::Mutex::new(mpv_ctrl));

        let daemon = Self {
            config,
            state_manager,
            mpv,
            clients: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            broadcast_tx,
            command_tx,
            intend_playing: Arc::new(AtomicBool::new(false)),
        };
        
        Ok((daemon, command_rx))
    }
    
    pub async fn run(mut self, mut command_rx: tokio::sync::mpsc::Receiver<Command>) -> anyhow::Result<()> {
        info!("Starting radio daemon");
        
        // Setup file logging
        let _log_path = self.config.daemon.state_file.parent()
            .map(|p| p.join("daemon.log"))
            .unwrap_or_else(|| radio_tui::shared::platform::data_dir().join("daemon.log"));
        
        // Write PID file
        self.write_pid_file().await?;
        
        // Start TCP socket server
        let command_tx = self.command_tx.clone();
        let socket_handle = socket::start_server(
            self.config.http.bind_address.clone(),
            radio_tui::shared::platform::DAEMON_TCP_PORT,
            self.state_manager.clone(),
            self.clients.clone(),
            command_tx,
            self.broadcast_tx.clone(),
        );
        
        // Start HTTP API if enabled
        let http_handle = if self.config.http.enabled {
            Some(http::start_server(
                self.config.http.bind_address.clone(),
                self.config.http.port,
                self.state_manager.clone(),
                self.command_tx.clone(),
            ))
        } else {
            None
        };
        
        // Start MPV monitoring
        let mpv_handle = self.start_mpv_monitor();
        
        // Main loop - process commands and check for shutdown
        let mut last_client_count = 0;
        let mut empty_since = None;
        
        loop {
            tokio::select! {
                Some(cmd) = command_rx.recv() => {
                    info!("Processing command: {:?}", cmd);
                    if let Err(e) = self.handle_command(cmd).await {
                        error!("Error handling command: {}", e);
                    }
                }
                
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {
                    let client_count = self.clients.read().await.len();
                    
                    if client_count == 0 {
                        if last_client_count > 0 {
                            // Client just disconnected, start grace period
                            empty_since = Some(tokio::time::Instant::now());
                            info!("Last client disconnected, starting shutdown grace period");
                        } else if let Some(since) = empty_since {
                            // Been empty for a while, check if we should shutdown
                            if since.elapsed() > Duration::from_secs(5) {
                                info!("No clients for 5 seconds, shutting down daemon");
                                break;
                            }
                        } else {
                            // Never had clients, wait longer (initial startup)
                            // Don't shutdown for first 30 seconds
                        }
                    } else {
                        empty_since = None;
                    }
                    
                    last_client_count = client_count;
                }
            }
        }
        
        // Cleanup
        self.cleanup().await?;
        
        Ok(())
    }
    
    pub async fn handle_command(&self, cmd: Command) -> anyhow::Result<()> {
        match cmd {
            Command::Play { station_idx } => {
                self.play_station(station_idx).await?;
            }
            Command::PlayFile { path } => {
                self.play_file(path, None, false).await?;
            }
            Command::PlayFileAt { path, start_secs } => {
                self.play_file(path, Some(start_secs), false).await?;
            }
            Command::PlayFilePausedAt { path, start_secs } => {
                self.play_file(path, Some(start_secs), true).await?;
            }
            Command::Stop => {
                self.stop().await?;
            }
            Command::Next => {
                self.next().await?;
            }
            Command::Prev => {
                self.prev().await?;
            }
            Command::Random => {
                self.random().await?;
            }
            Command::TogglePause => {
                self.toggle_pause().await?;
            }
            Command::Volume { value } => {
                self.set_volume(value).await?;
            }
            Command::SeekRelative { seconds } => {
                self.seek_relative(seconds).await?;
            }
            Command::SeekTo { seconds } => {
                self.seek_to(seconds).await?;
            }
            Command::GetState => {
                // State will be broadcast automatically
            }
        }
        Ok(())
    }
    
    async fn play_station(&self, idx: usize) -> anyhow::Result<()> {
        let (station, volume) = {
            let state = self.state_manager.get_state().await;
            (state.stations.get(idx).cloned(), state.volume)
        };

        if let Some(station) = station {
            info!("Playing station: {}", station.name);
            // Update state BEFORE loading so the monitor detects the station
            // change immediately and resets its connecting timer.
            self.intend_playing.store(true, Ordering::SeqCst);
            self.state_manager.set_playing(idx).await?;
            let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);

            let mut mpv = self.mpv.lock().await;
            if let Err(e) = mpv.load_stream(&station.url, volume).await {
                warn!("Failed to load stream '{}': {}", station.name, e);
                // Mark as error immediately instead of waiting for the monitor timeout.
                drop(mpv);
                self.intend_playing.store(false, Ordering::SeqCst);
                self.state_manager.set_playback_status(PlaybackStatus::Error).await;
                let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
            }
        }
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        info!("Stopping playback");
        self.intend_playing.store(false, Ordering::SeqCst);
        let mut mpv = self.mpv.lock().await;
        mpv.stop().await?;
        drop(mpv);
        self.state_manager.set_stopped().await?;
        let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
        Ok(())
    }

    async fn play_file(&self, path: String, start_secs: Option<f64>, pause_after: bool) -> anyhow::Result<()> {
        let volume = self.state_manager.get_state().await.volume;
        info!("Playing local file: {}", path);
        self.intend_playing.store(true, Ordering::SeqCst);
        self.state_manager.set_playing_file(path.clone()).await?;
        let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);

        let mut mpv = self.mpv.lock().await;
        if let Err(e) = mpv.load_stream(&path, volume).await {
            warn!("Failed to load file '{}': {}", path, e);
            drop(mpv);
            self.intend_playing.store(false, Ordering::SeqCst);
            self.state_manager.set_playback_status(PlaybackStatus::Error).await;
            let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
        } else {
            if let Some(start) = start_secs {
                if start > 0.0 {
                    if let Err(e) = mpv.seek_to(start).await {
                        warn!("Failed to seek file '{}' to {:.1}s: {}", path, start, e);
                    }
                }
            }
            if pause_after {
                let _ = mpv.set_pause(true).await;
            }
        }
        Ok(())
    }
    
    async fn next(&self) -> anyhow::Result<()> {
        self.state_manager.next_station().await?;
        if let Some(idx) = self.state_manager.get_state().await.current_station {
            self.play_station(idx).await?;
        }
        Ok(())
    }
    
    async fn prev(&self) -> anyhow::Result<()> {
        self.state_manager.prev_station().await?;
        if let Some(idx) = self.state_manager.get_state().await.current_station {
            self.play_station(idx).await?;
        }
        Ok(())
    }
    
    async fn random(&self) -> anyhow::Result<()> {
        self.state_manager.random_station().await?;
        if let Some(idx) = self.state_manager.get_state().await.current_station {
            self.play_station(idx).await?;
        }
        Ok(())
    }
    
    async fn set_volume(&self, value: f32) -> anyhow::Result<()> {
        self.state_manager.set_volume(value).await?;
        let mut mpv = self.mpv.lock().await;
        mpv.set_volume(value).await?;
        let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
        Ok(())
    }

    async fn seek_relative(&self, seconds: f64) -> anyhow::Result<()> {
        let state = self.state_manager.get_state().await;
        if state.current_file.is_none() {
            return Ok(());
        }
        let mut mpv = self.mpv.lock().await;
        mpv.seek_relative(seconds).await?;
        Ok(())
    }

    async fn toggle_pause(&self) -> anyhow::Result<()> {
        let state = self.state_manager.get_state().await;
        if state.current_station.is_none() && state.current_file.is_none() {
            return Ok(());
        }
        let mut mpv = self.mpv.lock().await;
        let paused = mpv.get_pause().await.unwrap_or(false);
        mpv.set_pause(!paused).await?;
        let _ = self.broadcast_tx.send(BroadcastMessage::StateUpdated);
        Ok(())
    }

    async fn seek_to(&self, seconds: f64) -> anyhow::Result<()> {
        let state = self.state_manager.get_state().await;
        if state.current_file.is_none() {
            return Ok(());
        }
        let mut mpv = self.mpv.lock().await;
        mpv.seek_to(seconds).await?;
        Ok(())
    }
    
    async fn load_stations(config: &Config) -> anyhow::Result<Vec<Station>> {
        // 1. Local TOML file (highest priority — rich metadata)
        let toml_path = &config.stations.stations_toml;
        if toml_path.exists() {
            match load_stations_from_toml(toml_path) {
                Ok(stations) => {
                    info!("Loaded {} stations from TOML: {}", stations.len(), toml_path.display());
                    return Ok(stations);
                }
                Err(e) => {
                    warn!("Failed to parse TOML stations ({}): {}", toml_path.display(), e);
                }
            }
        } else {
            info!("TOML stations file not found ({}), trying m3u", toml_path.display());
        }

        // 2. Also check for stations.toml in the working directory
        let local_toml = PathBuf::from("stations.toml");
        if local_toml.exists() {
            match load_stations_from_toml(&local_toml) {
                Ok(stations) => {
                    info!("Loaded {} stations from local stations.toml", stations.len());
                    return Ok(stations);
                }
                Err(e) => {
                    warn!("Failed to parse local stations.toml: {}", e);
                }
            }
        }

        // 3. m3u URL or local m3u file (fallback)
        let source = &config.stations.m3u_url;
        info!("Loading stations from m3u: {}", source);

        if source.starts_with("http://") || source.starts_with("https://") {
            match Self::fetch_m3u_url(source).await {
                Ok(stations) => {
                    info!("Loaded {} stations from URL", stations.len());
                    return Ok(stations);
                }
                Err(e) => {
                    warn!("Failed to fetch stations from URL ({}): {}", source, e);
                }
            }
        } else {
            let path = PathBuf::from(source);
            if path.exists() {
                match load_stations_from_m3u(&path) {
                    Ok(stations) => {
                        info!("Loaded {} stations from m3u file", stations.len());
                        return Ok(stations);
                    }
                    Err(e) => {
                        warn!("Failed to read m3u file ({}): {}", source, e);
                    }
                }
            }
        }

        // 4. Last-resort: local m3u files in working directory
        for filename in &["jamf_radios.m3u", "radios.m3u"] {
            let path = PathBuf::from(filename);
            if path.exists() {
                if let Ok(stations) = load_stations_from_m3u(&path) {
                    info!("Loaded {} stations from local fallback {}", stations.len(), filename);
                    return Ok(stations);
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
    
    fn start_mpv_monitor(&self) {
        let state_manager = self.state_manager.clone();
        let broadcast_tx = self.broadcast_tx.clone();
        let mpv = self.mpv.clone();
        let intend_playing = self.intend_playing.clone();

        tokio::spawn(async move {
            let mut last_icy: Option<String> = None;
            let mut last_status = PlaybackStatus::Idle;
            let mut last_source: (Option<usize>, Option<String>) = (None, None);
            let mut connecting_since: Option<tokio::time::Instant> = None;

            loop {
                // Adaptive poll rate: faster when connecting so we catch
                // playback start quickly; slower when idle or errored.
                let poll_interval = match last_status {
                    PlaybackStatus::Connecting => tokio::time::Duration::from_millis(1000),
                    PlaybackStatus::Playing    => tokio::time::Duration::from_secs(2),
                    PlaybackStatus::Error      => tokio::time::Duration::from_secs(5),
                    PlaybackStatus::Idle       => tokio::time::Duration::from_secs(3),
                };
                tokio::time::sleep(poll_interval).await;

                // Detect station change → reset connecting timer AND last_status.
                // set_playing() always writes Connecting into state, so the TUI
                // will already show Connecting.  We must reset last_status here
                // so that the very next poll — even if core-idle=false straight
                // away — is treated as a *new* transition and gets broadcast.
                let state_snapshot = state_manager.get_state().await;
                let current_source = (state_snapshot.current_station, state_snapshot.current_file.clone());
                if current_source != last_source {
                    info!("monitor: source changed {:?} → {:?}, resetting timer", last_source, current_source);
                    connecting_since = None;
                    last_source = current_source;
                    last_status = PlaybackStatus::Connecting;
                }

                let (core_idle, current_icy, time_pos_secs, duration_secs) = {
                    let mut guard = mpv.lock().await;
                    guard.poll_state().await
                };

                // Derive playback status from intent + actual mpv state
                let intent = intend_playing.load(Ordering::SeqCst);
                let status = if !intent {
                    connecting_since = None;
                    PlaybackStatus::Idle
                } else {
                    match core_idle {
                        Some(false) => {
                            // Audio is flowing — clear any connecting timeout
                            connecting_since = None;
                            PlaybackStatus::Playing
                        }
                        other => {
                            // core_idle = Some(true) or None (IPC failed)
                            let since = connecting_since
                                .get_or_insert_with(tokio::time::Instant::now);
                            let elapsed = since.elapsed().as_secs();
                            info!(
                                "monitor: waiting for playback — core_idle={:?} elapsed={}s",
                                other, elapsed
                            );
                            if elapsed >= 10 {
                                warn!(
                                    "monitor: no audio after {}s, marking Error (core_idle={:?})",
                                    elapsed, other
                                );
                                PlaybackStatus::Error
                            } else {
                                PlaybackStatus::Connecting
                            }
                        }
                    }
                };

                if status != last_status {
                    info!("monitor: status {:?} → {:?}", last_status, status);
                    last_status = status.clone();
                    state_manager.set_playback_status(status).await;
                    let _ = broadcast_tx.send(BroadcastMessage::StateUpdated);
                }

                state_manager.set_timeline(time_pos_secs, duration_secs).await;
                let _ = broadcast_tx.send(BroadcastMessage::StateUpdated);

                // Filter trivial/empty ICY titles before broadcasting
                let current_icy = current_icy.and_then(|t| {
                    let trimmed = t.trim().trim_matches('-').trim();
                    if trimmed.is_empty() { None } else { Some(t) }
                });

                if current_icy != last_icy {
                    info!("monitor: ICY title {:?} → {:?}", last_icy, current_icy);
                    last_icy = current_icy.clone();
                    state_manager.set_icy_title(current_icy.clone()).await;
                    let _ = broadcast_tx.send(BroadcastMessage::IcyUpdated(current_icy));
                }
            }
        });
    }
    
    async fn write_pid_file(&self) -> anyhow::Result<()> {
        let pid = std::process::id().to_string();
        if let Some(parent) = self.config.daemon.pid_file.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&self.config.daemon.pid_file, pid).await?;
        Ok(())
    }
    
    async fn cleanup(&self) -> anyhow::Result<()> {
        info!("Cleaning up daemon");
        
        // Stop MPV
        let mut mpv = self.mpv.lock().await;
        let _ = mpv.stop().await;
        drop(mpv);
        
        // Remove PID file
        let _ = tokio::fs::remove_file(&self.config.daemon.pid_file).await;
        
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Setup file logging
    let data_dir = radio_tui::shared::platform::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let log_path = data_dir.join("daemon.log");

    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    
    tracing_subscriber::fmt()
        .with_writer(log_file)
        .with_env_filter("info")
        .init();
    
    info!("Log file: {:?}", log_path);
    
    let config = Config::load()?;
    info!("Config loaded from: {:?}", Config::config_path());
    
    let (daemon, command_rx) = Daemon::new(config).await?;
    info!("Daemon initialized, starting...");
    
    daemon.run(command_rx).await?;
    
    Ok(())
}
