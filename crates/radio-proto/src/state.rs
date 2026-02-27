use crate::protocol::{DaemonState, MpvHealth, PlaybackStatus, Station};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentState {
    pub last_station_idx: Option<usize>,
    pub volume: f32,
}

impl Default for PersistentState {
    fn default() -> Self {
        Self {
            last_station_idx: None,
            volume: 0.5,
        }
    }
}

pub struct StateManager {
    state: Arc<RwLock<DaemonState>>,
    state_file: PathBuf,
}

impl StateManager {
    pub fn new(state_file: PathBuf, stations: Vec<Station>) -> Self {
        let persistent = Self::load_persistent(&state_file);

        let state = DaemonState {
            rev: 1,
            stations,
            current_station: persistent.last_station_idx,
            current_file: None,
            volume: persistent.volume,
            is_playing: false,
            is_paused: false,
            playback_status: PlaybackStatus::Idle,
            icy_title: None,
            time_pos_secs: None,
            duration_secs: None,
            mpv_health: MpvHealth::Absent,
        };

        Self {
            state: Arc::new(RwLock::new(state)),
            state_file,
        }
    }

    pub fn arc(&self) -> Arc<RwLock<DaemonState>> {
        Arc::clone(&self.state)
    }

    pub async fn get_state(&self) -> DaemonState {
        self.state.read().await.clone()
    }

    pub async fn set_playing(&self, idx: usize) -> anyhow::Result<()> {
        {
            let mut state = self.state.write().await;
            state.current_station = Some(idx);
            state.current_file = None;
            state.is_playing = true;
            state.playback_status = PlaybackStatus::Connecting;
            state.icy_title = None; // clear stale ICY from previous station
            state.time_pos_secs = None;
            state.duration_secs = None;
            state.rev += 1;
        }
        self.save().await
    }

    pub async fn set_playing_file(&self, path: String) -> anyhow::Result<()> {
        {
            let mut state = self.state.write().await;
            state.current_station = None;
            state.current_file = Some(path);
            state.is_playing = true;
            state.playback_status = PlaybackStatus::Connecting;
            state.icy_title = None;
            state.time_pos_secs = Some(0.0);
            state.duration_secs = None;
            state.rev += 1;
        }
        self.save().await
    }

    pub async fn set_stopped(&self) -> anyhow::Result<()> {
        {
            let mut state = self.state.write().await;
            state.is_playing = false;
            state.playback_status = PlaybackStatus::Idle;
            state.icy_title = None;
            state.current_file = None;
            state.time_pos_secs = None;
            state.duration_secs = None;
            state.rev += 1;
        }
        self.save().await
    }

    pub async fn set_playback_status(&self, status: PlaybackStatus) {
        let mut state = self.state.write().await;
        state.is_playing = matches!(status, PlaybackStatus::Playing | PlaybackStatus::Paused);
        state.is_paused = status == PlaybackStatus::Paused;
        state.playback_status = status;
        state.rev += 1;
    }

    pub async fn set_mpv_health(&self, health: MpvHealth) {
        let mut state = self.state.write().await;
        state.mpv_health = health;
        state.rev += 1;
    }

    pub async fn set_volume(&self, volume: f32) -> anyhow::Result<()> {
        {
            let mut state = self.state.write().await;
            state.volume = volume.clamp(0.0, 1.0);
            state.rev += 1;
        }
        self.save().await
    }

    pub async fn set_icy_title(&self, title: Option<String>) {
        let mut state = self.state.write().await;
        state.icy_title = title;
        state.rev += 1;
    }

    pub async fn set_timeline(&self, time_pos_secs: Option<f64>, duration_secs: Option<f64>) {
        let mut state = self.state.write().await;
        state.time_pos_secs = time_pos_secs;
        state.duration_secs = duration_secs;
        state.rev += 1;
    }

