# r4dio — Project Status & Roadmap

> Developer reference. Updated after v1.0.1 release (2026-02).

---

## Current Status

**v1.0.1 released.** All platforms building and shipping.
| Platform | Binary | Bundled |
|---|---|---|
| macOS arm64 | `r4dio.app` DMG | mpv, ffmpeg, ffprobe, yt-dlp, vibra, starred.toml |
| macOS x86_64 | `r4dio.app` DMG | mpv, ffmpeg, ffprobe, yt-dlp, vibra, starred.toml |
| Linux x86_64 | `r4dio` tarball | mpv (AppImage), ffmpeg, ffprobe, yt-dlp, vibra, starred.toml |
| Windows x86_64 | `r4dio.exe` zip | mpv.exe, ffmpeg.exe, ffprobe.exe, yt-dlp.exe, vibra.exe, DLLs, starred.toml |

**Windows zip layout (v1.0.1):**
```
r4dio-windows-x86_64/
  r4dio.exe, config.toml, README.txt
  external/   ← all executables + mpv DLLs (vibra statically linked — no VCRUNTIME/libcurl/libfftw3 deps)
  data/       ← stations.toml, starred.toml, songs.vds
```

**Working features:**
- Radio streaming via proxy (ICY-aware, HLS detection)
- Local file playback with chapter navigation
- VU meter (PCM/lavfi dual path), oscilloscope, adaptive title lamp
- Song identification via vibra (Shazam-like fingerprinting) + NTS show metadata
- NTS show download via yt-dlp (`d` key in songs ticker) with progress indicators
- NTS live schedule panel (NTS 1 & 2)
- File browser with star ratings, sort, filter
- HTTP remote control API on :8989
- stations.toml + starred.toml bundled and found correctly at runtime; starred.toml auto-seeded on first run
- macOS app bundle: Finder drag-to-Applications, Spotlight indexable, Terminal.app launcher

**Fixed in v1.0.1:**
- ✅ All 4 Windows DLL errors on `i` key: VCRUNTIME140.dll, MSVCP140.dll, libcurl.dll, libfftw3-3.dll — vibra now statically linked (`x64-windows-static` + `/MT`)
- ✅ No ffprobe.exe on Windows — now sourced from ffmpeg-static and bundled in `external/`
- ✅ starred.toml not bundled — now committed to repo and auto-seeded into OS data dir on first run
- ✅ Windows zip cluttered — reorganized into `external/` and `data/` subdirs
- ✅ Radio Valentin Letelier city wrong (Santiago → Valparaiso)

---

## Known Issues

### Audio / Visualisation
- **Scope stutters at high load**: The 30fps `meter_tick` redraw and PCM ring are correct
  but the terminal emulator itself is the bottleneck; nothing in the code can fix this.
  Consider reducing redraw rate to 24fps or making it configurable.
- **VU meter lavfi path (local files) lags behind PCM path (stations)**: The `astats`
  observer fires at mpv's internal rate which is not wall-clock locked. The PCM path
  is more accurate. Consider running ffmpeg PCM tap for local files too.
- **RMS marker interpolation is linear**: `meter_rms_smoothed` in `header.rs` uses
  a fixed lerp factor. A proper attack/release envelope (like the peak marker) would
  look more natural.

### Stations
- **m3u fallback URL points to private repo**: `default_m3u_url()` in `config.rs` still
  references `ja-mf/radio-curation` which is private. Either make that repo public or
  change the fallback to an empty string or a public playlist.
- **Network grouping in TUI**: Stations without `network` field all appear flat. Could
  group/collapse by network in the list for cleaner display when there are many stations.

### macOS App Bundle
- **No code signing**: The `.app` is unsigned. macOS Gatekeeper will quarantine it on
  first launch. Users need to right-click → Open, or `xattr -d com.apple.quarantine`
  the app. Add `codesign` step to CI when a developer certificate is available.
- **Terminal.app launcher only**: The double-click launcher opens `Terminal.app`.
  Users with iTerm2/Warp as default won't get their preferred terminal.
  Consider `open -a "$TERM_PROGRAM" ...` fallback or an AppleScript dialog.
