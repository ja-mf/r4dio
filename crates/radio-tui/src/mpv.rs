/// mpv IPC driver with separated reader/writer tasks.
///
/// Architecture:
///
/// ```text
///   MpvDriver::spawn_and_connect()
///         │
///         ├── writer_task   ← receives MpvRequest via mpsc, serialises → socket
///         └── reader_task   ← reads JSON lines from socket
///                                ├── response (has request_id) → matched oneshot::Sender
///                                └── event / property-change   → event_tx channel
/// ```
///
/// Public API:
///   - `MpvHandle` — cheaply cloneable.  `send(cmd)` returns a `Future<Value>`.
///   - `MpvDriver` — owns the process, reconnects on death.
///
/// Platform notes:
/// - Unix:   Unix domain sockets
/// - Windows: Named pipes  \\.\pipe\<name>
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, info, warn};

#[cfg(unix)]
use tokio::net::UnixStream;

#[cfg(windows)]
use tokio::net::windows::named_pipe::ClientOptions;

// ── global request-id counter ─────────────────────────────────────────────────

static NEXT_REQ_ID: AtomicU64 = AtomicU64::new(1);

// ── observation property IDs ──────────────────────────────────────────────────

/// Fixed observe_property IDs.  We match on these in property-change events.
pub const OBS_CORE_IDLE: u64 = 1;
pub const OBS_PAUSE: u64 = 2;
pub const OBS_ICY_TITLE: u64 = 3;
pub const OBS_TIME_POS: u64 = 4;
pub const OBS_DURATION: u64 = 5;
/// Observation ID for `af-metadata/meter` (lavfi astats audio levels).
pub const OBS_AUDIO_LEVEL: u64 = 7;

// ── internal channel types ────────────────────────────────────────────────────

struct PendingRequest {
    req_id: u64,
    payload: String, // serialised JSON line (already has '\n')
    reply: oneshot::Sender<anyhow::Result<Value>>,
}

/// An mpv event / property-change that arrived unsolicited (no request_id).
#[derive(Debug, Clone)]
pub struct MpvEvent {
    pub raw: Value,
}

impl MpvEvent {
    /// Returns `Some((obs_id, data))` if this is a property-change event.
    pub fn as_property_change(&self) -> Option<(u64, &Value)> {
        if self.raw.get("event")?.as_str()? == "property-change" {
            let id = self.raw.get("id")?.as_u64()?;
            let data = self.raw.get("data").unwrap_or(&Value::Null);
            Some((id, data))
        } else {
            None
        }
    }

    /// Returns the event name, e.g. "end-file", "start-file", "file-loaded".
    pub fn event_name(&self) -> Option<&str> {
        self.raw.get("event")?.as_str()
    }
}

// ── public handle ─────────────────────────────────────────────────────────────

/// Cloneable handle to the mpv writer task.  Use `send()` to fire a command
/// and await the response.
#[derive(Clone)]
pub struct MpvHandle {
    tx: mpsc::Sender<PendingRequest>,
}

impl MpvHandle {
    pub async fn send(&self, command: Value) -> anyhow::Result<Value> {
        let req_id = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
        let msg = json!({ "command": command, "request_id": req_id });
        let mut raw = serde_json::to_string(&msg)?;
        raw.push('\n');

        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(PendingRequest {
                req_id,
                payload: raw,
                reply: reply_tx,
            })
            .await
            .map_err(|_| anyhow::anyhow!("mpv writer task gone"))?;

        tokio::time::timeout(tokio::time::Duration::from_secs(5), reply_rx)
            .await
            .map_err(|_| anyhow::anyhow!("mpv IPC timeout for req={}", req_id))?
            .map_err(|_| anyhow::anyhow!("mpv reply channel dropped req={}", req_id))?
    }
}

// ── driver ────────────────────────────────────────────────────────────────────

/// Owns the mpv child process and manages (re)connection.
///
/// After calling `connect()`, a `MpvHandle` + event channel are returned.
/// If the process dies, call `reconnect()` to get a fresh pair.
pub struct MpvDriver {
    pub socket_name: String,
    process: Option<tokio::process::Child>,
    pub last_volume: f32,
}

impl MpvDriver {
    pub fn new() -> Self {
        Self {
            socket_name: radio_proto::platform::mpv_socket_name(),
            process: None,
            last_volume: 0.5,
        }
    }

