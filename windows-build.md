# Windows Build Guide for radio-tui

This document describes how to build radio-tui for Windows, including all necessary code changes for cross-platform compatibility.

## Overview

The main changes needed for Windows support:
1. **Daemon-TUI IPC**: Switch from Unix sockets to TCP sockets (works everywhere)
2. **MPV IPC**: Use named pipes on Windows, Unix sockets on Unix
3. **File paths**: Replace hardcoded `/tmp/` with platform-agnostic paths
4. **Process spawning**: Handle Windows executable detection

## Distribution Bundle

```
radio-tui-windows/
├── radio.exe          # Combined daemon + TUI binary
├── mpv.exe            # MPV player (download separately)
├── ffmpeg.exe         # Optional, MPV usually includes what it needs
├── stations.toml      # Default station list
└── README.txt         # Usage instructions
```

User extracts ZIP, runs `radio.exe`, everything works.

---

## Code Changes Required

### 1. Update Cargo.toml

Add conditional dependencies for Windows:

```toml
[dependencies]
# ... existing dependencies ...

[target.'cfg(windows)'.dependencies]
winapi = { version = "0.3", features = ["winbase", "namedpipeapi"] }

[target.'cfg(unix)'.dependencies]
# Unix socket support is built into tokio
```

### 2. Create Platform Abstraction Module

Create `src/shared/platform.rs`:

```rust
use std::path::PathBuf;

#[cfg(unix)]
pub fn daemon_socket_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("radio/daemon.sock")
}

#[cfg(windows)]
pub fn daemon_socket_path() -> PathBuf {
    // On Windows, we use TCP instead of sockets
    // Return the port number as a "path" convention
    PathBuf::from("127.0.0.1:9876")
}

#[cfg(unix)]
pub fn mpv_socket_path() -> PathBuf {
    std::env::temp_dir().join("radio-daemon-mpv.sock")
}

#[cfg(windows)]
pub fn mpv_socket_path() -> PathBuf {
    // Windows named pipe - MPV automatically adds \\.\pipe\ prefix
    PathBuf::from("radio-mpv")
}

#[cfg(unix)]
pub fn mpv_socket_arg(path: &PathBuf) -> String {
    format!("--input-ipc-server={}", path.display())
}

#[cfg(windows)]
pub fn mpv_socket_arg(path: &PathBuf) -> String {
    // MPV on Windows uses named pipes: \\.\pipe\<name>
    format!("--input-ipc-server=\\\\.\\pipe\\{}", path.display())
}

#[cfg(unix)]
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir()
}

#[cfg(windows)]
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir()
}

#[cfg(unix)]
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("radio")
}

#[cfg(windows)]
pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("radio")
}

#[cfg(unix)]
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("radio")
}

#[cfg(windows)]
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("radio")
}
```

### 3. Update src/shared/mod.rs

Add the platform module:

```rust
pub mod config;
pub mod platform;
pub mod protocol;
pub mod state;
```

### 4. Update src/shared/config.rs

Replace hardcoded paths with platform-aware ones:

```rust
use super::platform;

fn default_socket_path() -> PathBuf {
    platform::daemon_socket_path()
}

fn default_pid_file() -> PathBuf {
    platform::data_dir().join("daemon.pid")
}

fn default_state_file() -> PathBuf {
    platform::data_dir().join("state.json")
}

fn default_stations_toml() -> PathBuf {
    platform::config_dir().join("stations.toml")
}
```

### 5. Create MPV IPC Abstraction

Create `src/daemon/mpv_ipc.rs` (rename from mpv.rs and refactor):

```rust
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, info, warn};

static NEXT_REQ_ID: AtomicU64 = AtomicU64::new(1);

// ── Platform-specific connection types ─────────────────────────────────────

#[cfg(unix)]
type MpvReader = BufReader<tokio::net::unix::OwnedReadHalf>;
#[cfg(unix)]
type MpvWriter = tokio::net::unix::OwnedWriteHalf;

#[cfg(windows)]
type MpvReader = BufReader<tokio::net::windows::named_pipe::NamedPipeClient>;
#[cfg(windows)]  
type MpvWriter = tokio::net::windows::named_pipe::NamedPipeClient;

pub struct MpvController {
    socket_path: PathBuf,
    process: Option<tokio::process::Child>,
    reader: Option<MpvReader>,
    writer: Option<MpvWriter>,
    last_volume: f32,
}

impl MpvController {
    pub async fn new() -> anyhow::Result<Self> {
        let socket_path = crate::shared::platform::mpv_socket_path();
        Ok(Self {
            socket_path,
            process: None,
            reader: None,
            writer: None,
            last_volume: 0.5,
        })
    }

    // ... rest of implementation (see full file below)
}
```

