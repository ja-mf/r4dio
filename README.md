# r4dio

Terminal radio player with:
- live station playback via ICY/HLS streams
- VU meter + oscilloscope visualization
- passive background polling — station list shows what's playing on every station, updated every 120s
- song identification via `vibra` (Shazam-like fingerprinting)
- NTS Infinite Mixtape show metadata enrichment (browserless, via Firestore)
- NTS show download via `yt-dlp`
- local file playback with chapter navigation
- starred stations/files with persistence

## v1.1 status

`r4dio` ships as a single self-contained archive per platform:

| Platform | Archive | Bundled |
|---|---|---|
| Windows x86_64 | `r4dio-windows-x86_64.zip` | mpv, ffmpeg, ffprobe, yt-dlp, vibra, DLLs |
| Linux x86_64 | `r4dio-linux-x86_64.tar.gz` | mpv (AppImage), ffmpeg, ffprobe, yt-dlp, vibra |
| macOS Apple Silicon | `r4dio-macos-arm64.dmg` | mpv.app, ffmpeg, ffprobe, yt-dlp, vibra |
| macOS Intel | `r4dio-macos-x86_64.dmg` | mpv.app, ffmpeg, ffprobe, yt-dlp, vibra |

All bundles include `stations.toml` and `starred.toml` (pre-loaded with curated stations and starred favourites).

## Quick start

1. Download the archive for your platform from the [latest release](https://github.com/ja-mf/r4dio/releases/latest).
2. Unzip / extract / open DMG.
3. Run `r4dio` (`r4dio.exe` on Windows).

**Windows**: unzip, run `r4dio.exe`. External tools are in `external/`, data files in `data/`.
**macOS**: drag `r4dio` to Applications. First launch: right-click → Open to bypass Gatekeeper.
**Linux**: `tar xzf r4dio-linux-x86_64.tar.gz && cd r4dio-linux-x86_64 && ./r4dio`

No extra installation required.

## Passive polling (v1.1)

Every 120 seconds r4dio probes all stations in the background and annotates the station list with what's currently playing. No interaction required — open the app and the list fills in progressively.

- **NTS 1 & 2**: fetched from `https://www.nts.live/api/v2/live`
- **NTS Infinite Mixtapes**: fetched from Firestore `mixtape_titles` (browserless)
- **All other ICY stations**: lightweight HTTP probe reading the first ICY metadata block; 6 concurrent workers, ~13s for 71 stations

The currently-playing station always shows the live ICY title from mpv rather than the polled value, so the annotation stays in sync with what you hear.

Toggle with `p`. Status shown in the keys bar (`poll:on` / `poll:off`).

## Build from source

```bash
cargo build --release -p radio-tui --bin r4dio
./target/release/r4dio
```

Requires `mpv`, `ffmpeg`, and optionally `vibra` and `yt-dlp` on PATH for full functionality.

## Default controls

| Key | Action |
|---|---|
| `Enter` | play selected station/file |
| `Space` | pause/resume |
| `n` / `P` | next / previous |
| `p` | toggle passive polling on/off |
| `r` / `R` | random / random back |
| `i` | identify song (vibra fingerprint) |
| `d` | download NTS show (yt-dlp) |
| `*` | cycle star rating |
| `o` | toggle oscilloscope |
| `?` | help |
| `q` | quit |

## Runtime files

- `stations.toml` — station list (beside exe or in `data/` on Windows)
- `starred.toml` — star ratings (auto-seeded from bundle on first run)
- `songs.vds` — recognition history (tab-separated)
- `config.toml` — runtime config (HTTP port, volume, poll interval, station source)

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