    pub fn process_alive(&mut self) -> bool {
        if let Some(ref mut child) = self.process {
            match child.try_wait() {
                Ok(None) => true, // Still running
                Ok(Some(status)) => {
                    // Process exited - log the status
                    if let Some(code) = status.code() {
                        warn!("mpv process exited with code: {}", code);
                    } else {
                        warn!("mpv process terminated by signal");
                    }
                    false
                }
                Err(e) => {
                    warn!("mpv process_alive check failed: {}", e);
                    false
                }
            }
        } else {
            false
        }
    }

    /// Kill the process if running.
    pub async fn kill(&mut self) {
        if let Some(mut p) = self.process.take() {
            let _ = p.kill().await;
        }
    }

    // ── spawn / reconnect ─────────────────────────────────────────────────────

    #[cfg(unix)]
    pub async fn spawn_and_connect(
        &mut self,
        event_tx: mpsc::Sender<MpvEvent>,
    ) -> anyhow::Result<MpvHandle> {
        // Kill stale process
        if let Some(mut p) = self.process.take() {
            let _ = p.kill().await;
        }

        let socket_path = std::path::PathBuf::from(&self.socket_name);
        let _ = tokio::fs::remove_file(&socket_path).await;

        info!("mpv: spawning new process");
        let mpv_binary = radio_proto::platform::find_mpv_binary()
            .ok_or_else(|| anyhow::anyhow!("mpv binary not found"))?;

        let vol_arg = format!(
            "--volume={}",
            (self.last_volume * 100.0).clamp(0.0, 100.0).round() as i64
        );
        let ipc_arg = radio_proto::platform::mpv_socket_arg();

        // Create mpv stderr log file for debugging crashes
        let stderr_path = radio_proto::platform::data_dir().join("mpv-stderr.log");
        let stderr_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stderr_path)?;
        info!("mpv: logging stderr to {:?}", stderr_path);

        let child = tokio::process::Command::new(&mpv_binary)
            .arg("--no-video")
            .arg("--idle=yes")
            .arg(&ipc_arg)
            .arg("--quiet")
            .arg(&vol_arg)
            .stdout(std::process::Stdio::null())
            .stderr(stderr_file)
            .spawn()?;
        info!("mpv: spawned process with pid {:?}", child.id());
        self.process = Some(child);

        // Wait for socket to appear
        for _ in 0..50 {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            if socket_path.exists() {
                break;
            }
        }
        if !socket_path.exists() {
            anyhow::bail!("mpv IPC socket did not appear");
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        let stream = UnixStream::connect(&socket_path).await?;
        info!("mpv: connected to IPC socket");
        Ok(Self::start_io_tasks(stream, event_tx))
    }

    /// Try to connect to an already-running mpv socket without spawning.
    #[cfg(unix)]
    pub async fn try_reconnect(&mut self, event_tx: mpsc::Sender<MpvEvent>) -> Option<MpvHandle> {
        let socket_path = std::path::PathBuf::from(&self.socket_name);
        if !socket_path.exists() {
            return None;
        }
        match UnixStream::connect(&socket_path).await {
            Ok(stream) => {
                info!("mpv: reconnected to existing IPC socket");
                Some(Self::start_io_tasks(stream, event_tx))
            }
            Err(e) => {
                warn!("mpv: failed to reconnect: {}", e);
                None
            }
        }
    }

    #[cfg(unix)]
    fn start_io_tasks(stream: UnixStream, event_tx: mpsc::Sender<MpvEvent>) -> MpvHandle {
        let (read_half, write_half) = stream.into_split();
        let reader = BufReader::new(read_half);

        // pending map: req_id → reply channel.  Shared between writer (inserts) and reader (resolves).
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<anyhow::Result<Value>>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let (cmd_tx, cmd_rx) = mpsc::channel::<PendingRequest>(64);

        // writer task
        let pending_w = pending.clone();
        tokio::spawn(writer_task(write_half, cmd_rx, pending_w));

        // reader task
        tokio::spawn(reader_task(reader, pending, event_tx));

        MpvHandle { tx: cmd_tx }
    }

    // ── Windows ───────────────────────────────────────────────────────────────