- **DYLD_LIBRARY_PATH in ffmpeg wrappers**: May be stripped by SIP on some macOS
  versions when launching from certain contexts. If ffmpeg fails to run, this is why.

### Windows
- **Windows cross-compilation via cargo-xwin**: Takes ~8 min in CI. Could speed up
  by caching the MSVC sysroot.

---

## Code Quality Findings (post-v1.0 audit)

### 🔴 Critical — Potential Panics

| File | Line | Issue |
|---|---|---|
| `proxy.rs` | 44 | `.expect("failed to build reqwest client")` — panics if TLS init fails |
| `proxy.rs` | 149, 178 | `.unwrap()` on `Response::builder()` — panics on invalid header values |
| `core.rs` | 867 | `.expect("ffmpeg stdout")` — panics if ffmpeg spawned without `Stdio::piped()` |

These three should be converted to proper error propagation (`?` or `anyhow::bail!`).

### 🟡 High — Dead / Orphaned Code

| File | Issue |
|---|---|
| `crates/radio-tui/src/connection.rs` | Entire file is orphaned — not declared as a `mod` in `main.rs`, never compiled. A leftover from the split-process daemon era. **Delete it.** |
| `core.rs:39` | `DaemonEvent::Shutdown` has `#[allow(dead_code)]` — variant is never sent or matched. Remove variant or implement graceful shutdown signal. |
| `core.rs:894` | `fn pcm_rms_db(samples: &[i16])` — defined but never called; RMS is now computed in the app handler. Delete. |
| `crates/radio-daemon/` | Legacy split-process daemon. Duplicates most of `radio-tui/src/core.rs` and `mpv.rs`. Should either be deleted or clearly marked as an archived experiment in its own `README`. |

### 🟡 High — Duplicated Logic

| Issue | Locations |
|---|---|
| `spawn_audio_observer` Unix vs Windows | `mpv.rs:543–621` and `mpv.rs:623–685` — near-identical property-parsing logic duplicated across `#[cfg]` blocks. Extract shared parsing into `parse_rms_event(line: &str) -> Option<f32>` and call from both. |
| `play_station` / `play_file` cleanup pattern | `core.rs:472–567` vs `core.rs:590–650` — both abort the same set of tasks and follow the same error-fallback structure. Extract `teardown_playback(&mut self)` helper. |
| HTTP handler boilerplate | `http.rs:128–174` — every handler is `log → send command → return status`. Extract `send_cmd(tx, cmd) -> impl IntoResponse` closure helper. |
| `load_stations` duplicated in daemon | `radio-tui/src/core.rs:736` and `radio-daemon/src/core.rs:781` — identical function. Should live only in `radio-proto::state` and be imported by both. |

### 🟡 High — Large Functions (Split Candidates)

| Function | Lines | Suggestion |
|---|---|---|
| `app.rs` event loop dispatch | ~2791 lines total; core dispatch ~300 lines | Split into `handle_playback_key()`, `handle_nav_key()`, `handle_search_key()` |
| `core.rs::handle_mpv_event()` | lines 189–318 (~129 lines) | Extract `handle_property_change()` and `handle_lifecycle_event()` |
| `core.rs::play_station()` | lines 472–567 (~95 lines) | Extract `teardown_playback()`, `setup_audio_pipeline()` |
| `proxy.rs::get_or_start_stream()` | lines 58–136 (~78 lines) | Extract `start_stream_pump()` |

### 🟢 Medium — Magic Numbers / Constants

All of these should be named constants with a one-line comment explaining the value:

```rust
// core.rs
const MPV_CONNECT_TIMEOUT_SECS: u64 = 15;   // currently hardcoded `elapsed >= 15`
const VU_WINDOW_SAMPLES: usize = 1024;       // currently named but undocumented
const FFMPEG_RETRY_DELAY_SECS: u64 = 2;      // restart delay on ffmpeg error

// mpv.rs
const MPV_SOCKET_POLL_INTERVAL_MS: u64 = 100;  // 50 iterations × 100ms = 5s max wait
const MPV_SOCKET_MAX_ATTEMPTS: usize = 50;

// proxy.rs  
const PROXY_BROADCAST_CAPACITY: usize = 4096;  // already named, needs doc comment
```

