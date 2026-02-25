/// Custom mpv IPC client.
///
/// mpv sends asynchronous event lines (e.g. property-change, end-file) on the
/// same socket connection as command responses.  Simple line-read loops get
/// confused by interleaved events.  This implementation matches every response
/// by the `request_id` we sent, skipping event lines until the right one
/// arrives.
///
/// Platform notes:
/// - Unix: Uses Unix domain sockets
/// - Windows: Uses named pipes (\\.\pipe\<name>)
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, info, warn};

#[cfg(unix)]
use tokio::net::UnixStream;

#[cfg(windows)]
use tokio::net::windows::named_pipe::ClientOptions;

static NEXT_REQ_ID: AtomicU64 = AtomicU64::new(1);

pub struct MpvController {
    socket_name: String,
    process: Option<tokio::process::Child>,
    #[cfg(unix)]
    reader: Option<BufReader<tokio::net::unix::OwnedReadHalf>>,
    #[cfg(unix)]
    writer: Option<tokio::net::unix::OwnedWriteHalf>,
    #[cfg(windows)]
    pipe: Option<tokio::net::windows::named_pipe::NamedPipeClient>,
    last_volume: f32,
}

impl MpvController {
    pub async fn new() -> anyhow::Result<Self> {
        let socket_name = radio_tui::shared::platform::mpv_socket_name();
        Ok(Self {
            socket_name,
            process: None,
            #[cfg(unix)]
            reader: None,
            #[cfg(unix)]
            writer: None,
            #[cfg(windows)]
            pipe: None,
            last_volume: 0.5,
        })
    }

    fn connected(&self) -> bool {
        #[cfg(unix)]
        {
            self.writer.is_some()
        }
        #[cfg(windows)]
        {
            self.pipe.is_some()
        }
    }

    fn disconnect(&mut self) {
        #[cfg(unix)]
        {
            self.reader = None;
            self.writer = None;
        }
        #[cfg(windows)]
        {
            self.pipe = None;
        }
    }

    #[cfg(unix)]
    async fn try_reconnect(&mut self) -> bool {
        let socket_path = std::path::PathBuf::from(&self.socket_name);
        if !socket_path.exists() {
            return false;
        }
        match UnixStream::connect(&socket_path).await {
            Ok(stream) => {
                info!("mpv: reconnected to existing IPC socket");
                let (r, w) = stream.into_split();
                self.reader = Some(BufReader::new(r));
                self.writer = Some(w);
                true
            }
            Err(e) => {
                warn!("mpv: failed to reconnect to IPC socket: {}", e);
                false
            }
        }
    }

    #[cfg(windows)]
    async fn try_reconnect(&mut self) -> bool {
        let pipe_path = format!(r"\\.\pipe\{}", self.socket_name);
        match ClientOptions::new().open(&pipe_path) {
            Ok(client) => {
                info!("mpv: reconnected to existing named pipe");
                self.pipe = Some(client);
                true
            }
            Err(e) => {
                warn!("mpv: failed to reconnect to named pipe: {}", e);
                false
            }
        }
    }

    #[cfg(unix)]
    async fn spawn_and_connect(&mut self) -> anyhow::Result<()> {
        if let Some(mut p) = self.process.take() {
            let _ = p.kill().await;
        }
        self.disconnect();

        let socket_path = std::path::PathBuf::from(&self.socket_name);
        let _ = tokio::fs::remove_file(&socket_path).await;

        info!("mpv: spawning new process");
        let mpv_binary = radio_tui::shared::platform::find_mpv_binary()
            .ok_or_else(|| anyhow::anyhow!("mpv binary not found"))?;

        let vol_arg = format!(
            "--volume={}",
            (self.last_volume * 100.0).clamp(0.0, 100.0).round() as i64
        );
        let ipc_arg = radio_tui::shared::platform::mpv_socket_arg();

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
        let (r, w) = stream.into_split();
        self.reader = Some(BufReader::new(r));
        self.writer = Some(w);

        info!("mpv: connected to IPC socket");
        Ok(())
    }

