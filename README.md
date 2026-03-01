# r4dio

r4dio is a terminal radio and audio player built as a single Rust binary (`crates/radio-tui`).

## What it does

- Plays internet radio stations (ICY/HLS) and local files
- Shows live metadata (ICY title + NTS metadata)
- Passive polling (`p`) annotates station list with current titles across stations
- VU meter + oscilloscope for live station playback
- Song identification with `vibra` (`i`)
- NTS show download via `yt-dlp` (`d` in Songs pane)
- Star ratings, sort/filter, random history, chapter-aware file playback
- Optional HTTP remote control API on `:8989`

## Runtime model

r4dio is a single process with in-process subsystems:

- **Playback control**: mpv process + JSON IPC
- **Station proxy**: HTTP stream proxy on `:8990` (`/stream/:idx`)
- **Audio analysis**: ffmpeg PCM tap for station RMS + scope samples
- **Polling**: background metadata resolver for NTS and non-NTS stations
- **Remote API**: optional control/status endpoints on `:8989`

For stations, mpv and ffmpeg both consume the proxied stream path so visual feedback tracks current playback.

## Quick start

1. Download the latest release: <https://github.com/ja-mf/r4dio/releases/latest>
2. Extract/open the package
3. Run `r4dio` (`r4dio.exe` on Windows)

### Build from source

```bash
cargo build --release -p radio-tui --bin r4dio
./target/release/r4dio
```

## Core controls

| Key | Action |
|---|---|
| `Enter` | play selected station/file |
| `Space` | pause/resume |
| `n` / `P` | next / previous |
| `p` | toggle passive polling |
| `r` / `R` | random / random back |
| `i` | identify song |
| `d` | download NTS show (Songs pane) |
| `o` | toggle oscilloscope |
| `?` | help |
| `q` | quit |

## Runtime files

- `config.toml` — runtime configuration
- `stations.toml` — station definitions
- `starred.toml` — station/file ratings
- `songs.vds` — recognition history database

## Credits & Dependencies

r4dio stands on the shoulders of exceptional open-source projects. Special thanks to the individual contributors and small teams behind:

**Audio & Playback:**
- [mpv](https://github.com/mpv-player/mpv) — powerful command-line video/audio player with JSON-RPC IPC
- [ffmpeg](https://ffmpeg.org/) — multimedia framework for encoding, decoding, and stream processing

**TUI & Visualization:**
- [ratatui](https://github.com/ratatui-org/ratatui) — Rust library for building terminal user interfaces
- [scope-tui](https://github.com/alacritty/alacritty/tree/master/extra/man) — oscilloscope-style waveform rendering in the terminal (adapted for audio visualization)

**Song Recognition:**
- [vibra](https://github.com/izwb003/vibra) — Shazam-like audio fingerprinting and song identification (maintained by solo developer)

**Runtime:**
- [tokio](https://github.com/tokio-rs/tokio) — async runtime for Rust
- [reqwest](https://github.com/seanmonstar/reqwest) — HTTP client
- [serde](https://github.com/serde-rs/serde) — serialization framework

Each of these projects solves a critical piece of the r4dio puzzle. Without them, building a real-time, responsive, cross-platform radio client would require orders of magnitude more development effort. If you find r4dio useful, consider supporting these upstream projects as well.

## License

MIT
