#!/bin/bash
# Build script for radio-tui Windows distribution

set -e

echo "=== Building Radio TUI for Windows ==="
echo ""

# Clean previous Windows build
echo "Cleaning previous build..."
rm -rf target/x86_64-pc-windows-msvc/release/radio-*.exe

# Build for Windows with static CRT
echo "Building Windows binaries (statically linked)..."
cargo xwin build --target x86_64-pc-windows-msvc --release

# Create distribution directory
echo "Creating distribution package..."
rm -rf dist/radio-tui-windows/*
mkdir -p dist/radio-tui-windows

# Copy binaries
cp target/x86_64-pc-windows-msvc/release/radio-tui.exe dist/radio-tui-windows/radio.exe
cp target/x86_64-pc-windows-msvc/release/radio-daemon.exe dist/radio-tui-windows/
cp win64/mpv/mpv.exe dist/radio-tui-windows/
cp win64/mpv/*.dll dist/radio-tui-windows/ 2>/dev/null || true

# Copy stations.toml for portable mode
cp stations.toml dist/radio-tui-windows/

# Create empty songs database for portable mode
echo "timestamp|title|artist|album|genre|year|radio_station|show_name|url|icy_raw|fingerprint|recognized|extra" > dist/radio-tui-windows/songs.vds

# Create default config.toml for Windows (portable)
cat > dist/radio-tui-windows/config.toml << 'EOF'
# Radio TUI Configuration (Windows Portable)
# Place this file in the same folder as radio.exe

[daemon]
# PID and state files will be created in %LOCALAPPDATA%\radio\

[http]
enabled = true
bind_address = "127.0.0.1"
port = 8989

[mpv]
default_volume = 0.5

[stations]
# Load stations from local stations.toml (portable mode)
stations_toml = "stations.toml"
m3u_url = "https://raw.githubusercontent.com/ja-mf/radio-curation/refs/heads/main/jamf_radios.m3u"
EOF

# Create config directory marker (empty folder to indicate portable mode)
mkdir -p dist/radio-tui-windows/config

# Copy documentation
cat > dist/radio-tui-windows/README.txt << 'EOF'
Radio TUI for Windows
=====================

A terminal-based internet radio player.

QUICK START
-----------
1. Double-click radio.exe or run: .\radio.exe
2. Navigate with arrow keys
3. Enter to play, Space to pause
4. Press 'q' to quit

FILES
-----
- radio.exe         - Main TUI application
- radio-daemon.exe  - Background service (auto-started by radio.exe)
- mpv.exe           - Media player backend
- stations.toml     - Station list (edit to add your own stations)
- config.toml       - Settings file
- songs.vds         - Song database (saves recognized tracks)
- config/           - Data directory (portable mode)

PORTABLE MODE
-------------
This is a portable build. All configuration is read from this folder:
- config.toml in this directory
- stations.toml in this directory
- Data saved to config/ subdirectory

No registry entries. No files outside this folder.

REQUIREMENTS
------------
- Windows 10 or Windows 11
- No Visual C++ Redistributable needed
- No installation required

TROUBLESHOOTING
---------------
If stuck on "connecting to daemon...":
1. Ensure radio-daemon.exe is in the same folder as radio.exe
2. Check Windows Defender isn't blocking the executable
3. Try running from PowerShell to see error messages: .\radio.exe

If stations don't load, check stations.toml is in this folder.
If audio doesn't play, ensure mpv.exe is present.

For more info: https://github.com/ja-mf/radio-tui
EOF

# Create launcher batch file
cat > dist/radio-tui-windows/radio.bat << 'EOF'
@echo off
cd /d "%~dp0"
echo Starting Radio TUI...
echo Config: %~dp0config.toml
echo Stations: %~dp0stations.toml
echo.
"%~dp0radio.exe"
EOF

# Create ZIP archive
cd dist
rm -f radio-tui-windows.zip
zip -r radio-tui-windows.zip radio-tui-windows/
cd ..

echo ""
echo "=== Build Complete ==="
echo ""
echo "Distribution: dist/radio-tui-windows.zip"
echo ""
ls -lh dist/radio-tui-windows.zip
echo ""
echo "Contents:"
ls -lh dist/radio-tui-windows/
