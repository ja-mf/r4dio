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
├── starred.toml                 # Personal station/file star ratings (bundled in releases)
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

### NTS download (`src/nts_download/` + `src/download_manager.rs`)
- `src/download_manager.rs` — concurrent download manager, progress tracking
- `src/nts_download/mod.rs` — public API: `download_episode(url, dir, yt_dlp)`
- `src/nts_download/api.rs` — NTS API client (fetches episode metadata)
- `src/nts_download/parser.rs` — HTML/JSON parser for NTS episode pages
- `src/nts_download/download.rs` — yt-dlp invocation and output capture
- `src/nts_download/metadata.rs` — audio tag embedding via `lofty`
- `src/nts_download/tests.rs` — unit tests

Downloads saved to `~/radio-downloads/`. Binary found via `find_yt_dlp_binary()` in `platform.rs` (checks `YT_DLP_PATH` env, beside exe, `external/` subdir, PATH).

**Key flows:**
- `play_station(idx)`: aborts lavfi observer, aborts old VU task, loads proxy URL into mpv, spawns `run_vu_ffmpeg` against real station URL (⚠️ not yet synced via proxy)
- `play_file(path)`: aborts VU task, loads file into mpv, spawns `spawn_audio_observer` (lavfi) for audio level
- `stop()`: aborts VU task and lavfi observer, stops mpv stream
- `ensure_mpv_handle()`: reconnects to existing mpv socket or spawns fresh mpv; single forwarding task per connection (no fan-out)

**⚠️ Code quality**: `core.rs` is a ~900-line god object. `play_station()` and `play_file()` share identical teardown logic. Extract `teardown_playback()` helper. See PROJECT.md for full refactoring priorities.

### Audio pipeline

**⚠️ Known issue (v1.0): Dual-connection desync**

The current design opens **two independent TCP connections** to the radio server:
- **Connection A** (via proxy.rs): feeds mpv the audio
- **Connection B** (via ffmpeg in core.rs): feeds the VU meter and oscilloscope

These are NOT synchronised — they buffer differently and start at different points in the live stream. **The VU/scope shows what ffmpeg is receiving, not what mpv is playing.**

**Planned fix (v1.1)**: Route ffmpeg's PCM tap through the same proxy as mpv. Change `run_vu_ffmpeg()` to use `proxy_url(idx)` instead of the real station URL. This makes both mpv and ffmpeg read from the same upstream source.

**Stations (PCM source of truth — to be unified):**
```
ffmpeg -i <station_url> -ac 1 -ar 22050 -f s16le pipe:1
  → 1024-sample chunks (~46ms at 22050Hz) → PcmChunk broadcast
  → app.rs PcmChunk handler: push to pcm_ring, compute RMS → update_audio_trackers()
```

**Local files (lavfi path — to be replaced with PCM):**
```
mpv lavfi.astats.Overall.RMS_level property observer
  → AudioLevel broadcast → update_audio_trackers()
```

**Planned unification (v1.1)**: Run ffmpeg PCM tap for local files too (currently lavfi only).
This enables the oscilloscope for files and makes both paths identical.

`update_audio_trackers()` maintains: `audio_level`, `peak_level` (fast attack, 6 dB/s decay), `meter_mean_db` (EMA τ=4s), `meter_spread_db` (EMA τ=8s).

### Oscilloscope (`src/scope/` + `src/components/scope_panel.rs`)
- Ring buffer: `AppState.pcm_ring: VecDeque<f32>` — 88200 samples (~2s at 22050 Hz)
- Toggle: `o` key → `RightPane::Scope`; scope occupies right half of the 2-row header
- Keys when focused: `Up`/`Down` scale ±0.01 (×10 Shift), `Left`/`Right` samples ±25, `Esc` reset
- Redraws at 30fps via `meter_tick` (40ms); `PcmChunk` messages never trigger a redraw
- **v1.0 limitation**: Scope only works for stations (lavfi path for files has no PCM). Planned: run ffmpeg for files too (see Audio pipeline notes).

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
d              Download NTS show (yt-dlp, songs ticker only)

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

---

## Known Issues & Improvement Opportunities (post-v1.0)

### Critical panics to fix
Three `expect()` / `unwrap()` calls that can crash the app:
- `proxy.rs:44` — `.expect()` on reqwest client builder
- `proxy.rs:149,178` — `.unwrap()` on Response builder
- `core.rs:867` — `.expect("ffmpeg stdout")` after spawn

All should convert to proper error propagation.

### High-priority refactoring
1. Delete `crates/radio-tui/src/connection.rs` — orphaned file, not compiled
2. Delete unused function `core.rs::pcm_rms_db()` at line 894
3. Extract `teardown_playback()` helper in `core.rs` to reduce duplication between `play_station()` and `play_file()`
4. Rename `http::AppState` to `HttpState` to avoid shadowing the TUI's `AppState`
5. Consolidate duplicated platform logic in `mpv.rs::spawn_audio_observer` (Unix vs Windows property parsing)

See PROJECT.md for the full 5-pass compiler warnings cleanup plan (74 total warnings, all dead code).

### Architecture improvements
- **DaemonCore god object**: `core.rs` struct has 15+ fields and ~900 lines. Split into `PlaybackEngine` (mpv + audio) and `AppCore` (state + config + broadcast).
- **VU/scope sync via proxy**: ffmpeg PCM tap should use `proxy_url(idx)` instead of the real station URL (currently causes dual-connection desync — see Audio pipeline notes)
- **Oscilloscope for files**: Run ffmpeg PCM tap for local files too, replacing the lavfi path (currently files have no scope, only VU meter)
- **m3u fallback URL**: `config.rs::default_m3u_url()` points to private `ja-mf/radio-curation` repo — either make it public or change to empty string
- **macOS code signing**: App is unsigned; users need `xattr -d com.apple.quarantine` to bypass Gatekeeper
- **Windows ffprobe**: Not bundled in shinchiro ffmpeg archive; file browser metadata will fail silently

All issues and detailed solutions are documented in PROJECT.md.


## Platform Notes

**macOS:** `~/.config/radio/`, mpv socket at `$TMPDIR/radio-mpv.sock`

**Linux:** XDG paths, mpv socket at `/tmp/radio-mpv.sock`

**Windows:** Portable mode — `config.toml` beside `r4dio.exe`, named pipes for mpv IPC.
Release zip layout:
```
r4dio-windows-x86_64/
  r4dio.exe, config.toml, README.txt
  external/   ← mpv.exe, ffmpeg.exe, ffprobe.exe, yt-dlp.exe, vibra.exe + mpv DLLs
  data/        ← stations.toml, starred.toml, songs.vds
```
`find_beside_exe()` in `platform.rs` searches both the exe dir and `external/` subdir.
`starred.toml` and `songs.vds` are auto-seeded from `data/` into the OS data dir on first run.
