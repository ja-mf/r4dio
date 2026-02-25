#!/bin/bash
# cmd4 state script for Radio NTS accessory
# Called as: radio_cmd4.sh Get <name> <characteristic>
#        or: radio_cmd4.sh Set <name> <characteristic> <value>

API="http://127.0.0.1:8989/api"

# NTS station index map: identifier (1-based) -> station idx
# identifier 1 = NTS 1 (idx 5)
# identifier 2 = NTS 2 (idx 6)
# identifier 3 = Labyrinth (idx 7)
# identifier 4 = Expansions (idx 8)
# identifier 5 = Sheet music (idx 10)
# identifier 6 = Slow focus (idx 11)
# identifier 7 = Field recordings (idx 14)
# identifier 8 = Island time (idx 16)
# identifier 9 = Feelings (idx 17)
# identifier 10 = Lowkey (idx 18)
# identifier 11 = Poolside (idx 21)
# identifier 12 = Memory Lane (idx 22)
# identifier 13 = Sweat (idx 25)
# identifier 14 = House (idx 26)
# identifier 15 = The Pit (idx 27)
# identifier 16 = The Tube (idx 28)
identifier_to_idx() {
    case "$1" in
        1)  echo 5  ;;
        2)  echo 6  ;;
        3)  echo 7  ;;
        4)  echo 8  ;;
        5)  echo 10 ;;
        6)  echo 11 ;;
        7)  echo 14 ;;
        8)  echo 16 ;;
        9)  echo 17 ;;
        10) echo 18 ;;
        11) echo 21 ;;
        12) echo 22 ;;
        13) echo 25 ;;
        14) echo 26 ;;
        15) echo 27 ;;
        16) echo 28 ;;
        *)  echo 5  ;;
    esac
}

idx_to_identifier() {
    case "$1" in
        5)  echo 1  ;;
        6)  echo 2  ;;
        7)  echo 3  ;;
        8)  echo 4  ;;
        10) echo 5  ;;
        11) echo 6  ;;
        14) echo 7  ;;
        16) echo 8  ;;
        17) echo 9  ;;
        18) echo 10 ;;
        21) echo 11 ;;
        22) echo 12 ;;
        25) echo 13 ;;
        26) echo 14 ;;
        27) echo 15 ;;
        28) echo 16 ;;
        *)  echo 1  ;;
    esac
}

get_state() {
    curl -sf --max-time 3 "${API}/state" 2>/dev/null
}

ACTION="$1"
# $2 is the accessory name (ignored)
CHARACTERISTIC="$3"
VALUE="$4"

if [ "$ACTION" = "Get" ]; then
    case "$CHARACTERISTIC" in
        Active)
            STATE=$(get_state) || { echo "0"; exit 0; }
            IS_PLAYING=$(echo "$STATE" | python3 -c "import json,sys; d=json.load(sys.stdin); print(1 if d['is_playing'] else 0)" 2>/dev/null)
            echo "${IS_PLAYING:-0}"
            ;;
        ActiveIdentifier)
            STATE=$(get_state) || { echo "1"; exit 0; }
            IDX=$(echo "$STATE" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('current_station') or 5)" 2>/dev/null)
            echo $(idx_to_identifier "${IDX:-5}")
            ;;
        ConfiguredName|Name)
            echo "NTS Radio"
            ;;
        SleepDiscoveryMode)
            echo "1"
            ;;
        *)
            echo "0"
            ;;
    esac

elif [ "$ACTION" = "Set" ]; then
    case "$CHARACTERISTIC" in
        Active)
            if [ "$VALUE" = "1" ]; then
                # Resume: get current station and play it
                STATE=$(get_state) || true
                IDX=$(echo "$STATE" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('current_station') or 5)" 2>/dev/null)
                curl -sf --max-time 3 "${API}/play/${IDX:-5}" > /dev/null
            else
                curl -sf --max-time 3 "${API}/stop" > /dev/null
            fi
            ;;
        ActiveIdentifier)
            IDX=$(identifier_to_idx "$VALUE")
            curl -sf --max-time 3 "${API}/play/${IDX}" > /dev/null
            ;;
        RemoteKey)
            # Optional: map remote keys
            case "$VALUE" in
                "4") curl -sf --max-time 3 "${API}/random" > /dev/null ;;  # SELECT -> random
            esac
            ;;
        *)
            ;;
    esac
fi

exit 0