    #[cfg(windows)]
    async fn spawn_and_connect(&mut self) -> anyhow::Result<()> {
        if let Some(mut p) = self.process.take() {
            let _ = p.kill().await;
        }
        self.disconnect();

        info!("mpv: spawning new process");
        let mpv_binary = radio_tui::shared::platform::find_mpv_binary()
            .ok_or_else(|| anyhow::anyhow!("mpv binary not found"))?;

        let vol_arg = format!(
            "--volume={}",
            (self.last_volume * 100.0).clamp(0.0, 100.0).round() as i64
        );
        let ipc_arg = radio_tui::shared::platform::mpv_socket_arg();

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
                    self.pipe = Some(client);
                    info!("mpv: connected to named pipe");
                    return Ok(());
                }
                Err(_) => continue,
            }
        }

        anyhow::bail!("mpv named pipe did not appear")
    }

    async fn ensure_running(&mut self) -> anyhow::Result<()> {
        if let Some(ref mut child) = self.process {
            if let Ok(Some(status)) = child.try_wait() {
                warn!("mpv: process exited unexpectedly ({}), will respawn", status);
                self.process = None;
                self.disconnect();
            }
        }

        if !self.connected() {
            if !self.try_reconnect().await {
                self.spawn_and_connect().await?;
            }
            return Ok(());
        }

        match tokio::time::timeout(
            tokio::time::Duration::from_secs(3),
            self.ipc(json!(["get_property", "volume"])),
        )
        .await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => {
                warn!("mpv: not responsive ({}), respawning", e);
                self.spawn_and_connect().await
            }
            Err(_) => {
                warn!("mpv: health-check timed out, respawning");
                self.disconnect();
                self.spawn_and_connect().await
            }
        }
    }

    #[cfg(unix)]
    async fn ipc(&mut self, command: Value) -> anyhow::Result<Value> {
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("mpv not connected"))?;
        let reader = self
            .reader
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("mpv not connected"))?;

        let req_id = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
        let msg = json!({ "command": command, "request_id": req_id });
        let mut raw = serde_json::to_string(&msg)?;
        raw.push('\n');
        debug!("mpv ipc send: req={} cmd={}", req_id, command);
        writer.write_all(raw.as_bytes()).await?;

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
        let mut skipped_events = 0usize;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                self.disconnect();
                anyhow::bail!(
                    "mpv ipc timeout waiting for response req={} (skipped {} events)",
                    req_id,
                    skipped_events
                );
            }

            let mut line = String::new();
            match tokio::time::timeout(remaining, reader.read_line(&mut line)).await {
                Ok(Ok(0)) => {
                    self.disconnect();
                    anyhow::bail!("mpv ipc: connection closed");
                }
                Ok(Ok(_)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let resp: Value = match serde_json::from_str(trimmed) {
                        Ok(v) => v,
                        Err(e) => {
                            debug!("mpv ipc: invalid json line: {} ({})", trimmed, e);
                            continue;
                        }
                    };

                    if resp.get("request_id").and_then(|v| v.as_u64()) == Some(req_id) {
                        debug!("mpv ipc recv: req={} resp={}", req_id, resp);
                        if resp["error"].as_str() == Some("success") {
                            return Ok(resp);
                        } else {
                            let err = resp["error"]
                                .as_str()
                                .unwrap_or("unknown error")
                                .to_string();
                            anyhow::bail!("mpv error: {}", err);
                        }
                    } else {
                        skipped_events += 1;
                        debug!("mpv ipc skip event: {}", trimmed);
                    }
                }
                Ok(Err(e)) => {
                    self.disconnect();
                    anyhow::bail!("mpv ipc read error: {}", e);
                }
                Err(_) => {
                    self.disconnect();
                    anyhow::bail!("mpv ipc timeout");
                }
            }
        }
    }

    #[cfg(windows)]
    async fn ipc(&mut self, command: Value) -> anyhow::Result<Value> {
        use tokio::io::AsyncReadExt;

        let pipe = self
            .pipe
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("mpv not connected"))?;

        let req_id = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
        let msg = json!({ "command": command, "request_id": req_id });
        let mut raw = serde_json::to_string(&msg)?;
        raw.push('\n');
        debug!("mpv ipc send: req={} cmd={}", req_id, command);
        pipe.write_all(raw.as_bytes()).await?;

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
        let mut skipped_events = 0usize;
        let mut line_buf = Vec::new();

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                self.disconnect();
                anyhow::bail!(
                    "mpv ipc timeout waiting for response req={} (skipped {} events)",
                    req_id,
                    skipped_events
                );
            }

            let mut byte = [0u8; 1];
            match tokio::time::timeout(remaining, pipe.read(&mut byte)).await {
                Ok(Ok(0)) => {
                    self.disconnect();
                    anyhow::bail!("mpv ipc: connection closed");
                }
                Ok(Ok(1)) => {
                    if byte[0] == b'\n' {
                        let line = String::from_utf8_lossy(&line_buf);
                        let trimmed = line.trim().to_string();
                        line_buf.clear();

                        if trimmed.is_empty() {
                            continue;
                        }

                        let resp: Value = match serde_json::from_str(&trimmed) {
                            Ok(v) => v,
                            Err(e) => {
                                debug!("mpv ipc: invalid json line: {} ({})", trimmed, e);
                                continue;
                            }
                        };

                        if resp.get("request_id").and_then(|v| v.as_u64()) == Some(req_id) {
                            debug!("mpv ipc recv: req={} resp={}", req_id, resp);
                            if resp["error"].as_str() == Some("success") {
                                return Ok(resp);
                            } else {
                                let err = resp["error"]
                                    .as_str()
                                    .unwrap_or("unknown error")
                                    .to_string();
                                anyhow::bail!("mpv error: {}", err);
                            }
                        } else {
                            skipped_events += 1;
                            debug!("mpv ipc skip event: {}", trimmed);
                        }
                    } else {
                        line_buf.push(byte[0]);
                    }
                }
                Ok(Ok(_)) => {
                    // Should not happen with 1-byte buffer, but handle gracefully
                    continue;
                }
                Ok(Err(e)) => {
                    self.disconnect();
                    anyhow::bail!("mpv ipc read error: {}", e);
                }
                Err(_) => {
                    self.disconnect();
                    anyhow::bail!("mpv ipc timeout");
                }
            }
        }
    }

    pub async fn load_url(&mut self, url: &str) -> anyhow::Result<()> {
        self.ensure_running().await?;
        self.ipc(json!(["loadfile", url])).await?;
        Ok(())
    }

    pub async fn play(&mut self) -> anyhow::Result<()> {
        self.ensure_running().await?;
        self.ipc(json!(["set_property", "pause", false])).await?;
        Ok(())
    }

    pub async fn pause(&mut self) -> anyhow::Result<()> {
        self.ensure_running().await?;
        self.ipc(json!(["set_property", "pause", true])).await?;
        Ok(())
    }

    pub async fn toggle_pause(&mut self) -> anyhow::Result<()> {
        self.ensure_running().await?;
        self.ipc(json!(["cycle", "pause"])).await?;
        Ok(())
    }

    pub async fn stop(&mut self) -> anyhow::Result<()> {
        if !self.connected() {
            return Ok(());
        }
        let _ = self.ipc(json!(["stop"])).await;
        Ok(())
    }

    pub async fn set_volume(&mut self, vol: f32) -> anyhow::Result<()> {
        self.last_volume = vol.clamp(0.0, 1.0);
        self.ensure_running().await?;
        let vol_pct = (self.last_volume * 100.0).clamp(0.0, 100.0);
        self.ipc(json!(["set_property", "volume", vol_pct])).await?;
        Ok(())
    }

    pub async fn get_volume(&mut self) -> anyhow::Result<f32> {
        self.ensure_running().await?;
        let resp = self.ipc(json!(["get_property", "volume"])).await?;
        let vol = resp["data"]
            .as_f64()
            .unwrap_or(50.0) as f32
            / 100.0;
        self.last_volume = vol;
        Ok(vol)
    }

    pub async fn seek(&mut self, secs: f64) -> anyhow::Result<()> {
        self.ensure_running().await?;
        self.ipc(json!(["seek", secs, "absolute"])).await?;
        Ok(())
    }

    pub async fn get_position(&mut self) -> anyhow::Result<Option<f64>> {
        if !self.connected() {
            return Ok(None);
        }
        match self.ipc(json!(["get_property", "time-pos"])).await {
            Ok(resp) => Ok(resp["data"].as_f64()),
            Err(_) => Ok(None),
        }
    }

    pub async fn get_duration(&mut self) -> anyhow::Result<Option<f64>> {
        if !self.connected() {
            return Ok(None);
        }
        match self.ipc(json!(["get_property", "duration"])).await {
            Ok(resp) => Ok(resp["data"].as_f64()),
            Err(_) => Ok(None),
        }
    }

    pub async fn get_pause_state(&mut self) -> anyhow::Result<bool> {
        if !self.connected() {
            return Ok(false);
        }
        match self.ipc(json!(["get_property", "pause"])).await {
            Ok(resp) => Ok(resp["data"].as_bool().unwrap_or(false)),
            Err(_) => Ok(false),
        }
    }

    pub async fn get_media_title(&mut self) -> anyhow::Result<Option<String>> {
        if !self.connected() {
            return Ok(None);
        }
        match self.ipc(json!(["get_property", "media-title"])).await {
            Ok(resp) => Ok(resp["data"].as_str().map(|s| s.to_string())),
            Err(_) => Ok(None),
        }
    }

    pub async fn get_path(&mut self) -> anyhow::Result<Option<String>> {
        if !self.connected() {
            return Ok(None);
        }
        match self.ipc(json!(["get_property", "path"])).await {
            Ok(resp) => Ok(resp["data"].as_str().map(|s| s.to_string())),
            Err(_) => Ok(None),
        }
    }

    pub fn process_alive(&mut self) -> bool {
        if let Some(ref mut child) = self.process {
            child.try_wait().ok().flatten().is_none()
        } else {
            false
        }
    }

    pub async fn kill(&mut self) {
        self.disconnect();
        if let Some(mut p) = self.process.take() {
            let _ = p.kill().await;
        }
    }

    // ── Convenience methods used by daemon ───────────────────────────────────

    pub async fn load_stream(&mut self, url: &str, volume: f32) -> anyhow::Result<()> {
        self.last_volume = volume;
        self.ensure_running().await?;
        self.ipc(json!(["loadfile", url])).await?;
        let vol_pct = (volume * 100.0).clamp(0.0, 100.0);
        let _ = self.ipc(json!(["set_property", "volume", vol_pct])).await;
        Ok(())
    }

    pub async fn seek_to(&mut self, secs: f64) -> anyhow::Result<()> {
        self.ensure_running().await?;
        self.ipc(json!(["set_property", "time-pos", secs])).await?;
        Ok(())
    }

    pub async fn seek_relative(&mut self, secs: f64) -> anyhow::Result<()> {
        self.ensure_running().await?;
        self.ipc(json!(["seek", secs, "relative"])).await?;
        Ok(())
    }

    pub async fn set_pause(&mut self, paused: bool) -> anyhow::Result<()> {
        self.ensure_running().await?;
        self.ipc(json!(["set_property", "pause", paused])).await?;
        Ok(())
    }

    pub async fn get_pause(&mut self) -> anyhow::Result<bool> {
        self.get_pause_state().await
    }

    pub fn set_last_volume(&mut self, vol: f32) {
        self.last_volume = vol;
    }

    pub async fn poll_state(&mut self) -> (Option<bool>, Option<String>, Option<f64>, Option<f64>) {
        if !self.connected() {
            return (None, None, None, None);
        }

        let core_idle = self.ipc(json!(["get_property", "core-idle"]))
            .await
            .ok()
            .and_then(|r| r["data"].as_bool());

        let icy_title = self.ipc(json!(["get_property", "icy-title"]))
            .await
            .ok()
            .and_then(|r| r["data"].as_str().map(|s| s.to_string()));

        let time_pos = self.ipc(json!(["get_property", "time-pos"]))
            .await
            .ok()
            .and_then(|r| r["data"].as_f64());

        let duration = self.ipc(json!(["get_property", "duration"]))
            .await
            .ok()
            .and_then(|r| r["data"].as_f64());

        (core_idle, icy_title, time_pos, duration)
    }
}
