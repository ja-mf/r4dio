# r4dio: Architecture and Technical Design

## Overview

r4dio is a high-performance terminal radio client combining real-time audio visualization, remote streaming, local file playback, and song identification in a single-process Rust application. The architecture prioritizes synchronization between audio output and visualization feedback, resilience to network degradation, and minimal latency through careful buffer tuning and single-upstream connection per stream.

## System Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    r4dio TUI Application (Rust)                  │
│                  (crates/radio-tui, single process)              │
└──────┬──────────────────────┬──────────────────────┬──────────────┘
       │                      │                      │
       │ Event Loop           │ Playback Engine      │ HTTP Services
       │ (app.rs)             │ (core.rs)            │ (http.rs, proxy.rs)
       │                      │                      │
       ├────────────────────────────────────────────────────────────
       │
       ├── [Station Playback]
       │   ├─ HTTP Proxy (port 8990)
       │   │  └─ Single upstream TCP conn to radio server
       │   │     ├─ Broadcast::channel<Bytes> (cap: 4096)
       │   │     │  └─ mpv (audio output)
       │   │     │
       │   │     └─ ffmpeg PCM tap (direct to real URL, not synced in v1.0)
       │   │        └─ Stdout pipe (1024 samples/chunk @ 22050Hz)
       │   │           └─ PcmChunk broadcast → pcm_ring → VU + scope
       │   │
       │   └─ mpv Control (JSON-RPC over Unix socket)
       │      └─ Property observers (ICY title, duration, time position)
       │
       ├── [File Playback]
       │   ├─ Direct path on filesystem
       │   ├─ mpv handles demuxing, audio decoding
       │   └─ lavfi astats observer (no PCM stream, scope unavailable)
       │
       └── [Visualization & Metadata]
           ├─ VU Meter (RMS dBFS, EMA smoothing)
           ├─ Oscilloscope (ring buffer from PCM, 2s history)
           ├─ Adaptive title lamp (animation based on frequency content)
           ├─ Song fingerprinting (vibra binary, offline identification)
           └─ NTS schedule fetch (API polling, 60s interval)

Remote Control API (port 8989):
├─ GET  /status        → DaemonState JSON
├─ POST /play/:idx     → Command::PlayStation(idx)
├─ POST /pause         → Command::Pause
└─ POST /volume/:pct   → Command::SetVolume(f32)
```

## Audio Pipeline: Synchronization Challenge

The fundamental design challenge is maintaining synchronization between the audible output and the visual feedback (VU meter, oscilloscope). In v1.0, this is partially compromised by a dual-connection design that will be unified in v1.1.

### Current State (v1.0): Dual-Connection Problem

```
Radio Server (internet)
    │
    ├─ TCP Conn A: reqwest client in proxy.rs
    │  │ Fetches station stream, broadcasts to mpv via HTTP
    │  │ Buffer: OS TCP recv (128KB), Proxy channel (4096 msgs)
    │  │ Consumers: mpv (primary audio path), ffmpeg (secondary, for VU)
    │  │
    │  └─ → mpv localhost:8990/stream/:idx
    │       ├─ Demux, decode, audio output (system speakers)
    │       └─ Property observers (ICY title, duration)
    │
    └─ TCP Conn B: ffmpeg process spawned from core.rs
       │ Opens independent connection to real station URL
       │ Buffers: -fflags nobuffer, -rtbufsize (default 3.5MB)
       │ Command: ffmpeg -i <url> -ac 1 -ar 22050 -f s16le pipe:1
       │
       └─ Stdout → 1024-sample chunks at 22050Hz
           └─ Parsed into f32 [-1.0, 1.0] range
               └─ PcmChunk broadcast
                   ├─ Consumed by app.rs VU handler (200ms cycles)
                   └─ Pushed to pcm_ring → Oscilloscope display (30fps)

Problem: Conn A and Conn B are independent. They start at different positions in the
live stream and buffer differently. The PCM samples visible in the scope do not always
correspond to what mpv is playing. On high-jitter networks or busy servers, the skew
can exceed 1-2 seconds.

Symptom: Scope waveform does not match audio you hear. VU meter spikes may appear
before/after corresponding audio transient.
```

### Planned Unification (v1.1): Single-Source PCM

```
Radio Server (internet)
    │
    └─ TCP Conn (single): reqwest in proxy.rs
       │ Fetches once, broadcasts to multiple consumers
       │ Channel: broadcast::channel<Bytes> (cap 4096 msgs)
       │
       ├─ → mpv localhost:8990/stream/:idx (primary path)
       │   └─ Demux, decode, audio output
       │
       └─ → ffmpeg stdin (same proxy URL, NOT real URL)
           └─ Receives same bytes as mpv (but decompressed separately)
               └─ Stdout PCM stream (same f32 samples)
                   └─ PcmChunk broadcast
                       └─ VU + scope now perfectly synced with audio

Benefit: Single upstream TCP connection (halves server load), exact sync between
visualization and audible output, deterministic buffer behavior.

