# Project TODO

## Data Layer

**CSV Migration & Database**: Migrate `~/songs.csv` to a more robust format while maintaining compatibility with visidata for easy inspection. Options: SQLite with CSV virtual table, or enhanced CSV with proper schema. The format should support: timestamp, track, artist, genre, extra metadata, comment (station; show; url), and derived fields like URL for chapters. Implement as internal database with CSV export capability. Default behavior: write to this format, read from it for the songs ticker. Consider using visidata's native format if it provides better metadata handling.

## UI Navigation

**Jumpable Panels**: Implement tab-based navigation between panels. Current: station list is focus. Add: songs.csv panel as focusable entity. When focused on songs ticker: arrow keys navigate entries, Enter opens URL (if present in comment), 'c' retrieves chapters (if URL has chapters), 'y' copies entry to clipboard, scroll with j/k or arrows. Visual indicator shows which panel is active (border highlight or cursor). Tab cycles: station list → songs ticker → (future: file browser). Escape returns to station list.

## Visualization

**Scope-tui Integration**: Integrate scope-tui (audio visualizer) into three modes: (1) compact mode below header, between title-clock and volume indicator—volume becomes just percentage number to save space; (2) full-screen mode toggled with dedicated key; (3) tabeable interface within full-screen allowing color scheme changes with arrow keys (different waveform colors: cyan, magenta, green, etc.). The visualizer connects to MPV's audio output via pipewire/pulse monitor or fifo. Requires MPV configuration to expose audio stream for visualization.

## File Browser & Download Workflow

**Local File Playback**: Add mode to browse local audio files in left panel instead of radio stations. Primary use case: NTS mixtapes downloaded via `nts_get` script.

Workflow:
1. Identify song via fingerprinting → writes to songs database → appears in songs ticker everywhere as normal
2. User tabs to songs ticker, navigates to entry, presses 'd' (download)
3. System calls `nts_get URL` (URL extracted from comment field) to retrieve mixtape
4. `nts_get` saves to `~/nts-downloads/` with metadata (including tracklist when available)
5. File appears in file browser list (left panel refreshes automatically)
6. User navigates file list, presses Enter to play
7. MPV switches from radio stream to local file playback
8. Right panel shows file metadata: title, date, duration, tracklist (if present), file path

File browser features: sortable by date, name, duration; filter with same `/` mechanism as stations; visual distinction between downloaded files and radio stations (different icon or color).

## Playback Progress

**Progress Popup**: Borderless, non-occluding popup below header showing file playback progress. Displays: current time / total duration as `MM:SS / MM:SS`, progress bar (ascii or unicode blocks), playback state icons matching MPV state (▶, ⏸, ⏹, ⏳). Updates in real-time via MPV IPC property observation. Only visible when playing local files (not radio streams). Position: directly below header row, spanning width of left panel (or centered). No background box—just text and bar to avoid visual clutter.

## Implementation Notes

- Database: Consider sqlite-vfs for CSV compatibility or rusqlite with serde
- Scope-tui: May require tokio process spawning and stdin/stdout piping
- File browser: New App mode `browse_mode: Radio | Files`, affects list rendering and key handlers
- Download integration: Extend `songid.sh` or create download daemon that watches songs.csv for URLs and triggers `nts_get`
- Progress popup: Subscribe to MPV `time-pos` and `duration` property changes via IPC
