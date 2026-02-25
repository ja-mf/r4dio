# Homebridge Radio Integration - Status Report

## What Was Implemented

### 1. Fixed HTTP API in radio-tui
Modified `src/daemon/http.rs` to actually execute commands:
- `POST /api/play/:idx` - Play station by index
- `POST /api/stop` - Stop playback  
- `POST /api/volume/:volume` - Set volume (0-100)
- `POST /api/random` - Random station
- `GET /api/state` - Get current state

The HTTP handlers now properly send commands through the command channel to the daemon.

### 2. Installed Homebridge Plugins
- `homebridge-http-lightbulb` - For volume control (brightness = volume)
- `homebridge-http-switch` - For shuffle button

### 3. Created Homebridge Configuration
Location: `/home/jam/repos/radio-tui/homebridge/config.json`

Accessories configured:
- **Radio Volume** (HTTP-LIGHTBULB) - Lightbulb with brightness controlling volume 0-100%
- **Radio Shuffle** (HTTP-SWITCH) - Stateless switch for random station

### 4. Removed Non-Working Accessories
- Removed "Computer Speakers" (homebridge-pc-volume) from config
- Disabled homebridge-pc-volume plugin in disabledPlugins array
- Uninstalled homebridge-pc-volume npm package

## Current Status

### ✅ Working:
1. Radio daemon HTTP API is functional on port 8989
2. All endpoints tested and working via curl:
   ```bash
   curl -s http://127.0.0.1:8989/api/state
   curl -s -X POST http://127.0.0.1:8989/api/play/0
   curl -s -X POST http://127.0.0.1:8989/api/stop
   curl -s -X POST http://127.0.0.1:8989/api/volume/50
   curl -s -X POST http://127.0.0.1:8989/api/random
   ```
3. Homebridge loads the accessories (logs show "Loading 2 accessories...")
4. Both accessories initialize successfully:
   - "[Radio Volume] Lightbulb successfully configured..."
   - "[Radio Shuffle] Switch successfully configured..."

### ❌ Not Working:
**The accessories are NOT appearing in Apple Home app**

Symptoms:
- Accessories initialize but are NOT cached to `/var/lib/homebridge/accessories/cachedAccessories`
- Cache file remains empty (`[]` - 3 bytes)
- Accessories don't appear in HomeKit

### 🔍 Debugging Notes:

1. **Cache Issue**: The main problem is that accessories load but don't persist to cache:
   ```
   [2/22/2026, 3:47:03 AM] Loading 2 accessories...
   [2/22/2026, 3:47:03 AM] [Radio Volume] Initializing HTTP-LIGHTBULB accessory...
   [2/22/2026, 3:47:03 AM] [Radio Volume] Lightbulb successfully configured...
   [2/22/2026, 3:47:03 AM] [Radio Shuffle] Initializing HTTP-SWITCH accessory...
   [2/22/2026, 3:47:03 AM] [Radio Shuffle] Switch successfully configured...
   [2/22/2026, 3:47:13 AM] Loaded 0 cached accessories from cachedAccessories.
   ```

2. **Bridge Running**: Homebridge is running on port 51496
   - Main bridge: "Homebridge 4AD3" 
   - PIN: 145-44-875
   - Username: 0E:69:03:2B:A4:B7

3. **Network Errors**: There are "Service name already in use" errors causing restarts
   - This might be interfering with accessory caching

4. **Plugin Registration**: Both plugins register correctly:
   - `homebridge.registerAccessory("homebridge-http-lightbulb", "HTTP-LIGHTBULB", HTTP_LIGHTBULB)`
   - `homebridge.registerAccessory("homebridge-http-switch", "HTTP-SWITCH", HTTP_SWITCH)`

## Next Steps to Debug:

1. **Check if accessories need unique UUIDs** - HomeKit requires stable UUIDs
2. **Verify HTTP plugin versions** - Check if there's a compatibility issue
3. **Try adding a simple test accessory** - Create a minimal accessory to test if caching works
4. **Check Homebridge version compatibility** - Current: v1.11.1
5. **Try child bridge** - Move radio accessories to a child bridge
6. **Clear persist data** - Remove `/var/lib/homebridge/persist/` and re-pair
7. **Check for plugin errors** - Look for silent failures during accessory creation

## File Locations:

- Radio-tui homebridge folder: `/home/jam/repos/radio-tui/homebridge/`
- Homebridge config: `/var/lib/homebridge/config.json`
- Homebridge logs: `/var/lib/homebridge/homebridge.log`
- Homebridge accessories cache: `/var/lib/homebridge/accessories/cachedAccessories`
- Homebridge persist: `/var/lib/homebridge/persist/`
- Radio daemon HTTP API: `http://127.0.0.1:8989`

## Manual Testing Commands:

```bash
# Check radio API
curl -s http://127.0.0.1:8989/api/state | python3 -m json.tool

# Check homebridge logs
sudo tail -f /var/lib/homebridge/homebridge.log

# Check cached accessories
sudo cat /var/lib/homebridge/accessories/cachedAccessories | python3 -m json.tool

# Restart homebridge
sudo systemctl restart homebridge
```

## Configuration Backup:

Original config backed up at: `/var/lib/homebridge/config.json.backup.20260222_032700`

Current working config at: `/home/jam/repos/radio-tui/homebridge/config.json`