    #[cfg(windows)]
    pub async fn spawn_and_connect(
        &mut self,
        event_tx: mpsc::Sender<MpvEvent>,
    ) -> anyhow::Result<MpvHandle> {
        if let Some(mut p) = self.process.take() {
            let _ = p.kill().await;
        }

        info!("mpv: spawning new process");
        let mpv_binary = radio_proto::platform::find_mpv_binary()
            .ok_or_else(|| anyhow::anyhow!("mpv binary not found"))?;

        let vol_arg = format!(
            "--volume={}",
            (self.last_volume * 100.0).clamp(0.0, 100.0).round() as i64
        );
        let ipc_arg = radio_proto::platform::mpv_socket_arg();

        let child = tokio::process::Command::new(mpv_binary)
            .arg("--no-video")
            .arg("--idle=yes")
            .arg(&ipc_arg)
            .arg("--quiet")
            .arg(vol_arg)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        self.process = Some(child);

        let pipe_path = format!(r"\\.\pipe\{}", self.socket_name);
        for _ in 0..50 {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            match ClientOptions::new().open(&pipe_path) {
                Ok(client) => {
                    info!("mpv: connected to named pipe");
                    return Ok(Self::start_io_tasks_windows(client, event_tx));
                }
                Err(_) => continue,
            }
        }
        anyhow::bail!("mpv named pipe did not appear")
    }

    #[cfg(windows)]
    pub async fn try_reconnect(&mut self, event_tx: mpsc::Sender<MpvEvent>) -> Option<MpvHandle> {
        let pipe_path = format!(r"\\.\pipe\{}", self.socket_name);
        match ClientOptions::new().open(&pipe_path) {
            Ok(client) => {
                info!("mpv: reconnected to named pipe");
                Some(Self::start_io_tasks_windows(client, event_tx))
            }
            Err(e) => {
                warn!("mpv: failed to reconnect to named pipe: {}", e);
                None
            }
        }
    }

    #[cfg(windows)]
    fn start_io_tasks_windows(
        pipe: tokio::net::windows::named_pipe::NamedPipeClient,
        event_tx: mpsc::Sender<MpvEvent>,
    ) -> MpvHandle {
        use tokio::io::split;
        let (read_half, write_half) = split(pipe);
        let reader = BufReader::new(read_half);

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<anyhow::Result<Value>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (cmd_tx, cmd_rx) = mpsc::channel::<PendingRequest>(64);

        let pending_w = pending.clone();
        tokio::spawn(writer_task(write_half, cmd_rx, pending_w));
        tokio::spawn(reader_task(reader, pending, event_tx));

        MpvHandle { tx: cmd_tx }
    }
}

// ── reader task ───────────────────────────────────────────────────────────────

async fn reader_task<R>(
    mut reader: BufReader<R>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<anyhow::Result<Value>>>>>,
    event_tx: mpsc::Sender<MpvEvent>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                debug!("mpv reader: connection closed");
                // Fail all pending requests
                let mut map = pending.lock().await;
                for (_, tx) in map.drain() {
                    let _ = tx.send(Err(anyhow::anyhow!("mpv IPC connection closed")));
                }
                break;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let val: Value = match serde_json::from_str(trimmed) {
                    Ok(v) => v,
                    Err(e) => {
                        debug!("mpv reader: invalid json '{}': {}", trimmed, e);
                        continue;
                    }
                };

                if let Some(req_id) = val.get("request_id").and_then(|v| v.as_u64()) {
                    // This is a command response — route to pending request
                    let mut map = pending.lock().await;
                    if let Some(tx) = map.remove(&req_id) {
                        let result = if val["error"].as_str() == Some("success") {
                            debug!("mpv reader: response req={} ok", req_id);
                            Ok(val)
                        } else {
                            let err = val["error"].as_str().unwrap_or("unknown error").to_string();
                            debug!("mpv reader: response req={} err={}", req_id, err);
                            Err(anyhow::anyhow!("mpv error: {}", err))
                        };
                        let _ = tx.send(result);
                    } else {
                        debug!("mpv reader: response for unknown req={}", req_id);
                    }
                } else {
                    // Unsolicited event / property-change
                    debug!("mpv reader: event {}", trimmed);
                    let _ = event_tx.send(MpvEvent { raw: val }).await;
                }
            }
            Err(e) => {
                warn!("mpv reader: read error: {}", e);
                let mut map = pending.lock().await;
                for (_, tx) in map.drain() {
                    let _ = tx.send(Err(anyhow::anyhow!("mpv IPC read error: {}", e)));
                }
                break;
            }
        }
    }
}

// ── writer task ───────────────────────────────────────────────────────────────

async fn writer_task<W>(
    mut writer: W,
    mut rx: mpsc::Receiver<PendingRequest>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<anyhow::Result<Value>>>>>,
) where
    W: tokio::io::AsyncWrite + Unpin,
{
    while let Some(req) = rx.recv().await {
        // Register reply channel before writing so reader can match it
        {
            let mut map = pending.lock().await;
            map.insert(req.req_id, req.reply);
        }
        debug!(
            "mpv writer: send req={} payload={}",
            req.req_id,
            req.payload.trim()
        );
        if let Err(e) = writer.write_all(req.payload.as_bytes()).await {
            warn!("mpv writer: write error: {}", e);
            // Remove and fail the request we just registered
            let mut map = pending.lock().await;
            if let Some(tx) = map.remove(&req.req_id) {
                let _ = tx.send(Err(anyhow::anyhow!("mpv write error: {}", e)));
            }
            break;
        }
    }
    debug!("mpv writer: task exiting");
}