### 🟢 Medium — Architecture

| Issue | Detail |
|---|---|
| **`DaemonCore` god object** | `core.rs` struct holds 15+ fields: mpv driver, state manager, audio tasks, config, broadcast sender. Should split into `PlaybackEngine` (mpv + audio) and `AppCore` (state + config + broadcast). |
| **HTTP `AppState` name collision** | `http.rs:18` defines `struct AppState` which shadows the TUI's `AppState` in the same binary. Rename to `HttpState` or `ApiState`. |
| **Brittle stream type detection** | `core.rs:499`: `!url.contains(".m3u8")` to decide proxy vs direct. Should use a `StreamKind` enum derived from URL parsing. |
| **mpv property IDs as magic ints** | `mpv.rs:42–48`: `OBS_ICY_TITLE = 3`, `OBS_DURATION = 5`, etc. are fine as constants but should have a comment explaining they are r4dio-internal IDs assigned at observer registration, not mpv protocol-defined values. |
| **Broadcast send failures silently ignored** | 40+ places in `core.rs` use `let _ = self.broadcast_tx.send(...)`. Should at least `debug!` log when channel has no receivers, which indicates a bug. |

### 🟢 Low — Cosmetic / Maintenance

- `app.rs` imports are very long; group into `use` blocks by crate
- Several `#[allow(unused_imports)]` likely from iterative development; prune them
- The `scope/` module (`scope_panel.rs` + `scope/`) has a stray `mod.rs` pattern vs the flat-module pattern used elsewhere
- `platform.rs` duplicates the `find_beside_exe` logic that `config.rs::default_stations_toml()` now also implements inline — consolidate

---

## Refactoring Priorities (suggested order)

1. **Delete `connection.rs`** — zero risk, immediate cleanup
2. **Delete `pcm_rms_db` dead function** in `core.rs`
3. **Fix 3 panics** in `proxy.rs` and `core.rs` — convert to error propagation
4. **Rename `http::AppState`** to `HttpState` — 2-line change, prevents future confusion
5. **Extract `teardown_playback()`** in `core.rs` — reduces duplication, improves readability
6. **Consolidate `spawn_audio_observer` platform split** — extract shared parser
7. **Move `load_stations` to `radio-proto::state`** — eliminates daemon duplication
8. **Document or remove `DaemonEvent::Shutdown`** — either wire it up or remove the variant
9. **Name all magic numbers as constants** with explanatory comments
10. **Audit `radio-daemon/`** — decide: delete, or archive with its own README

---

## Future Feature Ideas

### Audio / Visualisation
- **Per-band EQ visualizer**: Replace the linear VU bar with a simple FFT spectrum
  (4–8 bands) using the existing PCM ring buffer. The data is already there.
- **Waveform thumbnail in file list**: Use ffmpeg to extract a small waveform bitmap
  for each audio file when browsing the file list.
- **Configurable VU decay/attack rates**: Currently hardcoded in `header.rs`. Expose
  in `config.toml` under `[display]`.

### Stations / Discovery
- **Editable stations from TUI**: Add an `e` key to open a simple edit form for the
  currently selected station's name/URL/tags. Write back to `stations.toml`.
- **Import from Radio Browser API**: `radio-browser.info` has a public REST API with
  30,000+ stations. A one-shot import command (`r4dio --import-radiobrowser`) would
  let users build their own list.
- **Station health check**: Periodically probe stations in the background and mark
  dead ones with a visual indicator. The proxy already has the infrastructure.

### UX
- **Mouse support**: Ratatui supports mouse events. Clicking a station to select/play
  would be a natural addition.
- **Playlist / queue**: A simple ordered queue of stations/files to play in sequence.
- **Persistent playback position**: Save `time_pos` for local files on exit, resume on
  next play (already partially in place via `PlayFileAt` command).
- **Global hotkey daemon** (macOS/Linux): A small background process that registers
  media key bindings and sends commands to r4dio's HTTP API on :8989.
- **mpris2 / Now Playing integration** (Linux): Expose current track via D-Bus MPRIS2
  so desktop environments show it in their media widget.

