# radio-tui

Terminal UI for streaming radio with metadata-rich display, audio fingerprinting integration, and local file playback support.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │ ▶ Station Name  playing  22/02 14:32 (radio time LONDON 13:32)      │ ● 100%│
│  └─────────────────────────────────────────────────────────────────────┘   │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌─────────────────────────────────────┐  ┌─────────────────────────────┐  │
│  │                                     │  │  NTS Channel 1              │  │
│  │  ▶  NTS 1 · London, UK              │  │  ─────────────────────────  │  │
│  │     SomaFM: Groove Salad            │  │  ● Show Title               │  │
│  │     NTS: Slow Focus                 │  │    12:00 – 14:00            │  │
│  │     Worldwide FM                    │  │    14:00 – 16:00 · London   │  │
│  │                                     │  │    Electronic · Ambient     │  │
│  │                                     │  │    Mellow · Chill           │  │
│  │  ─────────────────────────────────  │  │                             │  │
│  │                                     │  │  Show description text...   │  │
│  │  /filter query    sort:network      │  │                             │  │
│  │  ─────────────────────────────────  │  │  upcoming                   │  │
│  │                                     │  │  14:00 – 16:00  Next Show   │  │
│  │                                     │  │  16:00 – 18:00  Another One │  │
│  │                                     │  │                             │  │
│  │  45/127 stations                    │  │  ─────────────────────────  │  │
│  │                                     │  │  ♪  14:32  Artist - Title   │  │
│  │                                     │  │  ♫  14:28  Track - Artist   │  │
│  │                                     │  │      NTS 1 · Show Title     │  │
│  │                                     │  │  ♫  14:25  Previous Song    │  │
│  │                                     │  │      NTS 2 · Other Show     │  │
│  │                                     │  │                             │  │
│  └─────────────────────────────────────┘  └─────────────────────────────┘  │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│  ↑↓ nav  / filter  s sort  enter play  spc stop  ←→ vol  l logs  1/2 NTS  ? help  q quit  │
└─────────────────────────────────────────────────────────────────────────────┘
```

The application follows a client-server architecture with three primary runtime components:

**radio-daemon** (`src/daemon/main.rs`) runs as a background service managing MPV process lifecycle, HTTP API (localhost:8989), and Unix domain socket for TUI communication. It handles station URL resolution from `stations.toml`, stream playback via MPV IPC, ICY metadata extraction, and state persistence. The daemon exposes REST endpoints for `/api/state` (full state) and WebSocket-style broadcasts over the Unix socket for real-time updates.

**radio-tui** (`src/tui/main.rs`) is the terminal interface using ratatui with crossterm backend. It connects to the daemon via Unix socket, maintains local App state with filtered station lists, and renders three main areas: header (status + clock), body (station list + info panel split), and footer (keybindings). The TUI implements mouse support, keyboard navigation (vim-style hjkl or arrows), filter-as-you-type with `/`, and four sort modes (default, network, location, name). Special NTS integration polls `nts.live/api/v2/live` every 10 seconds for Channels 1/2 show data, updating dynamic location fields and rendering detailed show panels with broadcast title, time range, location, genres, moods (wrapped), description, and upcoming schedule.

**songid.sh** is the external audio fingerprinting orchestrator that queries the daemon's HTTP API for current station, handles three station types differently: SomaFM parses ICY title directly; NTS 1/2 queries live API for show metadata; NTS mixtapes spawn Selenium to scrape current show (mixtapes lack API endpoints). Results append to `~/songs.csv` with format `time,track,artist,genre,extra,comment` where comment encodes `station; show_title; episode_alias`. The TUI reads this CSV every 10 seconds, deduplicates consecutive entries, and renders in the right panel ticker with station/show annotations.

The three components communicate via: HTTP (daemon state API), Unix socket (daemon↔TUI bidirectional messages using length-prefixed Message protocol), and filesystem (songs.csv, tui.log, icyticker.log). State synchronization is push-based from daemon to TUI via Broadcast messages (State, Icy, Log, Error), with TUI sending Commands (Play, Stop, Volume, Next, Prev, Random) over the same socket.

## Station List

Stations are defined in `stations.toml` with schema: name, network, url, city, country, tags (array), description. The TUI maintains `filtered_indices` vector recomputed on filter/sort changes, preserving original order for "default" sort. Location display uses "City, Country" format when both present. Networks appearing multiple times show network badges (e.g., "SomaFM · ") before station name.

## NTS Integration

NTS Channels 1 and 2 have special handling: their `city`/`country` fields are dynamically overridden from live API `location_long`/`location_short` on every poll, so the station list reflects current broadcast origin. When playing NTS 1 or 2, the right panel auto-switches to show that channel's data. Manual toggle via keys `1` and `2`. The NTS panel layout (when space permits) dedicates ~70% to show info with a compact 8-row ticker strip below containing ICY and songs data.

## Clock & Timezones

Header displays local time as `dd/mm HH:MM` with radio location time in parentheses when available: `(radio time CITY HH:MM)`. Timezone calculation uses hardcoded IANA mappings for 70+ cities with DST-aware offsets (Northern: Mar-Oct, Southern: inverted). Falls back to `(CITY)` if timezone unknown. NTS channels extract city from API `location_long` field; other stations use `stations.toml` city field.

## Keybindings

| Key | Action |
|-----|--------|
| `↑/↓` or `j/k` | Navigate station list |
| `Enter` | Play selected station |
| `Space` | Stop playback |
| `n/p` | Next/previous station |
| `r` | Random station |
| `←/→` or `-/+` | Volume down/up |
| `/` | Filter stations (type to filter, Esc to clear) |
| `s` | Cycle sort: default → network → location → name |
| `1/2` | Toggle NTS Channel 1/2 info panel |
| `l` | Toggle log panel |
| `h` | Toggle keybindings footer |
| `?` | Help overlay |
| `q` | Quit |
| Mouse scroll | Navigate list |

## Files

- `~/.config/radio/stations.toml` — Station definitions
- `~/.local/share/radio/tui.log` — TUI debug logs
- `~/.local/share/radio/icyticker.log` — ICY metadata history
- `~/songs.csv` — Audio fingerprinting results

## Building

```bash
cargo build --release
# Binaries: target/release/radio-daemon, target/release/radio-tui
```

Requires: MPV, vibra (for songid.sh), Python+Selenium (for NTS mixtape scraping).
