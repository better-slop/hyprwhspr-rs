#!/bin/bash
# Waybar status wrapper for hyprwhspr-rs
# Uses inotifywait for instant updates, with periodic refresh as keepalive

JSON_FILE="${XDG_CACHE_HOME:-$HOME/.cache}/hyprwhspr/status.json"
WATCH_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/hyprwhspr"
DEFAULT='{"text":"󰍭","class":"inactive","tooltip":"Not running"}'

emit() {
    if [[ -f "$JSON_FILE" ]]; then
        cat "$JSON_FILE"
    else
        echo "$DEFAULT"
    fi
}

# Emit initial state
emit

# Watch for changes with timeout - re-emit every 30s as keepalive
while true; do
    # Wait for file change or timeout after 30s
    inotifywait -q -t 30 -e moved_to,close_write "$WATCH_DIR" 2>/dev/null | while read -r _ event file; do
        if [[ "$file" == "status.json" ]]; then
            emit
        fi
    done
    # Timeout hit or inotifywait exited - re-emit current state
    emit
done
