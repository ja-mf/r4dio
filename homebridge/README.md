# Radio-TUI Homebridge Integration

This folder contains the Homebridge integration for controlling radio-tui via HomeKit.

## What Was Implemented

### 1. Fixed HTTP API in radio-tui
The HTTP API endpoints in `src/daemon/http.rs` were updated to actually execute commands:
- `POST /api/play/:idx` - Play station by index
- `POST /api/stop` - Stop playback
- `POST /api/next` - Next station
- `POST /api/prev` - Previous station
- `POST /api/random` - Random station
- `POST /api/volume/:volume` - Set volume (0-100)
- `GET /api/state` - Get current state

### 2. Homebridge Accessories

Two accessories were added to Homebridge:

#### Radio Volume (HTTP-LIGHTBULB)
- **Type**: Lightbulb
- **On**: Plays station 0 (or resumes last station)
- **Off**: Stops playback
- **Brightness**: Controls volume (0-100%)
- **Status**: Shows if radio is playing

#### Radio Shuffle (HTTP-SWITCH)
- **Type**: Stateless switch (button)
- **Action**: Triggers random station selection
- **Use**: Tap to shuffle to a random station

### 3. Files in This Folder

- `config.json` - Homebridge configuration with radio accessories
- `radio-control.sh` - Helper script for manual control/testing

## How to Use

### From HomeKit (iPhone/Home App)
1. **Radio Volume** tile:
   - Tap to play/stop
   - Long press for brightness slider (volume)
   - Shows current volume as brightness percentage

2. **Radio Shuffle** tile:
   - Tap to jump to a random station

### Manual Control (for testing)
```bash
# Using the helper script
./radio-control.sh playing    # Check if playing
./radio-control.sh volume     # Get current volume
./radio-control.sh station    # Get current station name
./radio-control.sh play       # Start playing
./radio-control.sh stop       # Stop playing
./radio-control.sh setvolume 75  # Set volume to 75%
./radio-control.sh shuffle    # Random station

# Using curl directly
curl -s http://127.0.0.1:8989/api/state
curl -s -X POST http://127.0.0.1:8989/api/play/0
curl -s -X POST http://127.0.0.1:8989/api/stop
curl -s -X POST http://127.0.0.1:8989/api/volume/50
curl -s -X POST http://127.0.0.1:8989/api/random
```

## Technical Details

### Radio Daemon HTTP API
- **Port**: 8989
- **Bind**: 127.0.0.1 (localhost only)
- **State Format**: JSON with stations, current_station, volume, is_playing, icy_title

### Homebridge Plugins Used
- `homebridge-http-lightbulb` - For volume control
- `homebridge-http-switch` - For shuffle button

### Architecture
```
HomeKit App → Homebridge → HTTP Plugin → radio-daemon HTTP API → MPV
```

## Troubleshooting

### Radio not responding in HomeKit
1. Check if radio-daemon is running: `ps aux | grep radio-daemon`
2. Check HTTP API: `curl http://127.0.0.1:8989/api/state`
3. Restart radio-daemon if needed
4. Check Homebridge logs: `sudo journalctl -u homebridge -f`

### Volume not working
- The volume is mapped to lightbulb brightness (0-100%)
- If brightness shows 0%, the radio might be stopped
- Try turning the lightbulb on first, then adjusting brightness

### Station name display
- Current station name is NOT shown in HomeKit (HomeKit limitation)
- The state is available via API for custom integrations

## Future Improvements

Possible enhancements:
1. Add a "Radio Station" occupancy sensor that shows current station name
2. Add Next/Prev station switches
3. Create a custom Homebridge plugin for better UX
4. Add Siri shortcuts support
