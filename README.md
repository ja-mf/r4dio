# r4dio

Terminal radio player with:
- live station playback
- VU + scope visualization
- song recognition via `vibra`
- local file playback

## v1.0 status

`r4dio` is released as a single executable (`r4dio`) with bundled runtime binaries in release archives:
- `mpv`
- `ffmpeg` + `ffprobe`
- `vibra`
- `stations.toml`
- empty `songs.vds`

## Quick start (release archives)

1. Download the archive for your platform.
2. Unzip / extract.
3. Run `r4dio` (`r4dio.exe` on Windows).

No extra installation required for playback or recognition in bundled builds.

## Build from source

```bash
cargo build --release -p radio-tui --bin r4dio
./target/release/r4dio
```

## Default controls

- `Enter`: play selected station/file
- `Space`: pause/resume
- `n` / `p`: next / previous
- `r`: random
- `i`: identify song
- `q`: quit
- `?`: help

## Runtime files

- `stations.toml`: station list used at startup
- `songs.vds`: recognition DB (starts empty in release bundles)
- `config.toml`: runtime config (HTTP + station source + defaults)

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
