# AGENTS.md — r4dio Architecture Guide

Technical reference for developers working on the r4dio codebase.

## Architecture: Single Binary

`r4dio` is a **monolithic binary** built from `crates/radio-tui`. The TUI and all playback logic run in the same process — there is no separate daemon. mpv is spawned and controlled directly from inside `r4dio`.

```
r4dio  (crates/radio-tui)
  ├── spawns → mpv              (audio playback, IPC socket)
  ├── spawns → ffmpeg           (PCM tap for VU meter + oscilloscope, stations only)
  ├── spawns → ffmpeg           (song fingerprinting via vibra)
  ├── serves → HTTP :8989       (optional remote control API)
  └── serves → HTTP :8990       (stream proxy — rewrites station URLs for mpv)
```

`crates/radio-daemon` is a legacy split-process daemon. It still builds and is kept for reference but is **not the primary binary and not the focus of active development**. Do not add features there.

`src/` is an older architectural prototype. It is not compiled by the workspace and can be ignored.

## Directory Structure

```
r4dio/
├── Cargo.toml                   # Workspace manifest (members: crates/*)
├── config.toml                  # Reference user config
├── stations.toml                # Station definitions
│
├── crates/
│   ├── radio-proto/             # Shared types: DaemonState, Message, Config, Station, songs
│   ├── radio-tui/               # ← PRIMARY BINARY (r4dio): TUI + playback + HTTP
│   └── radio-daemon/            # Legacy split-process daemon (not active)
│
└── src/                         # Old prototype (not compiled, ignore)
```

## Active Codebase: `crates/radio-tui/`

### Entry point & event loop
- `src/main.rs` — binary entry, tokio runtime, initialises `App`
- `src/app.rs` — `App` struct, main event loop, message dispatch, draw calls
- `src/app_state.rs` — `AppState`: all UI + playback state

### Playback engine (`src/core.rs` + `src/mpv.rs`)
- `src/core.rs` — `DaemonCore`: mpv lifecycle, command handling, VU/PCM ffmpeg task
- `src/mpv.rs` — `MpvDriver` / `MpvHandle`: spawn mpv, JSON IPC socket, property observers

**Key flows:**
- `play_station(idx)`: aborts lavfi observer, aborts old VU task, loads proxy URL into mpv, spawns `run_vu_ffmpeg` against real station URL
- `play_file(path)`: aborts VU task, loads file into mpv, spawns `spawn_audio_observer` (lavfi) for audio level
- `stop()`: aborts VU task and lavfi observer, stops mpv stream
- `ensure_mpv_handle()`: reconnects to existing mpv socket or spawns fresh mpv; single forwarding task per connection (no fan-out)

### Audio pipeline

**Stations (PCM source of truth):**
```
ffmpeg -i <station_url> -ac 1 -ar 11025 -f s16le pipe:1
  → 512-sample chunks (~46ms) → PcmChunk broadcast
  → app.rs PcmChunk handler: push to pcm_ring, compute RMS → update_audio_trackers()
```

**Local files (lavfi source of truth):**
```
mpv lavfi.astats.Overall.RMS_level property observer
  → AudioLevel broadcast → update_audio_trackers()
```

`update_audio_trackers()` maintains: `audio_level`, `peak_level` (fast attack, 6 dB/s decay), `meter_mean_db` (EMA τ=4s), `meter_spread_db` (EMA τ=8s).

### Oscilloscope (`src/scope/` + `src/components/scope_panel.rs`)
- Ring buffer: `AppState.pcm_ring: VecDeque<f32>` — 22050 samples (~2s at 11025 Hz)
- Toggle: `o` key → `RightPane::Scope`; scope occupies right half of the 2-row header
- Keys when focused: `Up`/`Down` scale ±0.01 (×10 Shift), `Left`/`Right` samples ±25, `Esc` reset
- Redraws at 30fps via `meter_tick` (33ms); `PcmChunk` messages never trigger a redraw

### Network services (in-process)
- `src/proxy.rs` — Axum HTTP proxy on port 8990: `GET /stream/:idx` rewrites station URL for mpv, forwards ICY headers
- `src/http.rs` — HTTP REST API on port 8989: optional remote control

### UI components (`src/components/`)

| Component | File | Purpose |
|-----------|------|---------|
| Header | `header.rs` | 2-row top bar: station/file info (row 1), VU meter + seek (row 2) |
| StationList | `station_list.rs` | Left pane, radio workspace |
| FileList | `file_list.rs` | Left pane, files workspace |
| IcyTicker | `icy_ticker.rs` | Right pane: scrolling ICY title history |
| SongsTicker | `songs_ticker.rs` | Right pane: identified songs |
| NtsPanel | `nts_panel.rs` | Right pane: NTS live schedule |
| FileMeta | `file_meta.rs` | Right pane: file metadata / chapters |
| LogPanel | `log_panel.rs` | Right pane: log stream |
| ScopePanel | `scope_panel.rs` | Header right half: oscilloscope |
| HelpOverlay | `help_overlay.rs` | Full-screen key reference |

### Workspace / pane layout

