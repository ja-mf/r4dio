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

## License

MIT
