mod action;
mod app;
mod app_state;
mod component;
mod components;
mod core;
mod download_manager;
mod dsp;
mod focus;
mod http;
mod intent;
mod latency;
mod mpv;
mod nts_download;
mod pipewire_viz;
mod proxy;
mod scope;
mod theme;
mod widgets;
mod workspace;

use tokio::sync::{broadcast, mpsc};
use tracing::{error, info};

#[cfg(feature = "profiling")]
use pprof::ProfilerGuard;

/// Which subsystem produced the PCM data — determines jitter-buffer strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VizSourceKind {
    /// PipeWire/PulseAudio monitor: real-time, no network jitter.
    PipeWire,
    /// ffmpeg decoding a network stream: bursty, needs jitter absorption.
    Ffmpeg,
}

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
    /// `captured_at` records the instant the samples were captured/decoded.
    PcmChunk {
        samples: std::sync::Arc<Vec<f32>>,
        captured_at: std::time::Instant,
        source: VizSourceKind,
    },
    /// Reported audio-output latency from the viz source (microseconds).
    VizLatencyReport { latency_us: u64 },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── Start CPU profiling for entire session ────────────────────────────────
    #[cfg(feature = "profiling")]
    let mut profiler: Option<ProfilerGuard<'_>> = None;
    #[cfg(feature = "profiling")]
    {
        info!("Starting CPU profiler for entire session...");
        profiler = Some(ProfilerGuard::new(100)?);
    }

    let data_dir = radio_proto::platform::data_dir();
    // On Windows use data_dir() so we respect portable mode (exe_dir/data/).
    // On Unix keep the original radio-tui subdir for backwards compatibility.
    #[cfg(windows)]
    let tui_data_dir = data_dir.clone();
    #[cfg(not(windows))]
    let tui_data_dir = dirs::data_dir()
        .map(|p| p.join("radio-tui"))
        .unwrap_or_else(|| radio_proto::platform::temp_dir().join("radio-tui"));

    std::fs::create_dir_all(&data_dir)?;
    std::fs::create_dir_all(&tui_data_dir)?;

    let icy_log_path = data_dir.join("icyticker.log");

    let songs_csv_path = data_dir.join("songs.csv");
    let songs_vds_path = tui_data_dir.join("songs.vds");
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

    // ── Rolling log setup ─────────────────────────────────────────────────────
    // Rotate daily; keep 7 days of logs. tracing-appender produces files like:
    //   tui.log.2026-03-22
    // On startup delete any logs older than 7 days.
    cleanup_old_logs(&data_dir, 7);

    let file_appender = tracing_appender::rolling::daily(&data_dir, "tui.log");
    let (non_blocking, _log_guard) = tracing_appender::non_blocking(file_appender);

    // Current day's log path, passed to the log panel for tailing.
    let log_path = {
        let today = chrono::Local::now().format("%Y-%m-%d");
        data_dir.join(format!("tui.log.{}", today))
    };

    // Allow RUST_LOG override; default to debug for app code but suppress noisy
    // connection-level DEBUG from HTTP client internals (hyper_util, reqwest).
    let log_filter = std::env::var("RUST_LOG")
        .unwrap_or_else(|_| "debug,hyper_util=warn,reqwest=warn,hyper=warn".to_string());
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_env_filter(log_filter.as_str())
        .with_ansi(false)
        .init();

    // Print log path to stderr so the operator can tail it immediately.
    eprintln!("r4dio log: {}", log_path.display());

    tracing::info!("r4dio starting…");

    // ── Load config ──────────────────────────────────────────────────────────
    let config = radio_proto::config::Config::load().unwrap_or_default();
    
    // Configure whether to use system dependencies or bundled ones
    radio_proto::platform::set_use_system_deps(config.binaries.use_system_deps);

    // downloads_dir depends on config (portable path resolution done in config.rs)
    let downloads_dir = config.paths.downloads_dir.clone();

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
        config.polling.max_concurrency,
        config.polling.max_jobs_per_cycle,
    );
    app.run(broadcast_rx).await?;

    // ── Write CPU profiling flamegraph ────────────────────────────────────────
    #[cfg(feature = "profiling")]
    if let Some(profiler) = profiler {
        info!("Writing CPU profiling flamegraph...");
        match profiler.report().build() {
            Ok(report) => {
                let flamegraph_path = data_dir.join("flamegraph.svg");
                match std::fs::File::create(&flamegraph_path) {
                    Ok(file) => {
                        match report.flamegraph(file) {
                            Ok(_) => info!("Flamegraph written to {:?}", flamegraph_path),
                            Err(e) => error!("Failed to write flamegraph: {}", e),
                        }
                    }
                    Err(e) => error!("Failed to create flamegraph file: {}", e),
                }
            }
            Err(e) => error!("Failed to build profiling report: {}", e),
        }
    }

    Ok(())
}

/// Delete `tui.log.<date>` files in `dir` whose date suffix is older than `keep_days` days.
fn cleanup_old_logs(dir: &std::path::Path, keep_days: i64) {
    let cutoff = chrono::Local::now() - chrono::Duration::days(keep_days);
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // tracing-appender names files "tui.log.YYYY-MM-DD"
        if let Some(date_str) = name.strip_prefix("tui.log.") {
            if let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                let file_dt = date.and_hms_opt(0, 0, 0)
                    .and_then(|dt| dt.and_local_timezone(chrono::Local).single());
                if let Some(file_dt) = file_dt {
                    if file_dt < cutoff {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}