For local files, run parallel ffmpeg to replace lavfi astats:
```

### Unified Pipeline (v1.1 target)

```
Stations:
  Upstream conn → Proxy broadcast → mpv + ffmpeg (same source) → PCM ring → VU + scope

Files:
  File on disk
    ├─ mpv (demux, decode, audio out)
    └─ ffmpeg (parallel PCM tap to file) → PcmChunk broadcast → pcm_ring → VU + scope
       (On seek: restart ffmpeg at new position)

Result: Identical pipeline for stations and files. Oscilloscope works for both.
```

## Buffer Inventory and Latency Budget

Every buffer in the chain introduces latency and potential for desynchronization. The
full inventory with current tuning reflects a balance between responsiveness, jitter
resilience, and CPU load.

### Buffer Chain (Stations)

Layer 1 - OS TCP: The kernel's TCP receive buffer (typically 128KB-256KB). Absorbs
microsecond-scale network jitter. Increasing this would reduce packet loss during
brief bursty traffic, but the proxy's broadcast channel (layer 3) is the practical
limit.

Layer 2 - reqwest demuxer: The reqwest HTTP client buffers incoming response body into
an internal buffer. This is typically 8-16KB per read, adjusted by the underlying
hyper library. Not user-tunable; trust the defaults.

Layer 3 - Proxy broadcast channel: broadcast::channel<Bytes> with capacity 4096 messages.
Each message is typically 8KB of compressed audio. At 320kbps MP3, 8KB ≈ 200ms of audio.
So 4096 messages ≈ 800s of headroom before Lagged error fires. In practice, the channel
is rarely full. If ffmpeg (PCM path) lags and the channel fills, it skips ahead.

Layer 4 - mpv demuxer cache: mpv's internal demuxer reads from localhost:8990, caches
~2MB by default. Adjustable via `--demuxer-max-bytes` config. Higher values absorb jitter
but increase latency to live edge (less responsive to station changes). Lower values make
the stream more interactive but vulnerable to momentary network stutters.

Layer 5 - ffmpeg network buffer: ffmpeg's `-fflags nobuffer` disables write-side buffering,
but the input reader still has its own buffer. Not fully tuneable. `-rtbufsize` controls
the max duration of cached packets; default 3.5MB. For low-latency, reduce to 512K.

Layer 6 - ffmpeg probe buffer: `-probesize 64k -analyzeduration 200000` (0.2 seconds).
The time spent analyzing the stream format before PCM decoding starts. Longer probes
reduce misidentification of weird codecs; shorter probes reduce startup latency. 200ms
is a reasonable compromise.

Layer 7 - PCM chunk size: VU_WINDOW_SAMPLES=1024 at 22050Hz = 46ms per chunk. Sent
as PcmChunk broadcast messages. Smaller chunks = more responsive VU updates but more
allocations and broadcast overhead. 1024 samples is a good middle ground.

Layer 8 - PCM ring buffer: PCM_RING_MAX=88200 samples at 22050Hz = 4 seconds of audio.
Provides 4s of oscilloscope history. Increasing this wastes memory; decreasing it limits
how far back you can see the waveform.

Layer 9 - Display tick: METER_FPS=25 (40ms per frame). The TUI redraws the VU meter and
scope 25 times per second. Increasing to 30fps looks smoother but burns more CPU. 25fps
is a sweet spot on most terminals.

**Total latency from station to audible output: ~1-2 seconds (mpv demuxer + ffmpeg probe +
OS buffering). Oscilloscope lag relative to audio: in v1.0, ~200ms-2s depending on
network jitter; in v1.1, <200ms (ffmpeg lag from upstream).**

## Robustness Mechanisms

### Network Resilience

The proxy.rs pump (reqwest fetching and broadcasting) runs in a tokio task. If the
upstream connection drops, the task logs a warn and breaks, causing the broadcast channel
to close. mpv receives EOF and stops. In v1.0, the user must press Enter to reconnect. In
v1.1, we should implement auto-reconnect with exponential backoff (1s, 2s, 4s, 8s, cap 30s)
and jitter buffer (buffer first 2-3s before forwarding, so brief network hiccups are
invisible).

The ffmpeg PCM tap opens a separate connection. If the upstream is flaky and the proxy
reconnects before ffmpeg, ffmpeg will fail and restart. The core.rs loop restarts ffmpeg
every 2 seconds on error. For a robust radio application, this should use the same
exponential backoff strategy as the proxy.

### Error Propagation

Three critical panic sites currently exist (v1.0):
- proxy.rs:44 — .expect() on reqwest client builder (TLS init failure)
- proxy.rs:149,178 — .unwrap() on Response::builder() (malformed headers)
- core.rs:867 — .expect("ffmpeg stdout") (spawn failure)

All should convert to proper error propagation and graceful fallback. The app should
log these errors and continue, not crash.

### Audio Level Tracking (VU Meter)

The RMS dBFS calculation converts raw PCM samples (i16 or f32) to a perceptual dB scale.
The app maintains four derived metrics: audio_level (instantaneous RMS), peak_level
(fast attack, 6dB/s decay), meter_mean_db (EMA with τ=4s), meter_spread_db (EMA with
τ=8s). The EMA smoothing filters out single-sample noise and creates the typical radio
station VU meter animation. Peak level shows transients (drums, vocals), while mean
level shows sustained power. Spread shows dynamic range.

### Message Queueing

The core.rs broadcasts state changes and PCM chunks via a broadcast::channel with
multiple subscribers: the app event loop, the HTTP server, and logging. For audio-
critical messages (PcmChunk), if a subscriber lags and the channel fills, it skips
ahead rather than blocking the sender. This is the right choice: it is better to drop a
20ms chunk and recover than to stall the audio pipeline. For non-critical messages
(StateUpdated), losing one is usually fine; the next one will catch up. The HTTP server
is rarely the bottleneck here because it is local (port 8989).

### File Metadata / Seeking

Local file playback uses mpv's native seeking. When the user presses , (seek -30s), the
app sends Command::Seek(-30) to core.rs, which calls mpv_driver.seek(). This is instant
on local files. For remote streams (if seek is ever enabled), there is no seeking; the
stream position is server-determined. The file browser metadata panel (file_meta.rs) uses
ffprobe to extract duration and chapter markers, cached in memory.

## Concurrency Model

The architecture uses tokio async throughout. The main event loop (app.rs) spawns tasks
for the core (playback engine), HTTP server, and the PCM broadcast receiver. All tasks
communicate via channels (mpsc for commands, broadcast for state/audio). No shared
mutable state exists; the StateManager (Arc<RwLock<DaemonState>>) is the only cross-task
shared memory, protected by a reader-writer lock. The HTTP server holds a read lock only,
so it never blocks the playback engine.

The FFmpeg PCM task runs in its own tokio task, reading stdout in a loop and broadcasting
chunks. The loop is non-blocking; each chunk read yields to tokio, allowing other tasks
to run. This prevents FFmpeg from starving the display refresh or the mpv driver.

## Configuration and Defaults

All parameters (buffer sizes, API ports, file paths) are configurable via config.toml.
The default config is embedded in the binary at compile time (crates/radio-proto/src/config.rs).
At runtime, the loader checks three locations in order: ~/.config/radio/config.toml (user),
./config.toml (portable beside-exe for bundled releases), then falls back to defaults. This
allows bundled releases to work out of the box while still respecting user customization.

Stations are loaded similarly: ~/.config/radio/stations.toml, ./stations.toml (beside-exe,
for bundled releases), or a fallback to a public m3u URL. This design keeps the main binary
small (no embedded station list) while shipping a sensible default that works offline.

## Distribution and Cross-Platform Considerations

r4dio is compiled and released for macOS (arm64, x86_64), Linux (x86_64), and Windows
(x86_64). Each release bundles mpv, ffmpeg, ffprobe, yt-dlp, and vibra to avoid system dependencies. `starred.toml` is also bundled and auto-seeded into the OS data dir on first run.

macOS releases use .app bundles with a launcher script that detects whether the app was
launched from Finder (double-click) or Terminal. If Finder, it opens Terminal.app and runs
r4dio there. If Terminal, it runs directly. The .app is unsigned, so users see a Gatekeeper
warning on first launch (right-click → Open); v1.1 will add code signing.

Linux releases are tarballs with a relative directory structure, assuming they are extracted
to a working directory. mpv is bundled as an AppImage.

Windows releases are .zip files reorganized into subdirectories:
```
r4dio-windows-x86_64/
  r4dio.exe, config.toml, README.txt
  external/   ← mpv.exe, ffmpeg.exe, ffprobe.exe, yt-dlp.exe, vibra.exe + mpv DLLs
  data/        ← stations.toml, starred.toml, songs.vds
```
Vibra is statically linked (`x64-windows-static` + `/MT`) so it requires no VCRUNTIME140.dll, MSVCP140.dll, libcurl.dll, or libfftw3-3.dll. The app finds executables in `external/` via `find_beside_exe()` which searches both the exe directory and `external/` subdir.

## Testing and Logging

r4dio logs to ~/.local/share/radio/tui.log (Linux/macOS) or %APPDATA%\radio\tui.log (Windows)
at INFO level by default. Setting RUST_LOG=debug enables verbose logging including mpv IPC
traffic, ffmpeg subprocess events, and proxy connection state. All async task panics are caught
and logged.

## Future Improvements

Version 1.1 will unify the PCM pipeline (ffmpeg-via-proxy for both paths), implement auto-
reconnect with jitter buffer, add code signing for macOS, and fix the three panic sites. Version
1.2 targets oscilloscope for files, per-band EQ visualization, and mouse support. The NTS
download feature (v1.0.1+) will be extended to support batch downloads, metadata enrichment
from the NTS API, and playlist generation for downloaded shows.
