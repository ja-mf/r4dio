mod core;
mod http;
mod mpv;
mod proxy;
mod socket;

use radio_proto::config::Config;
use tokio::sync::broadcast;
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Debug, Clone)]
pub enum BroadcastMessage {
    StateUpdated,
    IcyUpdated(Option<String>),
    Log(String),
    /// Real-time audio RMS level in dBFS.
    AudioLevel(f32),
    /// Raw PCM samples (mono f32 normalised -1..1, 11025 Hz) for scope display.
    /// Sent in chunks of VU_WINDOW_SAMPLES (512) alongside every AudioLevel.
    PcmChunk(std::sync::Arc<Vec<f32>>),
}

/// A custom tracing layer that forwards log messages to the broadcast channel
struct BroadcastLayer {
    sender: broadcast::Sender<BroadcastMessage>,
}

impl BroadcastLayer {
    fn new(sender: broadcast::Sender<BroadcastMessage>) -> Self {
        Self { sender }
    }
}

impl<S> tracing_subscriber::Layer<S> for BroadcastLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        // Only forward WARN and ERROR to TUI to avoid clogging the channel
        let level = event.metadata().level();
        if !matches!(*level, tracing::Level::WARN | tracing::Level::ERROR) {
            return;
        }
        
        // Format the log message
        let mut message = String::new();
        
        // Add timestamp
        let now = chrono::Local::now();
        message.push_str(&format!("{} ", now.format("%H:%M:%S")));
        
        // Add level
        message.push_str(&format!("[{}] ", level));
        
        // Add the message
        let mut visitor = MessageVisitor(&mut message);
        event.record(&mut visitor);
        
        // Send to broadcast channel (ignore errors - no receivers is OK)
        let _ = self.sender.send(BroadcastMessage::Log(message));
    }
}

struct MessageVisitor<'a>(&'a mut String);

impl<'a> tracing::field::Visit for MessageVisitor<'a> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0.push_str(&format!("{:?}", value));
        } else {
            self.0.push_str(&format!(" {}={:?}", field.name(), value));
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Setup broadcast channel first so we can use it for logging
    let (broadcast_tx, _) = broadcast::channel::<BroadcastMessage>(100);

    // Setup file logging + broadcast layer
    let data_dir = radio_proto::platform::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let log_path = data_dir.join("daemon.log");

    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    // Create layers: file writer + broadcast
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(log_file)
        .with_ansi(false);
    
    let broadcast_layer = BroadcastLayer::new(broadcast_tx.clone());

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(broadcast_layer)
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,radio_daemon=debug")),
        )
        .init();

    info!("Log file: {:?}", log_path);

    let config = Config::load()?;
    info!("Config loaded from: {:?}", Config::config_path());

    // Event channel — all external inputs funnel into DaemonCore
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<core::DaemonEvent>(256);

    // Build DaemonCore (loads stations, initialises mpv driver)
    let daemon_core = core::DaemonCore::new(
        config.clone(),
        broadcast_tx.clone(),
        event_tx.clone(),
    )
    .await?;

    let state_manager = daemon_core.state_manager();

    // Client list for socket server shutdown detection
    let clients = std::sync::Arc::new(tokio::sync::RwLock::new(
        Vec::<socket::ClientHandle>::new(),
    ));

    // Start TCP socket server
    let _socket_handle = socket::start_server(
        config.http.bind_address.clone(),
        radio_proto::platform::DAEMON_TCP_PORT,
        state_manager.clone(),
        clients.clone(),
        event_tx.clone(),
        broadcast_tx.clone(),
    );

    // Start HTTP API if enabled
    if config.http.enabled {
        let _http_handle = http::start_server(
            config.http.bind_address.clone(),
            config.http.port,
            state_manager.clone(),
            event_tx.clone(),
        );
    }

    // Start stream proxy (always on — mpv is directed here for station playback)
    let _proxy_handle = proxy::start_server(
        config.http.bind_address.clone(),
        proxy::PROXY_PORT,
        state_manager.arc(),
    );

    info!("Daemon initialised, running event loop");
    daemon_core.run(event_rx).await?;

    Ok(())
}