### 6. Update src/daemon/mpv.rs - Full Platform-Aware Version

The key changes are in the connection methods:

```rust
#[cfg(unix)]
async fn try_reconnect(&mut self) -> bool {
    if !self.socket_path.exists() {
        return false;
    }
    match tokio::net::UnixStream::connect(&self.socket_path).await {
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
    let pipe_path = format!("\\\\.\\pipe\\{}", self.socket_path.display());
    match tokio::net::windows::named_pipe::NamedPipeClient::connect(&pipe_path).await {
        Ok(client) => {
            info!("mpv: reconnected to existing named pipe");
            // On Windows, named pipe is bidirectional
            self.reader = Some(BufReader::new(client));
            // Need to handle writer separately - see note below
            true
        }
        Err(e) => {
            warn!("mpv: failed to reconnect to named pipe: {}", e);
            false
        }
    }
}
```

**Note:** Windows named pipes with tokio require special handling. Consider using the `interprocess` crate for a cleaner abstraction.

### 7. Alternative: Use `interprocess` Crate (Recommended)

Add to Cargo.toml:

```toml
[dependencies]
interprocess = { version = "2.0", features = ["tokio"] }
```

Then use it for both daemon-TUI and MPV communication:

```rust
use interprocess::local_socket::tokio::Stream;

// Works on both Unix and Windows
async fn connect_mpv(socket_path: &Path) -> anyhow::Result<Stream> {
    let name = socket_path.display().to_string();
    #[cfg(windows)]
    let name = format!("{}\\pipe\\{}", r"\\.", name);
    
    Stream::connect(name).await
}
```

### 8. Update Daemon Socket Server (src/daemon/socket.rs)

Replace Unix sockets with TCP for daemon-TUI communication:

```rust
use tokio::net::{TcpListener, TcpStream};

pub fn start_server(
    bind_address: String,
    port: u16,
    state_manager: Arc<StateManager>,
    clients: Arc<RwLock<Vec<ClientHandle>>>,
    command_tx: mpsc::Sender<Command>,
    broadcast_tx: broadcast::Sender<BroadcastMessage>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let addr = format!("{}:{}", bind_address, port);
        let listener = match TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                error!("Failed to bind TCP socket: {}", e);
                return;
            }
        };

        info!("TCP server listening at {}", addr);
        // ... rest of implementation
    })
}
```

### 9. Update TUI Connection (src/tui/main.rs)

Update the connection handler to use TCP:

```rust
async fn connection_handler(
    daemon_address: String, // Changed from socket_path
    tx: mpsc::Sender<AppMessage>,
    mut cmd_rx: mpsc::Receiver<Command>,
) {
    let mut retry_delay = Duration::from_millis(100);
    let max_retry_delay = Duration::from_secs(5);

    loop {
        // Start daemon if not reachable
        match tokio::net::TcpStream::connect(&daemon_address).await {
            Ok(stream) => {
                info!("Connected to daemon");
                // ... existing connection logic
            }
            Err(e) => {
                info!("Daemon not running, starting it…");
                if let Err(e) = start_daemon().await {
                    warn!("Failed to start daemon: {}", e);
                }
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(max_retry_delay);
            }
        }
    }
}
```

### 10. Update src/tui/connection.rs

Replace UnixStream with TcpStream:

```rust
use tokio::net::TcpStream;

pub struct DaemonConnection {
    stream: TcpStream,
    read_buffer: Vec<u8>,
}

impl DaemonConnection {
    pub async fn connect(address: &str) -> anyhow::Result<Self> {
        let stream = TcpStream::connect(address).await?;
        Ok(Self {
            stream,
            read_buffer: Vec::with_capacity(4096),
        })
    }
    // ... rest unchanged
}
```

### 11. Fix Hardcoded Paths in src/tui/main.rs

Find and replace all `/tmp/` references:

```rust
// BEFORE
.unwrap_or_else(|| PathBuf::from("/tmp/radio"));

// AFTER
.unwrap_or_else(|| crate::shared::platform::data_dir());
```

Search for these patterns and fix them:
- `/tmp/radio`
- `/tmp/radio-tui`
- `/tmp/songs.csv`
- `/tmp/nts-downloads`

---

## Build Instructions

### Prerequisites