// ── convenience wrappers (used by DaemonCore) ─────────────────────────────────

impl MpvHandle {
    pub async fn load_stream(&self, url: &str, volume: f32) -> anyhow::Result<()> {
        debug!("mpv: sending loadfile command for url={}", url);
        let resp = self.send(json!(["loadfile", url])).await?;
        debug!("mpv: loadfile response: {:?}", resp);
        let vol_pct = (volume * 100.0).clamp(0.0, 100.0);
        let _ = self.send(json!(["set_property", "volume", vol_pct])).await;
        Ok(())
    }

    pub async fn stop(&self) -> anyhow::Result<()> {
        let _ = self.send(json!(["stop"])).await;
        Ok(())
    }

    pub async fn set_volume(&self, vol: f32) -> anyhow::Result<()> {
        let vol_pct = (vol * 100.0).clamp(0.0, 100.0);
        self.send(json!(["set_property", "volume", vol_pct]))
            .await?;
        Ok(())
    }

    pub async fn set_pause(&self, paused: bool) -> anyhow::Result<()> {
        self.send(json!(["set_property", "pause", paused])).await?;
        Ok(())
    }

    pub async fn get_pause(&self) -> anyhow::Result<bool> {
        match self.send(json!(["get_property", "pause"])).await {
            Ok(resp) => Ok(resp["data"].as_bool().unwrap_or(false)),
            Err(_) => Ok(false),
        }
    }

    pub async fn seek_to(&self, secs: f64) -> anyhow::Result<()> {
        self.send(json!(["set_property", "time-pos", secs])).await?;
        Ok(())
    }

    pub async fn seek_relative(&self, secs: f64) -> anyhow::Result<()> {
        self.send(json!(["seek", secs, "relative"])).await?;
        Ok(())
    }

    /// Register observe_property for all properties we care about.
    /// Must be called after every fresh connection (connect or reconnect).
    /// mpv will push property-change events whenever any of these change.
    /// Note: af-metadata/meter is NOT observed here — we poll it directly
    /// at high rate via get_property instead of relying on mpv's push rate.
    pub async fn observe_all_properties(&self) {
        let props = [
            (OBS_CORE_IDLE, "core-idle"),
            (OBS_PAUSE, "pause"),
            (OBS_ICY_TITLE, "metadata/by-key/icy-title"),
            (OBS_TIME_POS, "time-pos"),
            (OBS_DURATION, "duration"),
        ];
        for (id, name) in &props {
            match self.send(json!(["observe_property", id, name])).await {
                Ok(_) => debug!("mpv: observe_property id={} name={}", id, name),
                Err(e) => warn!("mpv: observe_property {} failed: {}", name, e),
            }
        }
        // Also observe icy-title directly (some mpv versions expose it here too)
        let _ = self
            .send(json!(["observe_property", 6u64, "icy-title"]))
            .await;
    }

    /// Install the lavfi astats audio filter so mpv exposes per-chunk RMS/peak
    /// levels via the `af-metadata/meter` property.
    ///
    /// - `metadata=1` — expose stats as AVFrame side-data (readable via af-metadata)
    /// - `reset=0`    — reset stats every audio frame (gives instantaneous level)
    /// - `length=0.02` — 20ms measurement window for fast response
    ///
    /// We poll `af-metadata/meter` via get_property at 50 Hz rather than relying
    /// on observe_property, so this filter just needs to produce fresh data each frame.
    pub async fn set_audio_filter(&self) {
        let filter = json!([{
            "name": "lavfi",
            "label": "meter",
            "params": { "graph": "astats=metadata=1:reset=0:length=0.02" }
        }]);
        match self.send(json!(["set_property", "af", filter])).await {
            Ok(_) => debug!("mpv: astats audio filter installed (reset=0, length=0.02)"),
            Err(e) => warn!("mpv: failed to set astats filter: {}", e),
        }
    }

    /// Health-check: returns Ok(()) if mpv is responsive.
    pub async fn ping(&self) -> anyhow::Result<()> {
        self.send(json!(["get_property", "volume"])).await?;
        Ok(())
    }
}