### Song Recognition
- **Batch identify**: Run fingerprinting on the last N segments of the PCM ring
  without interrupting playback (currently blocks for 5s).
- **Export songs.vds to CSV/JSON**: Simple `r4dio --export-songs` command.
- **Duplicate detection**: When adding to `songs.vds`, check if same song already
  identified within the last X minutes to avoid redundant entries.

### Distribution
- **macOS code signing**: Requires an Apple Developer account. Once signed, Gatekeeper
  will allow opening without right-click. Notarize for full transparency.
- **Homebrew formula**: Simple `brew install r4dio` via a tap. The DMG/tar.gz
  release already provides the right structure.
- **Flatpak / AppImage** (Linux): The current AppImage for mpv works; package r4dio
  itself as an AppImage or Flatpak for distro-agnostic installation.
- **Windows Store / winget manifest**: Low priority but would improve discoverability.

---

## Architecture Notes (for next developer)

### Audio pipeline (two paths, intentional)

```
Station  → ffmpeg PCM tap → PcmChunk broadcast → pcm_ring + VU
File     → mpv lavfi astats → AudioLevel broadcast → VU only (no ring)
```

The PCM path gives the oscilloscope its data. The lavfi path is simpler but
only gives scalar RMS. Unifying both to PCM (running ffmpeg for files too) is
the cleanest future improvement.

### Where state lives

- `AppState` (in `app_state.rs`): all TUI state — selected indices, pane focus, filter text
- `DaemonState` (in `radio-proto::protocol`): all playback state — current station, volume, ICY title
- `StateManager` (in `radio-proto::state`): `Arc<RwLock<DaemonState>>` wrapper shared between core and HTTP

### Broadcast message flow

```
DaemonCore  →  broadcast_tx  →  App event loop (BroadcastMessage enum)
mpv driver  →  event_tx (mpsc) →  DaemonCore handle_mpv_event()
```

The App event loop is the single consumer of broadcast messages. The HTTP server
sends `Command` variants to `core` via a separate `mpsc` channel.

### CI / release

Workflow: `.github/workflows/build-all-platforms.yml`
- Triggered by `push` to `tags: v*` or `workflow_dispatch`
- 4 compile jobs (linux, win-cross, macos-arm, macos-intel) + 4 vibra builds run in parallel
- Package jobs run after their respective build jobs complete
- `release` job runs last, only on tag pushes, uploads all 4 artifacts
- macOS Intel builds on `macos-14` with Rosetta + x86_64 Homebrew (`/usr/local/bin/brew`)
- Windows cross-compiles via `cargo-xwin` on `ubuntu-latest`

To trigger a new release: `git tag -f vX.Y && git push origin -f vX.Y`

---

## Compiler Warnings — Full Cleanup Plan

**Current state: 74 warnings.** All are dead code, unused imports, or unused
variables from iterative development. None are functional bugs. The goal is zero
warnings with no `#[allow]` suppressions except where the API genuinely requires it.

### Pass 1 — Unused imports (trivial, ~15 warnings)

Each of these is a one-line delete. Work file by file:

| File | Item(s) to remove |
|---|---|
| `app.rs` | `error`, `trace` from tracing imports |
| `app_state.rs` | `PlaybackStatus`, `Severity` |
| `components/station_list.rs` | `Badge`, `C_TAG`, `C_NETWORK` (multiple import lines) |
| `components/file_list.rs` | `Badge`, `C_SECONDARY` |
| `components/file_meta.rs` | `Badge` |
| `components/icy_ticker.rs` | `Badge` |
| `components/help_overlay.rs` | `Constraint`, `Direction`, `Layout` |
| `components/log_panel.rs` | `KeyModifiers`, `Modifier`, `C_PANEL_BORDER_FOCUSED` |
| `components/nts_panel.rs` | `C_SECONDARY` |

Run `cargo fix --bin r4dio` to auto-fix the safe subset, then handle the rest manually.

### Pass 2 — Unused variables (prefix with `_`, ~6 warnings)

| File | Variable | Fix |
|---|---|---|
| `app.rs` | `has_overlay`, `orig_idx` | rename to `_has_overlay`, `_orig_idx` |
| `components/station_list.rs` | `state` | `_state` |
| `components/file_list.rs` | `entry`, `state` | `_entry`, `_state` |
| `core.rs` | `variable does not need to be mutable` | remove `mut` |

