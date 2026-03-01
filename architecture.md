# r4dio Architecture

## Overview

r4dio runs as a **single binary** (`crates/radio-tui`) with async tasks for UI, playback, networking, and metadata. It is not a daemon + client split in active use.

```
r4dio (radio-tui)
  ├─ controls mpv (audio output + IPC state)
  ├─ runs ffmpeg PCM tap (VU + oscilloscope data, stations)
  ├─ serves stream proxy :8990 (/stream/:idx)
  ├─ serves optional control API :8989
  ├─ runs passive polling scheduler
  └─ runs song recognition/download tasks on demand
```

## Main subsystems

### 1) App loop and UI

- `src/main.rs` starts runtime and app
- `src/app.rs` owns event loop, input handling, draw scheduling
- `src/app_state.rs` stores UI + runtime state

### 2) Playback engine

- `src/core.rs` manages playback lifecycle and command handling
- `src/mpv.rs` spawns mpv and handles JSON IPC observers
- Playback source can be station stream or local file

### 3) Stream proxy (`:8990`)

- `src/proxy.rs`
- Endpoint: `GET /stream/:idx`
- Rewrites station access through local proxy for a stable in-process stream path
- For station playback, mpv and ffmpeg are fed from this proxied stream path

### 4) Audio metering/scope path

- Stations: ffmpeg decodes PCM samples to `PcmChunk` updates for RMS + scope ring buffer
- Files: mpv lavfi observer supplies scalar audio level (no PCM scope path)

### 5) Passive polling

- Background cycle updates station list metadata
- NTS 1/2 path uses `https://www.nts.live/api/v2/live`
- NTS mixtape path resolves show info via Firestore-backed metadata access
- Non-NTS path uses concurrent ICY probes with bounded workers

### 6) Remote control API (`:8989`)

- `src/http.rs`
- Exposes status + playback control endpoints
- Sends commands into the same core command channel used by the TUI

## Data and state flow

- `DaemonState` (from `radio-proto`) is the shared playback status model
- Core publishes updates via broadcast channels
- App loop consumes updates and redraws UI
- HTTP handlers read state and dispatch commands through mpsc channels

## Active/legacy boundaries

- **Primary active target:** `crates/radio-tui`
- `crates/radio-daemon` is legacy reference code
- repository `src/` prototype is not part of current workspace build