// ── Dedicated audio-level observer ───────────────────────────────────────────
//
// Opens a SECOND independent socket connection to mpv — used exclusively for
// streaming `af-metadata/meter` property-change events at full audio frame rate.
//
// This connection never sends any commands except the initial observe_property
// registration, so it can never interfere with play/stop/volume commands on the
// main handle.  mpv pushes one unsolicited JSON event per audio frame (~10ms at
// 44100 Hz / 512 samples), which we parse and forward to broadcast_tx.
//
// Callers: DaemonCore calls this immediately after spawn_and_connect succeeds.
// The returned JoinHandle can be aborted when mpv dies.

#[cfg(unix)]
pub fn spawn_audio_observer(
    socket_name: String,
    broadcast_tx: tokio::sync::broadcast::Sender<crate::BroadcastMessage>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Brief delay to let mpv finish initialising its socket.
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        loop {
            // Open a fresh independent connection.
            let stream = match UnixStream::connect(&socket_name).await {
                Ok(s) => s,
                Err(e) => {
                    warn!("audio_observer: connect failed: {}, retrying in 500ms", e);
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    continue;
                }
            };

            debug!("audio_observer: connected to {}", socket_name);

            let (read_half, mut write_half) = tokio::io::split(stream);
            let mut reader = BufReader::new(read_half);

            // Register observe_property for af-metadata/meter only.
            // Use observe ID 99 so it never collides with OBS_* on main handle.
            let cmd =
                "{\"command\":[\"observe_property\",99,\"af-metadata/meter\"],\"request_id\":0}\n";
            if let Err(e) = write_half.write_all(cmd.as_bytes()).await {
                warn!("audio_observer: failed to send observe_property: {}", e);
                continue;
            }

            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        debug!("audio_observer: socket closed by mpv");
                        break;
                    }
                    Err(e) => {
                        warn!("audio_observer: read error: {}", e);
                        break;
                    }
                    Ok(_) => {}
                }

                // We only care about property-change events for id=99.
                // Format: {"event":"property-change","id":99,"name":"af-metadata/meter","data":{...}}
                if !line.contains("\"property-change\"") {
                    continue;
                }

                if let Ok(val) = serde_json::from_str::<Value>(line.trim()) {
                    if val.get("id").and_then(|v| v.as_u64()) != Some(99) {
                        continue;
                    }
                    if let Some(obj) = val.get("data").and_then(|d| d.as_object()) {
                        let rms = obj
                            .get("lavfi.astats.Overall.RMS_level")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse::<f32>().ok())
                            .unwrap_or(-90.0);
                        // Clamp to sensible range; -inf can appear as -inf string
                        let rms = rms.max(-90.0).min(0.0);
                        let _ = broadcast_tx.send(crate::BroadcastMessage::AudioLevel(rms));
                    }
                }
            }

            // Socket closed — mpv died or restarted.  Wait before reconnecting;
            // DaemonCore will respawn mpv and call spawn_audio_observer again,
            // so we just exit here.
            debug!("audio_observer: exiting");
            break;
        }
    })
}

#[cfg(windows)]
pub fn spawn_audio_observer(
    socket_name: String,
    broadcast_tx: tokio::sync::broadcast::Sender<crate::BroadcastMessage>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        let pipe_path = format!(r"\\.\pipe\{}", socket_name);
        loop {
            let client = match ClientOptions::new().open(&pipe_path) {
                Ok(c) => c,
                Err(e) => {
                    warn!("audio_observer: pipe connect failed: {}, retrying", e);
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    continue;
                }
            };

            debug!("audio_observer: connected to {}", pipe_path);

            let (read_half, mut write_half) = tokio::io::split(client);
            let mut reader = BufReader::new(read_half);

            let cmd =
                "{\"command\":[\"observe_property\",99,\"af-metadata/meter\"],\"request_id\":0}\n";
            if let Err(e) = write_half.write_all(cmd.as_bytes()).await {
                warn!("audio_observer: write error: {}", e);
                continue;
            }

            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }

                if !line.contains("\"property-change\"") {
                    continue;
                }

                if let Ok(val) = serde_json::from_str::<Value>(line.trim()) {
                    if val.get("id").and_then(|v| v.as_u64()) != Some(99) {
                        continue;
                    }
                    if let Some(obj) = val.get("data").and_then(|d| d.as_object()) {
                        let rms = obj
                            .get("lavfi.astats.Overall.RMS_level")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse::<f32>().ok())
                            .unwrap_or(-90.0);
                        let rms = rms.max(-90.0).min(0.0);
                        let _ = broadcast_tx.send(crate::BroadcastMessage::AudioLevel(rms));
                    }
                }
            }

            debug!("audio_observer: exiting");
            break;
        }
    })
}