### Pass 3 — Dead functions and structs (delete, ~30 warnings)

These are never called and can be safely removed. Verify with a grep before deleting
to confirm no dynamic dispatch or macro usage is involved.

| File | Symbol | Notes |
|---|---|---|
| `core.rs:894` | `fn pcm_rms_db` | Replaced by inline RMS in app handler |
| `theme.rs` | `style_default`, `style_secondary`, `style_accent`, `style_playing`, `style_selected`, `style_selected_focused`, `style_filter`, `style_muted` | Eight unused style helpers. Either wire them up in components or delete. Check if any are intended as a public theming API. |
| `theme.rs` | `C_ERROR`, `C_SEPARATOR`, `C_BADGE_LIVE` | Unused colour constants |
| `components/station_list.rs` | `fn normalize_search_text`, `fn search_matches` | Superseded by the filter in `filter_stations()` |
| `components/station_list.rs` | `methods: selected_original_index, filter_query, is_filter_active` | Public API not consumed by app |
| `components/file_list.rs` | `methods: len, total_len, selected_original_index, filter_query, is_filter_active` | Same — public API not consumed |
| `components/file_meta.rs` | `methods: id, min_height` | Component trait methods defined but never dispatched |
| `app_state.rs` | `struct PlaybackInfo` | Never instantiated |
| `app_state.rs` | `fields: icy_log_path, songs_csv_path, songs_vds_path, tui_log_path, error_message` | Defined in DaemonState mirror, never read in TUI path |
| `app_state.rs` | `method: current_station_name` | Convenience accessor never called |
| `app_state.rs` | `method: dismiss_spinner` | Spinner logic partial |
| `app_state.rs` | `field: index_cursor` | Tracking field never used |
| `proxy.rs` | `field: config` in some struct | Never read |
| `mpv.rs` | `methods: get_pause, ping` | MpvHandle public API never called from TUI |
| `mpv.rs` | `method: is_focused` | Never called |
| `songs_ticker.rs` | `methods: intended, is_confirmed` | Dialog helpers unused |
| `songs_ticker.rs` | `method: show` | Never dispatched |
| `nts_panel.rs` | `method: update_nts_for_idx` | NTS update done differently |
| `nts_panel.rs` | `field: fetched_at`, `fields: show, url, comment` | Partial NTS show metadata fields |
| `songs.rs` | `field: date`, `field: size_bytes` | SongEntry fields never read |
| `workspace.rs` | `method: toggle_workspace` | App calls its own version |
| `action.rs` | `variant: Command`, `variant: HelpOverlay`, `variant: None` | Dead `ComponentId` and `Action` variants |
| `core.rs:39` | `DaemonEvent::Shutdown` | Never sent; either implement graceful shutdown or remove |

### Pass 4 — Fields never read in `DaemonState` mirror (~8 warnings)

Several fields in the TUI's internal `AppState` mirror `DaemonState` but are populated
and never displayed. Decision needed per field: display it, or stop populating it.

Fields: `songs_csv_path`, `songs_vds_path`, `tui_log_path`, `error_message`,
`icy_log_path` — these look like paths for a future "settings panel". Either build
that panel (see roadmap) or delete the fields.

### Pass 5 — Delete orphaned files

| File | Action |
|---|---|
| `crates/radio-tui/src/connection.rs` | **Delete.** Not declared as a `mod`, not compiled. |
| `crates/radio-daemon/` | Audit and either delete or add `# ARCHIVED` README. All its logic is duplicated in radio-tui. |

### Expected result

After all 5 passes: `0 warnings`. Add `#![deny(warnings)]` to `main.rs` and
`lib.rs` to prevent regression. Or lighter: add it only to CI via `RUSTFLAGS=-D warnings`.

---

## Realtime Architecture & Pipeline

### Current pipeline (stations)

