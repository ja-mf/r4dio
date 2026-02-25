use serde::{Deserialize, Serialize};

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
}

/// Messages sent from Daemon to TUI (broadcasts)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "broadcast")]
pub enum Broadcast {
    State { data: DaemonState },
    Icy { title: Option<String> },
    Log { message: String },
    Error { message: String },
}

/// Detailed playback status — reflects actual mpv state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum PlaybackStatus {
    #[default]
    Idle, // nothing loaded / explicitly stopped
    Connecting, // loadfile sent, mpv buffering/connecting
    Playing,    // core-idle=false, audio flowing
    Error,      // failed to play (timeout or mpv error)
}

/// Full state of the daemon
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DaemonState {
    pub stations: Vec<Station>,
    pub current_station: Option<usize>,
    pub current_file: Option<String>,
    pub volume: f32,
    pub is_playing: bool, // true when Playing
    pub playback_status: PlaybackStatus,
    pub icy_title: Option<String>,
    pub time_pos_secs: Option<f64>,
    pub duration_secs: Option<f64>,
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
}