    pub async fn next_station(&self) -> anyhow::Result<()> {
        let stations_len = {
            let state = self.state.read().await;
            state.stations.len()
        };

        if stations_len == 0 {
            return Ok(());
        }

        {
            let mut state = self.state.write().await;
            let current = state.current_station.unwrap_or(0);
            let next = (current + 1) % stations_len;
            state.current_station = Some(next);
            state.is_playing = true;
            state.rev += 1;
        }
        self.save().await
    }

    pub async fn prev_station(&self) -> anyhow::Result<()> {
        let stations_len = {
            let state = self.state.read().await;
            state.stations.len()
        };

        if stations_len == 0 {
            return Ok(());
        }

        {
            let mut state = self.state.write().await;
            let current = state.current_station.unwrap_or(0);
            let prev = if current == 0 {
                stations_len - 1
            } else {
                current - 1
            };
            state.current_station = Some(prev);
            state.is_playing = true;
            state.rev += 1;
        }
        self.save().await
    }

    pub async fn random_station(&self) -> anyhow::Result<()> {
        use rand::Rng;

        let stations_len = {
            let state = self.state.read().await;
            state.stations.len()
        };

        if stations_len == 0 {
            return Ok(());
        }

        let random_idx = rand::thread_rng().gen_range(0..stations_len);

        {
            let mut state = self.state.write().await;
            state.current_station = Some(random_idx);
            state.is_playing = true;
            state.rev += 1;
        }
        self.save().await
    }

    async fn save(&self) -> anyhow::Result<()> {
        let state = self.state.read().await;
        let persistent = PersistentState {
            last_station_idx: state.current_station,
            volume: state.volume,
        };

        if let Some(parent) = self.state_file.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let json = serde_json::to_string_pretty(&persistent)?;
        tokio::fs::write(&self.state_file, json).await?;
        Ok(())
    }

    fn load_persistent(state_file: &PathBuf) -> PersistentState {
        if let Ok(content) = std::fs::read_to_string(state_file) {
            if let Ok(persistent) = serde_json::from_str::<PersistentState>(&content) {
                return persistent;
            }
        }
        PersistentState::default()
    }
}

pub fn parse_m3u_from_str(content: &str) -> anyhow::Result<Vec<Station>> {
    let mut stations = Vec::new();
    let mut pending_name: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            if let Some(comma_idx) = rest.find(',') {
                pending_name = Some(rest[comma_idx + 1..].trim().to_string());
            }
            continue;
        }

        if line.starts_with('#') {
            continue;
        }

        let url = line.to_string();
        let name = pending_name.take().unwrap_or_else(|| url.clone());

        stations.push(Station {
            name,
            url,
            ..Station::default()
        });
    }

    Ok(stations)
}

pub fn load_stations_from_m3u(path: &std::path::Path) -> anyhow::Result<Vec<Station>> {
    let content = std::fs::read_to_string(path)?;
    parse_m3u_from_str(&content)
}

// ── TOML station loader ───────────────────────────────────────────────────────

/// Intermediate struct that matches the TOML `[[station]]` table.
/// We keep this separate from `Station` so the TOML schema can diverge from
/// the wire protocol struct without breaking either.
#[derive(Debug, serde::Deserialize)]
struct TomlStationFile {
    station: Vec<TomlStation>,
}

#[derive(Debug, serde::Deserialize)]
struct TomlStation {
    name: String,
    url: String,
    #[serde(default)]
    mixtape_url: String,
    #[serde(default)]
    network: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    city: String,
    #[serde(default)]
    country: String,
}

pub fn load_stations_from_toml(path: &std::path::Path) -> anyhow::Result<Vec<Station>> {
    let content = std::fs::read_to_string(path)?;
    parse_stations_from_toml_str(&content)
}

pub fn parse_stations_from_toml_str(content: &str) -> anyhow::Result<Vec<Station>> {
    let file: TomlStationFile = toml::from_str(content)?;
    let stations = file
        .station
        .into_iter()
        .map(|s| Station {
            name: s.name,
            url: s.url,
            mixtape_url: s.mixtape_url,
            network: s.network,
            description: s.description,
            tags: s.tags,
            city: s.city,
            country: s.country,
        })
        .collect();
    Ok(stations)
}