```
Radio server (internet)
    │
    ├─ Connection A → reqwest (inside proxy.rs)
    │       │
    │       └─ broadcast::channel<Bytes> (cap: 4096 msgs)
    │               │
    │               └─ mpv ← HTTP GET http://127.0.0.1:8990/stream/:idx
    │                   └─ audio output (speakers)
    │
    └─ Connection B → ffmpeg -i <real_url> (core.rs::run_vu_ffmpeg)
            │
            └─ stdout pipe → 1024-sample chunks → PcmChunk broadcast
                    │
                    └─ app.rs handler → pcm_ring (88200 f32, ~2s) → VU + scope
```

**Key problem**: two independent TCP connections to the radio server. mpv receives
audio through the proxy; ffmpeg receives it directly. They are NOT synchronised —
they start at different points in the live stream and buffer differently. The VU meter
and scope show what ffmpeg is receiving, not what mpv is playing. For most music the
drift is imperceptible, but it is a correctness issue and doubles server load.

### Current pipeline (local files)

```
File on disk
    └─ mpv (direct path)
            ├─ audio output
            └─ lavfi astats observer → AudioLevel (scalar dBFS, ~10Hz) → VU only
                                                                           (no pcm_ring → no scope)
```

The oscilloscope is unavailable for local files because we don't have a PCM stream.
Running `ffmpeg -i <file>` in parallel would give the same PCM pipe and enable scope
for files too.

### The synchronisation fix: ffmpeg via proxy

Route the ffmpeg PCM tap through the proxy instead of the real URL:

```rust
// core.rs::play_station — change this:
let pcm_url = station.url.clone();                // direct, unsynced
// to this:
let pcm_url = proxy_url(idx);                     // same source as mpv
```

**Result**: both mpv and ffmpeg receive from the same broadcast channel, fed by a
single upstream connection. The proxy already handles multiple subscribers. The VU
meter and scope will reflect exactly what is playing.

**Risk to evaluate**: if ffmpeg reads slower than mpv (e.g. its PCM decode loop
blocks), it will accumulate lag in the proxy broadcast channel. The channel's `Lagged`
error handler already recovers by skipping ahead, but this causes a gap in the PCM
stream and a VU hiccup. Mitigations:
- Make the PCM read loop non-blocking / yield to tokio on each chunk
- Increase `PROXY_BROADCAST_CAPACITY` slightly (see buffer table below)
- The ffmpeg PCM task runs in its own tokio task, so it shouldn't block mpv's path

### Buffer inventory and tuning

Every buffer in the chain is a potential source of latency or instability. Here is
the full inventory, current values, rationale, and tuning advice:

| Layer | Buffer | Current | Effect of increasing | Effect of decreasing |
|---|---|---|---|---|
| **reqwest body stream** | OS TCP recv buffer | ~128KB (kernel default) | More resilient to server bursts | Lower memory, higher dropout risk |
| **Proxy broadcast channel** | `PROXY_BROADCAST_CAPACITY = 4096` | 4096 messages (~32MB if 8KB chunks) | Slow subscribers can fall further behind without Lagged | Subscribers lag and skip more often |
| **mpv demuxer cache** | mpv default ~2MB | More pre-buffering, higher latency to live edge | Lower latency, more dropouts on jitter |
| **ffmpeg network buffer** | `nobuffer` + `-rtbufsize` not set (default ~3.5MB) | Absorbs jitter | Tighter latency |
| **ffmpeg probe** | `-probesize 64k`, `-analyzeduration 200000` (0.2s) | More format info, longer startup | Faster startup, may mis-identify codec |
| **PCM chunk size** | `VU_WINDOW_SAMPLES = 1024` at 44100Hz = **23ms** | Chunkier VU updates | Finer updates, more allocations/s |
| **PCM ring** | `PCM_RING_MAX = 88200` (2s at 44100Hz) | Longer oscilloscope history | Less history |
| **Meter display tick** | `METER_FPS = 25` (40ms) | Smoother appearance | Less CPU |

**Recommended config additions** (expose in `config.toml` under `[display]` and `[audio]`):

