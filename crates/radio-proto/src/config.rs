use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::platform;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub http: HttpConfig,
    #[serde(default)]
    pub mpv: MpvConfig,
    #[serde(default)]
    pub stations: StationsConfig,
    #[serde(default)]
    pub paths: PathsConfig,
    #[serde(default)]
    pub polling: PollingConfig,
    #[serde(default)]
    pub viz: VizConfig,
    #[serde(default)]
    pub binaries: BinariesConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(default)]
    pub pid_file: PathBuf,
    #[serde(default)]
    pub state_file: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpConfig {
    #[serde(default = "default_http_enabled")]
    pub enabled: bool,
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpvConfig {
    #[serde(default = "default_volume")]
    pub default_volume: f32,
}

/// User-configurable paths for downloads, cache, and data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsConfig {
    /// Directory for NTS show downloads.
    /// Defaults to `~/radio-downloads` (or portable `downloads/` on Windows).
    #[serde(default = "default_downloads_dir")]
    pub downloads_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollingConfig {
    #[serde(default = "default_auto_polling")]
    pub auto_polling: bool,
    #[serde(default = "default_poll_interval_secs")]
    pub poll_interval_secs: u64,
    /// Number of concurrent ICY probes for non-NTS stations per poll cycle.
    /// Lower values reduce CPU/network load on slower machines. Default: 3
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
    /// Maximum number of stations to poll per cycle. Default: 64
    #[serde(default = "default_max_jobs_per_cycle")]
    pub max_jobs_per_cycle: usize,
}

/// Linux-specific audio visualization configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VizConfig {
    /// Use PipeWire/PulseAudio monitor for VU meter and oscilloscope visualization
    /// instead of the stream's audio data. This visualizes the actual system audio output.
    /// Only works on Linux. Default: false
    #[serde(default = "default_pipewire_viz")]
    pub pipewire_viz: bool,
    /// PipeWire/PulseAudio device name to monitor. None = default monitor source
    #[serde(default)]
    pub pipewire_device: Option<String>,
}

/// Configuration for external binary dependencies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinariesConfig {
    /// If true, use system-installed binaries from PATH instead of bundled ones.
    /// When false (default), searches for binaries in external/ folder next to executable.
    #[serde(default = "default_use_system_deps")]
    pub use_system_deps: bool,
}

impl Default for VizConfig {
    fn default() -> Self {
        Self {
            pipewire_viz: default_pipewire_viz(),
            pipewire_device: None,
        }
    }
}

impl Default for BinariesConfig {
    fn default() -> Self {
        Self {
            use_system_deps: default_use_system_deps(),
        }
    }
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            downloads_dir: default_downloads_dir(),
        }
    }
}

impl Default for PollingConfig {
    fn default() -> Self {
        Self {
            auto_polling: default_auto_polling(),
            poll_interval_secs: default_poll_interval_secs(),
            max_concurrency: default_max_concurrency(),
            max_jobs_per_cycle: default_max_jobs_per_cycle(),
        }
    }
}

fn default_downloads_dir() -> PathBuf {
    // On Windows, check for portable downloads directory in executable directory
    #[cfg(windows)]
    {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let portable_downloads = exe_dir.join("downloads");
                if portable_downloads.exists() {
                    return portable_downloads;
                }
            }
        }
    }

    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("radio-downloads")
}

/// Station list source — either an https:// URL or a local file path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationsConfig {
    /// Path to a local TOML station file (highest priority).
    /// Defaults to `$XDG_CONFIG_HOME/radio/stations.toml`.
    #[serde(default = "default_stations_toml")]
    pub stations_toml: PathBuf,
    /// URL or file path for an m3u station list (fallback when TOML not found).
    #[serde(default = "default_m3u_url")]
    pub m3u_url: String,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            pid_file: default_pid_file(),
            state_file: default_state_file(),
        }
    }
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            enabled: default_http_enabled(),
            bind_address: default_bind_address(),
            port: default_port(),
        }
    }
}

impl Default for MpvConfig {
    fn default() -> Self {
        Self {
            default_volume: default_volume(),
        }
    }
}

impl Default for StationsConfig {
    fn default() -> Self {
        Self {
            stations_toml: default_stations_toml(),
            m3u_url: default_m3u_url(),
        }
    }
}

fn default_pid_file() -> PathBuf {
    platform::data_dir().join("daemon.pid")
}

fn default_state_file() -> PathBuf {
    platform::data_dir().join("state.json")
}

fn default_http_enabled() -> bool {
    true
}

fn default_bind_address() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8989
}

fn default_volume() -> f32 {
    0.5
}

fn default_auto_polling() -> bool {
    true
}

fn default_poll_interval_secs() -> u64 {
    120
}

fn default_max_concurrency() -> usize {
    3 // Reduced from 6 for lower CPU usage
}

fn default_max_jobs_per_cycle() -> usize {
    64
}

fn default_pipewire_viz() -> bool {
    false
}

fn default_use_system_deps() -> bool {
    false
}

fn default_m3u_url() -> String {
    "https://raw.githubusercontent.com/ja-mf/radio-curation/refs/heads/main/jamf_radios.m3u"
        .to_string()
}

fn default_stations_toml() -> PathBuf {
    // On Windows, check for portable stations.toml in executable directory
    #[cfg(windows)]
    {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let portable_stations = exe_dir.join("stations.toml");
                if portable_stations.exists() {
                    return portable_stations;
                }
            }
        }
    }

    platform::config_dir().join("stations.toml")
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let config_path = Self::config_path();

        if !config_path.exists() {
            let config = Self::default();
            config.save()?;
            return Ok(config);
        }

        let content = std::fs::read_to_string(&config_path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let config_path = Self::config_path();
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&config_path, content)?;
        Ok(())
    }

    pub fn config_path() -> PathBuf {
        platform::config_dir().join("config.toml")
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            daemon: DaemonConfig::default(),
            http: HttpConfig::default(),
            mpv: MpvConfig::default(),
            stations: StationsConfig::default(),
            paths: PathsConfig::default(),
            polling: PollingConfig::default(),
            viz: VizConfig::default(),
            binaries: BinariesConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.http.enabled);
        assert_eq!(config.http.port, 8989);
        assert_eq!(config.http.bind_address, "127.0.0.1");
        assert!(config.stations.m3u_url.starts_with("https://"));
        assert!(config.polling.auto_polling);
        assert_eq!(config.polling.poll_interval_secs, 120);
        assert!(config
            .stations
            .stations_toml
            .ends_with("radio/stations.toml"));
    }
}
