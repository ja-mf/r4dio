# scope-tui Integration Plan

Integration of [scope-tui](https://github.com/alemidev/scope-tui) as an oscilloscope widget
embedded in the r4dio TUI header area.

**Status:** Planned, not implemented.

---

## What we want

- Right half of the 2-row header, spanning both rows
- Oscilloscope (default), vectorscope, spectroscope — user cycles with `Tab`
- Focusable with key `5` (hidden — no hint in status bar)
- When focused: full scope-tui keybindings (`h`, `Tab`, `Up`/`Down` zoom,
  `Shift+Up/Down` coarse zoom, `t`/`e`/`p` trigger controls, `Space` pause, `Esc` reset, etc.)
- When not playing a file: pane is dark/empty
- Default display mode: Oscilloscope, initial scale: 0.5 (normalized [-1, 1] with headroom)

---

## scope-tui architecture (relevant parts)

scope-tui has a proper lib target (`scope` crate, `src/lib.rs`). The rendering is cleanly
separated from audio capture via a `DataSource<f64>` trait.

**Key public types:**
- `scope::display::GraphConfig` — shared rendering state (scale, samples, palette, etc.)
- `scope::display::DisplayMode` trait — `process(&mut self, cfg, &Matrix<f64>) -> Vec<DataSet>`
- `scope::display::oscilloscope::Oscilloscope` — time-domain, triggering
- `scope::display::vectorscope::Vectorscope` — XY Lissajous
- `scope::display::spectroscope::Spectroscope` — FFT frequency
- `scope::input::Matrix<f64>` = `Vec<Vec<f64>>` — outer = channels, inner = samples
- `scope::input::stream_to_matrix()` — demultiplex interleaved PCM

Rendering pipeline: `DisplayMode::process()` returns `Vec<DataSet>` → converted to ratatui
`Chart` widget via `From<&DataSet> for Dataset`. The crate does **not** implement ratatui's
`Widget` trait directly — you build and render the `Chart` yourself.

---

## Dependency compatibility

| Dep | scope-tui | r4dio | Resolution |
|-----|-----------|-------|------------|
| ratatui | 0.29 | 0.30 | bump scope-tui to 0.30 — no API changes needed |
| crossterm | 0.29 (direct dep) | 0.28 (via ratatui) | see below |

**crossterm version conflict:** scope-tui's `display/*.rs` imports
`use crossterm::event::{Event, KeyCode, KeyModifiers}` from its own direct dep (0.29).
r4dio uses `ratatui::crossterm` (0.28). These are different Cargo crate instances — their
`Event`/`KeyEvent` types are not interchangeable at the type system level, even though the
structs are structurally identical between 0.28 and 0.29.

**Fix (2 files):**
1. `Cargo.toml`: change `crossterm = "0.29"` → `"0.28"`, remove the direct dep entirely
2. In all `src/display/*.rs` and `src/app.rs`: `use crossterm::` → `use ratatui::crossterm::`

This unifies on a single crossterm instance. Total diff: ~6 lines across 5 files.

**Cargo.toml dep to add in radio-tui:**
```toml
scope = { path = "../../../../gh/scope-tui", default-features = false, features = ["tui"] }
```

The `tui` feature includes only `display/`, `input/`, `music/` — no clap, no audio backends.

---

## The PCM source problem

The oscilloscope needs raw PCM samples (`Matrix<f64>`). The VU meter uses
`lavfi.astats.Overall.RMS_level` — a scalar statistic from mpv's filter graph. These are
fundamentally different things. You cannot reconstruct a waveform from an RMS value.

### Why getting PCM out of mpv while it plays is hard

mpv has no built-in mechanism to simultaneously play audio to speakers **and** export raw PCM
to an external consumer:

- `--ao=pcm --ao-pcm-file=<path>` works (streams s16le to a named pipe) but **replaces**
  the normal audio output — no sound.
- `--lavfi-complex` can split the audio signal (`asplit`) but the tapped branch has no
  ffmpeg filter to write it to a file or pipe (`apipe` does not exist in lavfi).
- mpv's IPC does not expose audio sample data as a property.
- Multiple `--ao` drivers simultaneously is not supported.

### Options for the PCM source

#### Option A — Second ffmpeg to stream URL (simplest)

Spawn `ffmpeg -i <stream_url> -vn -ar 44100 -ac 2 -f s16le pipe:1` in parallel with mpv.
Feed its stdout into `stream_to_matrix()`.

**Pros:** Simple, always works, no user setup.

**Cons:**
- Opens a second HTTP connection to the station. Doubles bandwidth.
- **Sync is not guaranteed.** Radio streams buffer at the protocol level (HLS segments are
  3–10s each; Icecast has a connection buffer). The second ffmpeg connection starts at a
  different buffer position. The scope will show the right *shape* of the signal but may
  lag or lead what you hear by anywhere from 0 to 10+ seconds, and it drifts over time.
- For local files: perfect sync (ffmpeg reads the same file path, seeking to the same
  position). This is why **files only** is the practical starting point.

#### Option B — Files only (current recommendation for Phase 1)

`ffmpeg -i <file_path> -ss <time_pos> -vn -ar 44100 -ac 2 -f s16le pipe:1`

Perfect sync — ffmpeg reads the local file at the same offset mpv is at. No network.
Scope is simply empty/dark when a radio station is playing.

**Cons:** No waveform for radio.

#### Option C — CPAL loopback capture

Capture from the system audio output device using CPAL. On macOS this requires installing
BlackHole or creating an aggregate device. On Linux it requires PulseAudio monitor sources
or JACK. Not portable, requires user setup.

**Verdict:** Not worth it for a visualizer.

#### Option D — HTTP stream proxy with shared ring buffer (best long-term, most work)

The daemon becomes a local HTTP stream proxy. Both mpv and ffmpeg connect to
`localhost:8990/<station_idx>`. The daemon fetches the upstream stream once, tees the bytes
into a broadcast ring buffer, and serves both consumers from the same read position.

**How it works:**
1. When a station is selected, daemon opens a single upstream HTTP connection (reqwest streaming)
2. Incoming bytes are written to a `Arc<Mutex<RingBuf>>` and broadcast via
   `tokio::sync::broadcast::channel`
3. mpv is told to play `http://localhost:8990/<idx>` instead of the direct station URL
4. The scope feed task subscribes to the broadcast and reads the same bytes

Both consumers see identical bytes in the same order → **perfect sync**.

**Implementation complexity:**
- ~200–400 lines of async Rust in the daemon
- Need to handle: ICY metadata passthrough, chunked transfer encoding, HLS vs Icecast
  (HLS rewrites segment URLs — harder to proxy), reconnect on upstream drop, backpressure
  if a consumer is slow, correct Content-Type forwarding
- HLS streams (NTS uses HLS) are segment-based — each segment is a separate HTTP request.
  A true HLS proxy must intercept the playlist and rewrite segment URLs, then proxy each
  segment fetch. This is significantly more complex than proxying a raw Icecast stream.
- mpv currently connects directly to station URLs — need to thread the proxy URL through
  the play command path

**Verdict:** Architecturally correct, future-proof, enables recording and other features.
Non-trivial but tractable. Treat as a separate feature (Stage 0 of a "daemon audio bus").

---

## Implementation stages

### Stage 0 (prerequisite for radio sync) — HTTP stream proxy
*(optional, skippable if radio scope is acceptable without sync)*

In `radio-daemon`:
- New `StreamProxy` struct: upstream reqwest stream → broadcast channel
- axum route `GET /stream/:idx` — serves proxied bytes to N subscribers
- Change `Command::Play(idx)` to pass `http://localhost:8990/<idx>` to mpv instead of
  the direct URL

In `radio-tui`:
- Scope feed task subscribes to the broadcast channel (or connects to `localhost:8990/<idx>`)
- No ffmpeg needed for streams — daemon hands bytes directly

### Stage 1 — Patch scope-tui (local fork)

Files to change in `~/gh/scope-tui`:
- `Cargo.toml`: ratatui 0.30, crossterm 0.28 (or remove direct dep entirely)
- `src/display/mod.rs`, `oscilloscope.rs`, `spectroscope.rs`, `app.rs`:
  `use crossterm::` → `use ratatui::crossterm::`

### Stage 2 — Wire dep and add scope state

`crates/radio-tui/Cargo.toml`:
```toml
scope = { path = "../../../../gh/scope-tui", default-features = false, features = ["tui"] }
```

`app_state.rs`:
```rust
pub scope_buffer: VecDeque<Vec<f64>>,  // ring buffer, max ~8192 samples per channel
pub scope_active: bool,
```

`app.rs` `AppMessage`:
```rust
ScopeFrame(Vec<Vec<f64>>),
```

### Stage 3 — ffmpeg PCM feed task (files only)

New async fn `scope_feed_task(path: String, pos_secs: f64, tx)`:
- Spawns `ffmpeg -i <path> -ss <pos> -vn -ar 44100 -ac 2 -f s16le pipe:1`
- Reads 4096-byte chunks, calls `scope::input::stream_to_matrix()` (s16le, 2ch, /32768.0)
- Sends `AppMessage::ScopeFrame(matrix)` per chunk (~23ms of audio at 44100 Hz)
- Loops until EOF or task is aborted

Lifecycle in `app.rs`:
- `StateUpdated` with `current_file.is_some() && Playing` → abort old task, spawn new
- `StateUpdated` with `!Playing` or `current_file.is_none()` → abort task, clear buffer

### Stage 4 — ScopePane component

New `crates/radio-tui/src/components/scope_pane.rs`:

```rust
pub struct ScopePane {
    graph: GraphConfig,
    oscilloscope: Oscilloscope,
    vectorscope: Vectorscope,
    spectroscope: Spectroscope,
    mode: ScopePaneMode,  // Oscillo | Vector | Spectro
}
```

- `handle_key`: forward to `current_display_mut().handle(Event::Key(key))` for mode-specific
  keys; handle Tab (cycle mode), h, r, Space, Up/Down/Shift+Up/Down (zoom), Esc (reset)
  directly on `self.graph`
- `draw`: snapshot `state.scope_buffer` → `Matrix<f64>` → `process()` → `Chart` → render.
  If `!state.scope_active`: render blank area. Subtle 1px border when focused.
- `id()` → `ComponentId::ScopePane`

`GraphConfig` init:
```rust
GraphConfig {
    samples: 4096,
    sampling_rate: 44100,
    scale: 0.5,           // [-1,1] normalized, 0.5 = some headroom
    scatter: false,
    references: false,
    show_ui: false,
    marker_type: Marker::Braille,
    palette: vec![Color::Rgb(30, 140, 90), Color::Rgb(100, 160, 200)],
    labels_color: Color::Rgb(80, 80, 100),
    axis_color: Color::Rgb(40, 40, 55),
    ..Default::default()
}
```

### Stage 5 — Layout: split the header

`app.rs` `draw()` — replace single header draw with:
```rust
let header_cols = Layout::horizontal([
    Constraint::Percentage(50),
    Constraint::Percentage(50),
]).split(header_area);
self.header.draw(frame, header_cols[0], false, &self.state);
let scope_focused = self.wm.focused() == Some(ComponentId::ScopePane);
self.scope_pane.draw(frame, header_cols[1], scope_focused, &self.state);
```

### Stage 6 — Focus ring and key binding

`action.rs`: add `ComponentId::ScopePane`

`workspace.rs` `rebuild_focus_ring_with()`:
- Append `ScopePane` as 5th item to all four focus ring lists (Radio/Tickers,
  Radio/Nts1, Radio/Nts2, Files)

`app.rs` keyboard handler:
- Add `KeyCode::Char('5') => Action::FocusPane(ComponentId::ScopePane)`
- NOT added to status bar hints (intentionally hidden)

Focused component dispatch: add `ScopePane => self.scope_pane.handle_key(key, s)`

---

## Files changed (full list)

| File | Change |
|------|--------|
| `~/gh/scope-tui/Cargo.toml` | ratatui 0.30, crossterm 0.28 |
| `~/gh/scope-tui/src/display/{mod,oscilloscope,spectroscope}.rs`, `src/app.rs` | `use ratatui::crossterm::event` |
| `crates/radio-tui/Cargo.toml` | add `scope` path dep |
| `crates/radio-tui/src/action.rs` | `ComponentId::ScopePane` |
| `crates/radio-tui/src/app_state.rs` | `scope_buffer`, `scope_active` |
| `crates/radio-tui/src/app.rs` | `ScopeFrame` message, scope task lifecycle, `scope_pane` + `scope_task` fields, draw() header split, key `5`, focused dispatch |
| `crates/radio-tui/src/components/scope_pane.rs` | **new** — full ScopePane impl |
| `crates/radio-tui/src/components/mod.rs` | `pub mod scope_pane` |
| `crates/radio-tui/src/workspace.rs` | ScopePane in all focus rings |

Estimated new code: ~350 lines. Modified code: ~50 lines.
