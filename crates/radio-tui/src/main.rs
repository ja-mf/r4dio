mod action;
mod app;
mod app_state;
mod component;
mod components;
mod core;
mod download_manager;
mod focus;
mod http;
mod intent;
mod mpv;
mod nts_download;
mod proxy;
mod scope;
mod theme;
mod widgets;
mod workspace;

use tokio::sync::{broadcast, mpsc};

/// Forwarded from daemon's main.rs — defines what the DaemonCore broadcasts.
#[derive(Debug, Clone)]
pub enum BroadcastMessage {
    /// The full DaemonState has changed; receivers should fetch from StateManager.
    StateUpdated,
    /// The ICY metadata title changed (None = cleared).
    IcyUpdated(Option<String>),
    /// A log message from the core event loop.
    Log(String),
    /// RMS audio level (dBFS) from the lavfi astats filter.
    AudioLevel(f32),
    /// Raw PCM samples (mono f32 normalised -1..1, 44100 Hz) for scope display.
    PcmChunk(std::sync::Arc<Vec<f32>>),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let data_dir = radio_proto::platform::data_dir();
    // Keep tui_data_dir consistent with original path for backwards compatibility.
    let tui_data_dir = dirs::data_dir()
        .map(|p| p.join("radio-tui"))
        .unwrap_or_else(|| radio_proto::platform::temp_dir().join("radio-tui"));

    std::fs::create_dir_all(&data_dir)?;
    std::fs::create_dir_all(&tui_data_dir)?;

    let log_path = data_dir.join("tui.log");
    let icy_log_path = data_dir.join("icyticker.log");

    let songs_csv_path = dirs::home_dir()
        .map(|p| p.join("songs.csv"))
        .unwrap_or_else(|| radio_proto::platform::temp_dir().join("songs.csv"));
    let songs_vds_path = tui_data_dir.join("songs.vds");
    let downloads_dir = dirs::home_dir()
        .map(|p| p.join("nts-downloads"))
        .unwrap_or_else(|| radio_proto::platform::temp_dir().join("nts-downloads"));
    let stars_path = tui_data_dir.join("starred.toml");
    // Seed starred.toml on first run from beside-exe (or data/ subdir) for bundled packages
    if !stars_path.exists() {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let candidates = [
                    dir.join("starred.toml"),
                    dir.join("data").join("starred.toml"),
                ];
                for seed in &candidates {
                    if seed.exists() {
                        let _ = std::fs::copy(seed, &stars_path);
                        break;
                    }
                }
            }
        }
    }
    let random_history_path = tui_data_dir.join("random_history.json");
    let recent_path = tui_data_dir.join("recent.toml");
    let file_positions_path = tui_data_dir.join("file_positions.toml");
    let ui_state_path = tui_data_dir.join("ui_state.json");

    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    // Allow RUST_LOG override; default to debug for app code but suppress noisy
    // connection-level DEBUG from HTTP client internals (hyper_util, reqwest).
    let log_filter = std::env::var("RUST_LOG")
        .unwrap_or_else(|_| "debug,hyper_util=warn,reqwest=warn,hyper=warn".to_string());
    tracing_subscriber::fmt()
        .with_writer(log_file)
        .with_env_filter(log_filter.as_str())
        .with_ansi(false)
        .init();

    // Print log path to stderr so the operator can tail it immediately.
    eprintln!("r4dio log: {}", log_path.display());

    tracing::info!("r4dio starting…");

    // ── Load config ──────────────────────────────────────────────────────────
    let config = radio_proto::config::Config::load().unwrap_or_default();

    // ── Broadcast channel (DaemonCore → TUI) ────────────────────────────────
    let (broadcast_tx, broadcast_rx) = broadcast::channel::<BroadcastMessage>(1024);

    // ── DaemonEvent channel (TUI/HTTP → DaemonCore) ─────────────────────────
    let (event_tx, event_rx) = mpsc::channel::<core::DaemonEvent>(1024);

    // ── Build DaemonCore ─────────────────────────────────────────────────────
    let daemon_core =
        core::DaemonCore::new(config.clone(), broadcast_tx.clone(), event_tx.clone()).await?;
    let state_manager = daemon_core.state_manager();

    // ── Stream proxy for station playback + visual tap ───────────────────────
    proxy::start_server(state_manager.clone());

    // ── HTTP server ──────────────────────────────────────────────────────────
    if config.http.enabled {
        http::start_server(
            config.http.bind_address.clone(),
            config.http.port,
            state_manager.clone(),
            event_tx.clone(),
        );
    }

    // ── Send initial state to TUI so stations appear immediately ────────────
    // The broadcast channel only carries deltas; the TUI has no TCP handshake
    // to fetch an initial Hello any more, so we push one StateUpdated now.
    let _ = broadcast_tx.send(BroadcastMessage::StateUpdated);

    // ── Spawn DaemonCore event loop ──────────────────────────────────────────
    tokio::spawn(async move {
        if let Err(e) = daemon_core.run(event_rx).await {
            tracing::error!("DaemonCore exited with error: {}", e);
        }
    });

    // ── Run TUI ──────────────────────────────────────────────────────────────
    let app = app::App::new(
        icy_log_path,
        songs_csv_path,
        songs_vds_path,
        log_path,
        stars_path,
        random_history_path,
        recent_path,
        file_positions_path,
        ui_state_path,
        downloads_dir,
        event_tx,
        state_manager,
        config.polling.auto_polling,
        config.polling.poll_interval_secs,
    );
    app.run(broadcast_rx).await?;

    Ok(())
}