```
┌─ header left (station / ICY / seek) ─┬─ header right (scope, when active) ─┐
├─ left pane ──────────────────────────┴─ right pane ─────────────────────────┤
│ StationList or FileList               │ IcyTicker | SongsTicker | NtsPanel   │
│                                       │ FileMeta | LogPanel                  │
├─ status bar ──────────────────────────────────────────────────────────────── ┤
│ Mode + key hints                                                              │
└───────────────────────────────────────────────────────────────────────────── ┘
```

- `WorkspaceManager` (`src/workspace.rs`): tracks `Workspace` (Radio/Files), `RightPane` (Tickers/Nts1/Nts2/Scope), collapsed panes, focus ring
- When `RightPane::Scope`: header splits 50/50 left/right; body is full-width station list

## Shared types: `crates/radio-proto/`

- `protocol.rs` — `DaemonState`, `Command`, `Broadcast`, `PlaybackStatus`, `Station`, `Message` (length-prefixed JSON)
- `config.rs` — `Config`, `DaemonConfig`, `HttpConfig`, `MpvConfig`, `StationsConfig`
- `songs.rs` — `SongDatabase`, `SongEntry`, `recognize_via_vibra()`, `recognize_via_nts()`
- `state.rs` — `StateManager` (`Arc<RwLock<DaemonState>>` wrapper)
- `platform.rs` — `config_dir()`, `data_dir()`, `cache_dir()`

### DaemonState
```rust
pub struct DaemonState {
    pub stations: Vec<Station>,
    pub current_station: Option<usize>,
    pub current_file: Option<String>,
    pub playback_status: PlaybackStatus,
    pub volume: f32,
    pub icy_title: Option<String>,
    pub mpv_health: MpvHealth,
    pub time_pos_secs: Option<f64>,
    pub duration_secs: Option<f64>,
}
```

### Internal broadcast messages (`BroadcastMessage` in `src/main.rs`)
- `StateUpdated` — full state snapshot changed
- `IcyUpdated` — ICY title changed
- `Log(String, Severity)` — log line
- `AudioLevel(f32)` — RMS dBFS (lavfi path, files only)
- `PcmChunk(Arc<Vec<f32>>)` — 512 normalised f32 samples (ffmpeg path, stations only)

### NtsChannel / NtsShow
Fetched from `https://www.nts.live/api/v2/live` every 60s.

### SongEntry (`crates/radio-proto/src/songs.rs`)
Stored in `songs.vds` (tab-separated): `job_id timestamp station icy_info nts_show nts_tag nts_url vibra_rec`
`display()` priority: `vibra_rec` > `icy_info` > `"?"`

## Keybindings

```
Space          Toggle pause/play
n/p            Next/previous station/file
r              Random
R              Go back (random history)
m              Mute
←/→ or -/+     Volume down/up (5% steps)
,/.            Seek -30s/+30s (files only)
Shift+,/.      Seek -5min/+5min
f              Toggle workspace (Radio ↔ Files)
s/S            Cycle sort / reverse
*              Cycle star rating (0-3)
Tab            Next pane
1/2/3/4        Focus pane by number
!/@            Toggle NTS 1/2 panel
o              Toggle oscilloscope
/              Open filter
Esc            Clear filter / close overlay
?              Toggle help
q              Quit
i              Identify song (fingerprint)

Scope (when focused):
  Up/Down        Scale ±0.01 (×10 with Shift)
  Left/Right     Sample window ±25 (×10 with Shift)
  Esc            Reset scale=1.0, samples=2048
```

## Adding New Features

**New keybinding:**
1. Handle in the relevant `Component::handle_key()` in `src/components/`
2. Update key hints in the status bar component

**New panel:**
1. Create component implementing `Component` trait in `src/components/`
2. Add `ComponentId` variant in `src/action.rs`
3. Add `RightPane` variant if needed in `src/workspace.rs`
4. Wire into `WorkspaceManager` focus ring and `draw_radio` in `src/app.rs`

**New config option:**
1. Add field to appropriate struct in `crates/radio-proto/src/config.rs`
2. Implement `Default`
3. Access via `app_state.config.*`

**New playback command:**
1. Add variant to `Command` enum in `crates/radio-proto/src/protocol.rs`
2. Handle in `DaemonCore::handle_command()` in `src/core.rs`
3. Dispatch from TUI via `app.send_command(Command::...)`

**New sort mode:**
1. Add variant to `SortOrder` in `src/`
2. Update `next()` / `prev()` cycle
3. Add `apply_sort()` / `label()` logic

## Testing & Debugging

**Logs:**
- `~/.local/share/radio/tui.log`

**Environment:**
- `RUST_LOG=debug` — verbose logging
- `RUST_LOG=debug,hyper_util=warn,reqwest=warn` — suppress HTTP noise

**Run from source:**
```bash
cargo build
./target/debug/r4dio

# or
RUST_LOG=debug cargo run -p radio-tui
```

**Common issues:**
- "mpv not found" — ensure mpv is in PATH
- "vibra not found" — install vibra or set `VIBRA_PATH`
- No audio — check mpv audio output config (`ao`)
- Flat VU/scope — check ffmpeg is in PATH; stream must be playing

## Platform Notes

**macOS:** `~/.config/radio/`, mpv socket at `$TMPDIR/radio-mpv.sock`

**Linux:** XDG paths, mpv socket at `/tmp/radio-mpv.sock`

**Windows:** Portable mode — `config.toml` beside `r4dio.exe`, named pipes for mpv IPC
