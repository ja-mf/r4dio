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
use tracing::error;

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
    let cli_verbose = std::env::args().skip(1).any(|arg| arg == "--verbose" || arg == "-v");

    // ── Load config early (logging config is needed before subscriber init) ───
    let config = radio_proto::config::Config::load().unwrap_or_default();

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
    // Rotate daily; additionally enforce a total size cap via startup+periodic pruning.
    prune_logs_to_size_cap(&data_dir, None, config.logging.max_total_size_mb);

    let file_appender = tracing_appender::rolling::daily(&data_dir, "tui.log");
    let (non_blocking, _log_guard) = tracing_appender::non_blocking(file_appender);

    // Current day's log path, passed to the log panel for tailing.
    let log_path = {
        let today = chrono::Local::now().format("%Y-%m-%d");
        data_dir.join(format!("tui.log.{}", today))
    };
    let active_log = log_path.clone();

    // Precedence: CLI --verbose/-v > RUST_LOG > config.logging.verbose > default.
    // Default stays concise at info-level for app code.
    let log_filter = if cli_verbose {
        "debug,hyper_util=warn,reqwest=warn,hyper=warn".to_string()
    } else if let Ok(env_filter) = std::env::var("RUST_LOG") {
        env_filter
    } else if config.logging.verbose {
        "debug,hyper_util=warn,reqwest=warn,hyper=warn".to_string()
    } else {
        "info,hyper_util=warn,reqwest=warn,hyper=warn".to_string()
    };
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_env_filter(log_filter.as_str())
        .with_ansi(false)
        .init();

    // Print log path to stderr so the operator can tail it immediately.
    eprintln!("r4dio log: {}", log_path.display());
    if cli_verbose {
        eprintln!("r4dio logging: verbose enabled via CLI");
    }

    tracing::info!("r4dio starting…");

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
        log_path.clone(),
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

    // Periodic size-based log cleanup while process stays alive.
    let cleanup_dir = data_dir.clone();
    let cleanup_interval_secs = config.logging.cleanup_interval_secs.max(10);
    let max_log_mb = config.logging.max_total_size_mb;
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(cleanup_interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            prune_logs_to_size_cap(&cleanup_dir, Some(&active_log), max_log_mb);
        }
    });

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

fn is_managed_log_file(name: &str) -> bool {
    name == "mpv-stderr.log" || name.starts_with("tui.log.")
}

fn prune_logs_to_size_cap(
    dir: &std::path::Path,
    active_log_path: Option<&std::path::Path>,
    max_total_size_mb: u64,
) {
    let cap_bytes = max_total_size_mb.saturating_mul(1024 * 1024);
    if cap_bytes == 0 {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    let mut files = Vec::<(std::path::PathBuf, u64, std::time::SystemTime)>::new();
    let mut total_bytes = 0u64;

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if !is_managed_log_file(&name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let size = meta.len();
        let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        total_bytes = total_bytes.saturating_add(size);
        files.push((path, size, modified));
    }

    if total_bytes <= cap_bytes {
        return;
    }

    files.sort_by_key(|(_, _, modified)| *modified);
    for (path, size, _) in files {
        if total_bytes <= cap_bytes {
            break;
        }
        if active_log_path.map(|p| p == path.as_path()).unwrap_or(false) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            total_bytes = total_bytes.saturating_sub(size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::prune_logs_to_size_cap;
    use std::io::Write;

    #[test]
    fn prune_logs_oldest_first_keeps_active() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p1 = dir.path().join("tui.log.2026-01-01");
        let p2 = dir.path().join("tui.log.2026-01-02");
        let active = dir.path().join("tui.log.2026-01-03");

        for (path, bytes) in [
            (&p1, 900_000usize),
            (&p2, 900_000usize),
            (&active, 900_000usize),
        ] {
            let mut f = std::fs::File::create(path).expect("create log");
            f.write_all(&vec![b'x'; bytes]).expect("write log");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        prune_logs_to_size_cap(dir.path(), Some(active.as_path()), 2);

        assert!(!p1.exists());
        assert!(p2.exists());
        assert!(active.exists());
    }

    #[test]
    fn prune_logs_noop_when_under_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p1 = dir.path().join("tui.log.2026-01-01");
        let mut f = std::fs::File::create(&p1).expect("create log");
        f.write_all(&vec![b'x'; 256]).expect("write log");

        prune_logs_to_size_cap(dir.path(), None, 1);

        assert!(p1.exists());
    }
}