```toml
[audio]
# PCM tap sample rate. Lower values reduce CPU and broadcast pressure at the cost
# of frequency resolution in the oscilloscope. 22050 is a sweet spot.
# Valid: 11025 | 22050 | 44100
pcm_sample_rate = 22050

# PCM chunk size sent per broadcast message (samples).
# 512 @ 22050Hz = ~23ms per chunk ≈ 43 messages/s.
# Smaller = more responsive VU, more broadcast overhead.
pcm_chunk_samples = 512

# Oscilloscope ring buffer length in seconds.
pcm_ring_secs = 2

[display]
# Meter and scope redraw rate (fps). 25 is smooth without burning CPU.
meter_fps = 25

[proxy]
# Broadcast channel depth (number of chunks). Each chunk is ~pcm_chunk_samples * 2 bytes.
# 128 gives ~3s of lag tolerance at 44kHz/512-sample chunks before Lagged fires.
broadcast_capacity = 128
```

### ffmpeg startup latency

The current `-analyzeduration 200000` (0.2s) means the VU meter is dark for 200ms
after a station starts. This is acceptable but can be felt. Consider:
- `-analyzeduration 0` + `-probesize 32` for raw PCM-like streams (Icecast/MP3)
- Keep `200000` for HLS where container detection matters
- Detect stream type from content-type header before spawning ffmpeg

The proxy already has the `content-type` header (forwarded from ICY response). Pass
it to the ffmpeg spawner to select the right flags.

### Proxy stability for remote streams

Current behaviour when the upstream drops:

```
upstream error → proxy pump logs warn and breaks → stream removed from map
    → mpv receives EOF → mpv stops → TUI shows "stopped"
    → user must press Enter to reconnect
```

This is correct but not resilient. Improvements:

1. **Automatic reconnect in proxy pump**: On upstream error, wait 1–2s and reconnect
   to the same URL. Keep broadcasting silence (zero bytes) during the gap so mpv
   doesn't EOF. Log the reconnect attempt.

2. **Jitter buffer at proxy level**: Accumulate 2–3 seconds of compressed audio
   before starting to forward. This absorbs short network hiccups. Implemented by
   buffering the first N chunks before creating the broadcast. Trade-off: 2s startup
   latency.

3. **Health check loop**: Every 30s, test that the upstream is still responsive with
   a HEAD request. If not, proactively reconnect before mpv notices.

4. **Exponential backoff for ffmpeg restart**: Currently `core.rs` restarts ffmpeg
   every 2s on error (hardcoded `FFMPEG_RETRY_DELAY_SECS`). Should use exponential
   backoff: 1s, 2s, 4s, 8s, cap at 30s.

### The lavfi → PCM unification

For local files, add a PCM tap using ffmpeg (same as stations) to populate `pcm_ring`
and enable the oscilloscope. This replaces the lavfi `astats` path entirely:

```
mpv plays file → separately: ffmpeg -i <path> -nostdin -fflags nobuffer
                                     -ac 1 -ar 22050 -f s16le pipe:1
              → PcmChunk broadcast → pcm_ring → VU + scope
```

This makes the audio pipeline uniform: stations and files both use the same PCM
broadcast path. The lavfi observer can then be removed entirely.

Caveat: ffmpeg opens the file independently from mpv. Seeking in mpv won't move
ffmpeg's position. The PCM tap would go out of sync after a seek. Mitigation: restart
ffmpeg at the new seek position whenever `Command::SeekTo` is sent.

### Architecture target (v1.1 / v2)

```
                        ┌─────────────────────────────────┐
                        │  Single upstream connection per  │
                        │  station (reqwest in proxy.rs)   │
                        └──────────────┬──────────────────┘
                                       │ broadcast::channel<Bytes>
                         ┌─────────────┴──────────────┐
                         ▼                            ▼
               mpv ← proxy HTTP              ffmpeg PCM tap
               (audio output)                ← proxy HTTP (same source)
                                             └─ PcmChunk broadcast
                                                 ├─ VU meter (RMS)
                                                 └─ scope (ring buffer)

Local files:
  mpv plays file ──────────────────── audio output
  ffmpeg -i file (parallel) ────────  PcmChunk broadcast (same path as above)
  On seek: restart ffmpeg at new pos
```

This gives:
- Single upstream TCP connection (saves bandwidth, reduces server load)
- Exact VU/scope sync with audible output
- Uniform pipeline for files and stations
- All buffers controlled from `config.toml`

