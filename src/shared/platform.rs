use std::path::PathBuf;

pub const DAEMON_TCP_PORT: u16 = 9876;
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
    #[cfg(unix)]
    {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("radio")
    }
    #[cfg(windows)]
    {
        // On Windows, check for portable config in executable directory first
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let portable_config = exe_dir.join("config");
                if portable_config.exists() {
                    return portable_config;
                }
            }
        }

        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("radio")
    }
}

pub fn config_dir() -> PathBuf {
    // On Windows, check for portable config in executable directory first
    #[cfg(windows)]
    {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let portable_config = exe_dir.join("config");
                if portable_config.exists() {
                    return portable_config;
                }
            }
        }
    }

    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("radio")
}

pub fn temp_dir() -> PathBuf {
    std::env::temp_dir()
}

pub fn cache_dir() -> PathBuf {
    #[cfg(unix)]
    {
        dirs::cache_dir()
            .unwrap_or_else(|| temp_dir())
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

pub fn find_mpv_binary() -> Option<PathBuf> {
    let exe_name = mpv_binary_name();

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            let local_mpv = dir.join(exe_name);
            if local_mpv.exists() {
                return Some(local_mpv);
            }
        }
    }

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
