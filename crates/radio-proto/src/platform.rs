use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub const DAEMON_TCP_PORT: u16 = 9876;

/// Global flag to control whether to use system-installed binaries from PATH
/// instead of bundled ones in external/ folder.
/// Defaults to false (use bundled/external binaries).
static USE_SYSTEM_DEPS: AtomicBool = AtomicBool::new(false);

/// Set whether to use system dependencies (from PATH) instead of bundled ones.
pub fn set_use_system_deps(use_system: bool) {
    USE_SYSTEM_DEPS.store(use_system, Ordering::Relaxed);
}

/// Check whether to use system dependencies from PATH.
pub fn should_use_system_deps() -> bool {
    USE_SYSTEM_DEPS.load(Ordering::Relaxed)
}
const DAEMON_TCP_HOST: &str = "127.0.0.1";

pub fn daemon_address() -> String {
    format!("{}:{}", DAEMON_TCP_HOST, DAEMON_TCP_PORT)
}

#[cfg(unix)]
pub fn mpv_socket_name() -> String {
    format!("{}/radio-mpv.sock", std::env::temp_dir().display())
}

#[cfg(windows)]
pub fn mpv_socket_name() -> String {
    "radio-mpv".to_string()
}

#[cfg(unix)]
pub fn mpv_socket_arg() -> String {
    format!("--input-ipc-server={}", mpv_socket_name())
}

#[cfg(windows)]
pub fn mpv_socket_arg() -> String {
    format!("--input-ipc-server=\\\\.\\pipe\\{}", mpv_socket_name())
}

pub fn data_dir() -> PathBuf {
    // On macOS and Linux, use ~/.local/share/radio/ (XDG standard)
    // instead of macOS Application Support for consistency
    #[cfg(unix)]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".local")
            .join("share")
            .join("radio")
    }
    #[cfg(windows)]
    {
        // On Windows, check for portable data directory in executable directory first
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let portable_data = exe_dir.join("data");
                if portable_data.exists() {
                    return portable_data;
                }
            }
        }

        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("radio")
    }
}

pub fn config_dir() -> PathBuf {
    // On Windows, check for portable config.toml in executable directory first
    #[cfg(windows)]
    {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let portable_config = exe_dir.join("config.toml");
                if portable_config.exists() {
                    return exe_dir.to_path_buf();
                }
            }
        }
    }

    // On macOS and Linux, always use ~/.config/radio/
    // (avoid macOS Application Support folder for consistency)
    #[cfg(unix)]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
            .join("radio")
    }

    #[cfg(windows)]
    {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("radio")
    }
}

pub fn temp_dir() -> PathBuf {
    std::env::temp_dir()
}

pub fn cache_dir() -> PathBuf {
    // On macOS and Linux, use ~/.cache/radio/ (XDG standard)
    // instead of macOS ~/Library/Caches/ for consistency
    #[cfg(unix)]
    {
        dirs::home_dir()
            .unwrap_or_else(|| temp_dir())
            .join(".cache")
            .join("radio")
    }
    #[cfg(windows)]
    {
        dirs::cache_dir()
            .unwrap_or_else(|| temp_dir())
            .join("radio")
    }
}

#[cfg(unix)]
pub fn mpv_binary_name() -> &'static str {
    "mpv"
}

#[cfg(windows)]
pub fn mpv_binary_name() -> &'static str {
    "mpv.exe"
}

#[cfg(unix)]
fn vibra_binary_names() -> &'static [&'static str] {
    &["vibra"]
}

#[cfg(windows)]
fn vibra_binary_names() -> &'static [&'static str] {
    &["vibra.exe", "vibra"]
}

#[cfg(unix)]
fn ffmpeg_binary_names() -> &'static [&'static str] {
    &["ffmpeg"]
}

#[cfg(windows)]
fn ffmpeg_binary_names() -> &'static [&'static str] {
    &["ffmpeg.exe", "ffmpeg"]
}

#[cfg(unix)]
fn ffprobe_binary_names() -> &'static [&'static str] {
    &["ffprobe"]
}

#[cfg(windows)]
fn ffprobe_binary_names() -> &'static [&'static str] {
    &["ffprobe.exe", "ffprobe"]
}

#[cfg(unix)]
fn yt_dlp_binary_names() -> &'static [&'static str] {
    &["yt-dlp"]
}

