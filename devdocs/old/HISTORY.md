# Development History

This file preserves the commit history from the original repository before cleanup.

## Original Commits (21 total)

```
0c21e8d first commit
5b66e09 working pretty well now
5ade61c some fixes
9e95aed save work in progress before refactor
182a105 fix status/ICY lag, Eldorado crash recovery, ICY persistence+timestamps, songs.csv ticker, now-playing in header
00f292a simplify header: show only now-playing status+name, drop RADIO logo and station count
fc9f12c feat: add Windows cross-platform support with TCP sockets
e193b65 feat: update MPV IPC and paths for cross-platform support
1e95201 fix: keep TUI data dir at ~/.local/share/radio-tui for backwards compatibility
f0ab3bd feat: implement Windows named pipe support for MPV IPC
d2d0144 fix: fix Windows build errors
e2c3f61 build: add Windows static linking and distribution package
1e4d17c build: add Windows build script
3a97290 feat: add portable mode for Windows distribution
495d817 fix: Windows daemon spawning and distribution
bfd270a fix: improve Windows daemon debugging and error reporting
120fd1a fix: use portable config folder for daemon data on Windows
b6c55ec feat: add Windows song database and recognition, disable NTS downloads
a5563e9 ci: add GitHub Actions workflow for Windows builds with vibra
990d9fd chore: add .gitignore to exclude build artifacts and browser data
b360868 chore: remove build artifacts and Chrome cache from git
```

## Repository Cleanup

- **Date:** 2025-02-25
- **Reason:** Removed accidentally committed Chrome browser cache files (~900MB)
- **Method:** git-filter-repo to remove `'/home/jam/.config` paths from history
- **Result:** Clean repository with full source code, 21 commits squashed to 1
