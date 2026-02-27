# Radio TUI — Project Roadmap

## Overview
A terminal UI for streaming radio with metadata-rich display, audio fingerprinting integration, and local file playback support.

## Architecture
Three-process system:
- **radio-daemon**: Background MPV controller, HTTP API, TCP socket server
- **radio-tui**: Terminal UI with component-based architecture
- **External tools**: vibra (audio fingerprinting), ffmpeg (audio capture), mpv (playback)

## Current Status

### Implemented ✓
- [x] Three-process architecture (daemon + TUI + tools)
- [x] Station list with filtering and sorting
- [x] NTS 1/2 integration with live API polling
- [x] Audio fingerprinting (vibra) with VDS storage
- [x] File browser for local downloads
- [x] ICY metadata ticker
- [x] Songs recognition history panel
- [x] Keyboard navigation with mouse support
- [x] Workspace switching (Radio ↔ Files)
- [x] Star ratings for stations/files
- [x] Volume and seek controls
- [x] Portable config support (Windows)

### In Progress

### Planned

#### High Priority
- [ ] **Audio Visualizer**: Integrate scope-tui for waveform display
  - Compact mode: below header, between title and volume
  - Full-screen mode: toggled with dedicated key
  - Color schemes: cyan, magenta, green, etc.
  - Connect to MPV audio output via pipewire/pulse monitor

#### Medium Priority
- [ ] **Database Migration**: Move from VDS/CSV to SQLite
  - Maintain visidata compatibility
  - Better query performance
  - Schema: timestamp, track, artist, genre, metadata, comment

#### Low Priority / Future Ideas
- [ ] **Download Integration**: Direct download from songs ticker
  - 'd' key triggers `nts_get URL` for mixtapes
  - Auto-refresh file browser when download completes
- [ ] **Chapter Support**: Parse and display tracklists for downloaded files
- [ ] **Web Remote**: Mobile-friendly web interface via HTTP API

## Technical Notes

### Key Directories
- `crates/radio-proto/` — Shared types, protocol, config
- `crates/radio-daemon/` — Daemon implementation
- `crates/radio-tui/` — Terminal UI implementation

### Config Locations
- macOS/Linux: `~/.config/radio/config.toml`
- Windows Portable: beside `radio-tui.exe`

### Important Files
- `stations.toml` — Station definitions
- `songs.vds` — Song recognition history
- `starred.toml` — Star ratings
- `recent.toml` — Recently played

## Development

```bash
# Build all
cargo build --release

# Run daemon
cargo run -p radio-daemon

# Run TUI
cargo run -p radio-tui
```

See `AGENTS.md` for detailed architecture documentation.