#[cfg(windows)]
fn yt_dlp_binary_names() -> &'static [&'static str] {
    &["yt-dlp.exe", "yt-dlp"]
}

fn find_beside_exe(names: &[&str]) -> Option<PathBuf> {
    let current_exe = std::env::current_exe().ok()?;
    let dir = current_exe.parent()?;
    for name in names {
        let p = dir.join(name);
        if p.exists() {
            return Some(p);
        }
        let p = dir.join("external").join(name);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn find_on_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var("PATH").ok()?;
    #[cfg(unix)]
    let sep = ":";
    #[cfg(windows)]
    let sep = ";";
    for dir in path.split(sep) {
        for name in names {
            let p = PathBuf::from(dir).join(name);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

/// Find the vibra binary (Shazam fingerprint CLI).
/// Checks: beside the current exe, ~/gh/vibra/build/cli/vibra, then PATH.
/// If use_system_deps is true, skips bundled binaries and uses PATH only.
pub fn find_vibra_binary() -> Option<PathBuf> {
    // If using system deps, skip bundled/external search
    if !should_use_system_deps() {
        // 1. Beside current exe
        if let Some(p) = find_beside_exe(vibra_binary_names()) {
            return Some(p);
        }

        // 2. Developer build location
        if let Some(home) = dirs::home_dir() {
            for name in vibra_binary_names() {
                let dev = home.join("gh/vibra/build/cli").join(name);
                if dev.exists() {
                    return Some(dev);
                }
            }
        }
    }

    // 3. PATH
    if let Some(p) = find_on_path(vibra_binary_names()) {
        return Some(p);
    }

    None
}

/// Find ffmpeg binary for audio capture.
/// If use_system_deps is true, skips bundled binaries and uses PATH only.
pub fn find_ffmpeg_binary() -> Option<PathBuf> {
    // FFMPEG_PATH env override (vibra docs mention this)
    if let Ok(p) = std::env::var("FFMPEG_PATH") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }

    // If using system deps, skip bundled/external search
    if !should_use_system_deps() {
        // Beside current exe
        if let Some(p) = find_beside_exe(ffmpeg_binary_names()) {
            return Some(p);
        }
    }

    // PATH
    if let Some(p) = find_on_path(ffmpeg_binary_names()) {
        return Some(p);
    }

    None
}

/// Find ffprobe binary for metadata probing.
/// If use_system_deps is true, skips bundled binaries and uses PATH only.
pub fn find_ffprobe_binary() -> Option<PathBuf> {
    if !should_use_system_deps() {
        if let Some(p) = find_beside_exe(ffprobe_binary_names()) {
            return Some(p);
        }
    }
    find_on_path(ffprobe_binary_names())
}

/// Find mpv binary for playback.
/// If use_system_deps is true, skips bundled binaries and uses PATH only.
pub fn find_mpv_binary() -> Option<PathBuf> {
    let exe_name = mpv_binary_name();

    // If not using system deps, check beside the exe first
    if !should_use_system_deps() {
        if let Ok(current_exe) = std::env::current_exe() {
            if let Some(dir) = current_exe.parent() {
                let local_mpv = dir.join(exe_name);
                if local_mpv.exists() {
                    return Some(local_mpv);
                }
            }
        }
    }

    // Search PATH
    if let Ok(path) = std::env::var("PATH") {
        #[cfg(unix)]
        let separator = ":";
        #[cfg(windows)]
        let separator = ";";

        for dir in path.split(separator) {
            let mpv_path = PathBuf::from(dir).join(exe_name);
            if mpv_path.exists() {
                return Some(mpv_path);
            }
        }
    }

    None
}

/// Find yt-dlp binary for downloading audio.
///
/// Searches in order:
/// 1. YT_DLP_PATH environment variable
/// 2. Beside current executable (unless use_system_deps is true)
/// 3. PATH
pub fn find_yt_dlp_binary() -> Option<PathBuf> {
    // 1. Environment variable override
    if let Ok(path) = std::env::var("YT_DLP_PATH") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    // 2. Beside executable (only if not using system deps)
    if !should_use_system_deps() {
        if let Some(p) = find_beside_exe(yt_dlp_binary_names()) {
            return Some(p);
        }
    }

    // 3. PATH
    find_on_path(yt_dlp_binary_names())
}