1. Install Rust: https://rustup.rs/
2. On Windows: Install Visual Studio Build Tools (C++ workload)

### Build for Windows (from Windows)

```powershell
# Development build
cargo build

# Release build (optimized)
cargo build --release

# Output: target/release/radio.exe
```

### Build for Windows (from Linux)

```bash
# Install cross-compilation tools
cargo install cargo-xwin

# Build
cargo xwin build --target x86_64-pc-windows-msvc --release

# Output: target/x86_64-pc-windows-msvc/release/radio.exe
```

### Build for All Platforms (GitHub Actions)

Create `.github/workflows/release.yml`:

```yaml
name: Build and Release

on:
  push:
    tags:
      - 'v*'

jobs:
  build:
    strategy:
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            artifact: radio
          - os: macos-latest
            target: x86_64-apple-darwin
            artifact: radio
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            artifact: radio.exe

    runs-on: ${{ matrix.os }}
    
    steps:
      - uses: actions/checkout@v4
      
      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      
      - name: Build
        run: cargo build --release --target ${{ matrix.target }}
      
      - name: Package (Windows)
        if: matrix.os == 'windows-latest'
        run: |
          mkdir dist
          cp target/${{ matrix.target }}/release/radio.exe dist/
          # Download MPV
          Invoke-WebRequest -Uri "https://sourceforge.net/projects/mpv-player-windows/files/latest/download" -OutFile "mpv.7z"
          7z x mpv.7z -o./dist/mpv
          cp dist/mpv/mpv.exe dist/
          Compress-Archive -Path dist/* -Destination radio-tui-windows.zip
      
      - name: Upload Artifact
        uses: actions/upload-artifact@v4
        with:
          name: radio-tui-${{ matrix.os }}
          path: dist/
```

---

## Distribution Package

### Manual Windows Package Creation

1. Build the release binary:
   ```powershell
   cargo build --release
   ```

2. Download MPV:
   - Go to https://mpv.io/installation/
   - Download Windows build from shinchiro
   - Extract `mpv.exe`

3. Create distribution folder:
   ```
   radio-tui-windows/
   ├── radio.exe
   ├── mpv.exe
   └── README.txt
   ```

4. Create README.txt:
   ```
   radio-tui - Terminal Radio Player
   
   Usage:
     1. Double-click radio.exe or run from PowerShell
     2. Use arrow keys to navigate stations
     3. Press Enter to play, Space to pause
   
   First run will create config files in:
     %APPDATA%\radio\
   
   Requirements:
     - Windows 10 or later
     - Terminal that supports ANSI colors (Windows Terminal recommended)
   ```

5. ZIP the folder and distribute

---

## Testing on Windows

### Quick Test Checklist

1. **Basic startup:**
   ```powershell
   .\radio.exe
   ```
   - Should open TUI
   - Should spawn daemon in background
   - Should show station list

2. **MPV detection:**
   - If mpv.exe is in same folder, should use it
   - If not in PATH, should show error message

3. **Config creation:**
   - Check `%APPDATA%\radio\` for config files
   - Check `%LOCALAPPDATA%\radio\` for data files

4. **Playback:**
   - Select a station, press Enter
   - Should start playing
   - ICY metadata should appear

5. **Cleanup:**
   - Close with 'q'
   - Daemon should terminate
   - No orphan processes

---

## Known Windows Quirks

1. **Terminal colors:** Windows CMD has limited color support. Windows Terminal or PowerShell 7+ recommended.

2. **Named pipes:** MPV's named pipe implementation on Windows can be finicky. The `interprocess` crate helps.

3. **Path separators:** Always use `PathBuf` / `std::path::Path`, never hardcode `/` or `\`.

4. **Process spawning:** Windows doesn't have fork. Ensure daemon spawning uses `Command::new()`.

5. **Firewall:** First run may trigger Windows Firewall prompt for network access (HTTP API).

---

## Quick Implementation Checklist

- [ ] Create `src/shared/platform.rs` with platform abstractions
- [ ] Update `src/shared/mod.rs` to export platform module
- [ ] Update `src/shared/config.rs` to use platform paths
- [ ] Update `src/daemon/socket.rs` to use TCP
- [ ] Update `src/daemon/mpv.rs` for Windows named pipes
- [ ] Update `src/tui/main.rs` connection handler for TCP
- [ ] Update `src/tui/connection.rs` to use TcpStream
- [ ] Fix all hardcoded `/tmp/` paths
- [ ] Test build on Windows
- [ ] Create distribution ZIP with mpv.exe
