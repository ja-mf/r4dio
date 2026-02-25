#!/bin/bash
# Radio Control Script for Homebridge
# This script provides a unified interface for Homebridge to control radio-tui

RADIO_API="http://127.0.0.1:8989"

# Function to get current state
get_state() {
    curl -s "${RADIO_API}/api/state" 2>/dev/null
}

# Function to check if radio is playing
check_playing() {
    local state=$(get_state)
    if [ -z "$state" ]; then
        echo "false"
        return
    fi
    echo "$state" | grep -o '"is_playing":true' > /dev/null && echo "true" || echo "false"
}

# Function to get current volume (0-100)
get_volume() {
    local state=$(get_state)
    if [ -z "$state" ]; then
        echo "0"
        return
    fi
    # Extract volume and convert from 0.0-1.0 to 0-100
    local vol=$(echo "$state" | grep -o '"volume":[0-9.]*' | cut -d: -f2)
    if [ -n "$vol" ]; then
        # Convert to integer percentage
        echo "$vol * 100" | bc | cut -d. -f1
    else
        echo "0"
    fi
}

# Function to get current station name
get_station_name() {
    local state=$(get_state)
    if [ -z "$state" ]; then
        echo "Radio Off"
        return
    fi
    
    local is_playing=$(echo "$state" | grep -o '"is_playing":true')
    if [ -z "$is_playing" ]; then
        echo "Radio Off"
        return
    fi
    
    local current_idx=$(echo "$state" | grep -o '"current_station":[0-9]*' | cut -d: -f2)
    if [ -n "$current_idx" ]; then
        # Extract station name from stations array
        local station=$(echo "$state" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['stations'][$current_idx]['name'])" 2>/dev/null)
        if [ -n "$station" ]; then
            echo "$station"
        else
            echo "Playing"
        fi
    else
        echo "Radio"
    fi
}

# Main command handler
case "$1" in
    "playing")
        check_playing
        ;;
    "volume")
        get_volume
        ;;
    "station")
        get_station_name
        ;;
    "play")
        # Find first station and play it, or play current
        local state=$(get_state)
        local current_idx=$(echo "$state" | grep -o '"current_station":[0-9]*' | cut -d: -f2)
        if [ -n "$current_idx" ]; then
            curl -s -X POST "${RADIO_API}/api/play/${current_idx}" > /dev/null
        else
            # Play station 0
            curl -s -X POST "${RADIO_API}/api/play/0" > /dev/null
        fi
        ;;
    "stop")
        curl -s -X POST "${RADIO_API}/api/stop" > /dev/null
        ;;
    "setvolume")
        if [ -n "$2" ]; then
            curl -s -X POST "${RADIO_API}/api/volume/$2" > /dev/null
        fi
        ;;
    "shuffle")
        curl -s -X POST "${RADIO_API}/api/random" > /dev/null
        ;;
    *)
        echo "Usage: $0 {playing|volume|station|play|stop|setvolume <0-100>|shuffle}"
        exit 1
        ;;
esac
