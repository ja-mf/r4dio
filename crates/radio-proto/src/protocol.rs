use serde::{Deserialize, Serialize};

/// Current protocol version.  Bump this when the wire format changes in a
/// breaking way.  The TUI checks this on connect and can refuse to talk to an
/// incompatible daemon.
pub const PROTOCOL_VERSION: u32 = 1;

/// Messages sent from TUI to Daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd")]
pub enum Command {
    Play { station_idx: usize },
    PlayFile { path: String },
    PlayFileAt { path: String, start_secs: f64 },
    PlayFilePausedAt { path: String, start_secs: f64 },
    Stop,
    Next,
    Prev,
    Random,
    TogglePause,
    Volume { value: f32 },
    SeekRelative { seconds: f64 },
    SeekTo { seconds: f64 },
    GetState,
    /// Enable latency telemetry for debugging.
    EnableTelemetry,
    /// Print latency telemetry report to log.
    PrintTelemetryReport,
}

/// Messages sent from Daemon to TUI (broadcasts)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "broadcast")]
pub enum Broadcast {
    /// Sent immediately on connect: daemon version + full state snapshot.
    Hello {
        protocol_version: u32,
        daemon_rev: u64,
        state: DaemonState,
    },
    State {
        data: DaemonState,
    },
    Icy {
        title: Option<String>,
    },
    Log {
        message: String,
    },
    Error {
        message: String,
    },
    /// Audio level from mpv lavfi astats filter.  Sent at ~17-20 Hz during playback.
    /// `rms_db`: overall RMS level in dBFS (e.g. -18.5).  Silence ≈ -90.0.
    AudioLevel {
        rms_db: f32,
    },
    /// Raw PCM samples for the oscilloscope.  Mono, 11025 Hz, normalised f32 (-1.0..1.0).
    /// Sent every ~46 ms (512 samples per chunk) during stream playback.
    Pcm {
        samples: Vec<f32>,
    },
}

/// Detailed playback status — reflects actual mpv state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum PlaybackStatus {
    #[default]
    Idle, // nothing loaded / explicitly stopped
    Connecting, // loadfile sent, mpv buffering/connecting
    Playing,    // core-idle=false, audio flowing
    Paused,     // explicitly paused
    Error,      // failed to play (timeout or mpv error)
}

/// Health of the mpv process as observed by the daemon.
///
/// Transitions:
///   Absent -> Starting -> Running -> Dead -> Restarting -> Starting ...
///   Running -> Degraded(reason) -> Running | Dead
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum MpvHealth {
    /// mpv process does not exist yet (before first use).
    #[default]
    Absent,
    /// Process is spawning / socket not yet available.
    Starting,
    /// Socket connected, IPC responding normally.
    Running,
    /// Connected but IPC is slow / returning errors.
    Degraded(String),
    /// Process exited or socket closed.
    Dead,
    /// Restarting after death.
    Restarting,
}

impl MpvHealth {
    /// Short label for badges / status bar (≤5 chars).
    pub fn badge_label(&self) -> Option<&str> {
        match self {
            MpvHealth::Absent => None,
            MpvHealth::Starting => Some("INIT"),
            MpvHealth::Running => None, // normal — no badge needed
            MpvHealth::Degraded(_) => Some("DEGD"),
            MpvHealth::Dead => Some("DEAD"),
            MpvHealth::Restarting => Some("REST"),
        }
    }

    /// True when mpv is in an error/non-running state that users should notice.
    pub fn is_unhealthy(&self) -> bool {
        matches!(
            self,
            MpvHealth::Degraded(_) | MpvHealth::Dead | MpvHealth::Restarting
        )
    }
}

/// Full state of the daemon.  `rev` is a monotonically increasing counter
/// incremented every time the state changes.  Clients can use it to detect
/// missed updates and request a resync.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DaemonState {
    /// Monotonic revision counter — incremented on every state change.
    #[serde(default)]
    pub rev: u64,
    pub stations: Vec<Station>,
    pub current_station: Option<usize>,
    pub current_file: Option<String>,
    pub volume: f32,
    pub is_playing: bool, // true when Playing
    pub playback_status: PlaybackStatus,
    pub icy_title: Option<String>,
    pub time_pos_secs: Option<f64>,
    pub duration_secs: Option<f64>,
    /// Health of the mpv process as tracked by the daemon.
    #[serde(default)]
    pub mpv_health: MpvHealth,
    /// Whether playback is currently paused (separate from playback_status for clarity).
    #[serde(default)]
    pub is_paused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Station {
    pub name: String,
    pub url: String,
    /// Short description / blurb
    #[serde(default)]
    pub description: String,
    /// Parent network or brand (e.g. "NTS", "SomaFM", "BBC")
    #[serde(default)]
    pub network: String,
    /// Searchable tags (genre, style, language, etc.)
    #[serde(default)]
    pub tags: Vec<String>,
    /// City the station broadcasts from
    #[serde(default)]
    pub city: String,
    /// Country the station broadcasts from
    #[serde(default)]
    pub country: String,
}

/// Wrapper for socket communication
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Command(Command),
    Broadcast(Broadcast),
}

impl Message {
    pub fn encode(&self) -> anyhow::Result<Vec<u8>> {
        let json = serde_json::to_vec(self)?;
        let len = json.len() as u32;
        let mut result = Vec::with_capacity(4 + json.len());
        result.extend_from_slice(&len.to_be_bytes());
        result.extend_from_slice(&json);
        Ok(result)
    }

    pub fn decode(data: &[u8]) -> anyhow::Result<(Self, usize)> {
        if data.len() < 4 {
            anyhow::bail!("Insufficient data for length header");
        }
        let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if data.len() < 4 + len {
            anyhow::bail!("Insufficient data for message");
        }
        let msg: Self = serde_json::from_slice(&data[4..4 + len])?;
        Ok((msg, 4 + len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_encode_decode() {
        let msg = Message::Command(Command::Play { station_idx: 5 });
        let encoded = msg.encode().unwrap();
        let (decoded, len) = Message::decode(&encoded).unwrap();
        assert_eq!(len, encoded.len());
        match decoded {
            Message::Command(Command::Play { station_idx }) => assert_eq!(station_idx, 5),
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_hello_encode_decode() {
        let state = DaemonState {
            rev: 42,
            ..Default::default()
        };
        let msg = Message::Broadcast(Broadcast::Hello {
            protocol_version: PROTOCOL_VERSION,
            daemon_rev: 42,
            state,
        });
        let encoded = msg.encode().unwrap();
        let (decoded, _) = Message::decode(&encoded).unwrap();
        match decoded {
            Message::Broadcast(Broadcast::Hello {
                protocol_version,
                daemon_rev,
                ..
            }) => {
                assert_eq!(protocol_version, PROTOCOL_VERSION);
                assert_eq!(daemon_rev, 42);
            }
            _ => panic!("Wrong message type"),
        }
    }
}
