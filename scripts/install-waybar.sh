#!/usr/bin/env bash
# Install hyprwhspr-rs Waybar integration
#
# This script:
# 1. Backs up ALL existing config files before modification
# 2. Creates XDG directories for status/history
# 3. Sets up environment file for API keys
# 4. Installs and starts systemd user service
# 5. Adds Waybar module to config (first position in modules-right)
# 6. Adds CSS styles
# 7. Reloads Waybar
# 8. Optionally installs Elephant menu for Walker integration
#
# Usage: ./install-waybar.sh [--with-elephant]

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info() { echo -e "${BLUE}[INFO]${NC} $*"; }
success() { echo -e "${GREEN}[OK]${NC} $*"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*" >&2; }

# Determine script location - handle both direct execution and via hyprwhspr-rs install
if [[ -n "${HYPRWHSPR_INSTALL_DIR:-}" ]]; then
    REPO_DIR="$HYPRWHSPR_INSTALL_DIR"
elif [[ -f "${BASH_SOURCE[0]}" ]]; then
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    REPO_DIR="$(dirname "$SCRIPT_DIR")"
else
    # Fallback: assume we're in the repo
    REPO_DIR="$(pwd)"
fi

XDG_CACHE_HOME="${XDG_CACHE_HOME:-$HOME/.cache}"
XDG_DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"

HYPRWHSPR_CONFIG_DIR="$XDG_CONFIG_HOME/hyprwhspr-rs"
WAYBAR_CONFIG_DIR="$XDG_CONFIG_HOME/waybar"
SYSTEMD_USER_DIR="$XDG_CONFIG_HOME/systemd/user"
ELEPHANT_MENU_DIR="$XDG_CONFIG_HOME/elephant/menus"

BACKUP_DATE="$(date +%Y%m%d_%H%M%S)"
BACKUPS_MADE=()

WITH_ELEPHANT=false
for arg in "$@"; do
    case "$arg" in
        --with-elephant) WITH_ELEPHANT=true ;;
    esac
done

backup_file() {
    local file="$1"
    if [[ -f "$file" ]]; then
        local backup="${file}.backup-${BACKUP_DATE}"
        cp "$file" "$backup"
        BACKUPS_MADE+=("$backup")
        info "Backup: $backup"
        return 0
    fi
    return 1
}

backup_all_existing_configs() {
    info "Backing up existing configuration files..."
    
    local backed_up=0
    
    # Waybar configs
    for f in "$WAYBAR_CONFIG_DIR/config.jsonc" "$WAYBAR_CONFIG_DIR/config.json" "$WAYBAR_CONFIG_DIR/config" "$WAYBAR_CONFIG_DIR/style.css"; do
        if backup_file "$f"; then
            ((backed_up++)) || true
        fi
    done
    
    # Systemd service
    if backup_file "$SYSTEMD_USER_DIR/hyprwhspr-rs.service"; then
        ((backed_up++)) || true
    fi
    
    # Existing hyprwhspr env file
    if backup_file "$HYPRWHSPR_CONFIG_DIR/env"; then
        ((backed_up++)) || true
    fi
    
    # Elephant menu if exists
    if backup_file "$ELEPHANT_MENU_DIR/hyprwhspr.lua"; then
        ((backed_up++)) || true
    fi
    
    if [[ $backed_up -gt 0 ]]; then
        success "Backed up $backed_up file(s)"
    else
        info "No existing files to back up"
    fi
}

create_directories() {
    info "Creating directories..."
    
    mkdir -p "$XDG_CACHE_HOME/hyprwhspr-rs"
    mkdir -p "$XDG_DATA_HOME/hyprwhspr-rs"
    mkdir -p "$HYPRWHSPR_CONFIG_DIR"
    mkdir -p "$SYSTEMD_USER_DIR"
    mkdir -p "$WAYBAR_CONFIG_DIR"
    
    success "Directories created"
}

setup_env_file() {
    info "Setting up environment file..."
    
    local env_file="$HYPRWHSPR_CONFIG_DIR/env"
    
    # If env file exists and has content, don't overwrite
    if [[ -f "$env_file" && -s "$env_file" ]]; then
        success "Environment file already exists: $env_file"
        return 0
    fi
    
    # Check if GROQ_API_KEY is in current environment
    if [[ -n "${GROQ_API_KEY:-}" ]]; then
        echo "GROQ_API_KEY=$GROQ_API_KEY" > "$env_file"
        chmod 600 "$env_file"
        success "Created env file with GROQ_API_KEY from environment"
        return 0
    fi
    
    # Check common shell rc files for the key
    local found_key=""
    for rc_file in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.profile" "$HOME/.zshenv"; do
        if [[ -f "$rc_file" ]]; then
            local key_line=$(grep -E "^export\s+GROQ_API_KEY=" "$rc_file" 2>/dev/null | tail -1 || true)
            if [[ -n "$key_line" ]]; then
                found_key=$(echo "$key_line" | sed -E 's/^export\s+GROQ_API_KEY=["'"'"']?([^"'"'"']*)["'"'"']?.*$/\1/')
                if [[ -n "$found_key" ]]; then
                    info "Found GROQ_API_KEY in $rc_file"
                    break
                fi
            fi
        fi
    done
    
    if [[ -n "$found_key" ]]; then
        echo "GROQ_API_KEY=$found_key" > "$env_file"
        chmod 600 "$env_file"
        success "Created env file with GROQ_API_KEY from shell config"
        return 0
    fi
    
    # Prompt user for the key
    warn "GROQ_API_KEY not found in environment or shell config"
    echo ""
    read -rp "Enter your GROQ_API_KEY (or press Enter to skip): " user_key
    
    if [[ -n "$user_key" ]]; then
        echo "GROQ_API_KEY=$user_key" > "$env_file"
        chmod 600 "$env_file"
        success "Created env file with provided key"
    else
        touch "$env_file"
        chmod 600 "$env_file"
        warn "Created empty env file - add your API key later to: $env_file"
    fi
}

install_systemd_service() {
    info "Installing systemd user service..."
    
    local src="$REPO_DIR/config/systemd/hyprwhspr-rs.service"
    local dst="$SYSTEMD_USER_DIR/hyprwhspr-rs.service"
    
    if [[ ! -f "$src" ]]; then
        error "Service file not found: $src"
        return 1
    fi
    
    cp "$src" "$dst"
    
    systemctl --user daemon-reload
    systemctl --user enable hyprwhspr-rs.service
    
    success "Systemd service installed and enabled"
}

start_service() {
    info "Starting hyprwhspr-rs service..."
    
    if systemctl --user is-active --quiet hyprwhspr-rs.service; then
        systemctl --user restart hyprwhspr-rs.service
        success "Service restarted"
    else
        systemctl --user start hyprwhspr-rs.service
        success "Service started"
    fi
    
    sleep 1
    
    if systemctl --user is-active --quiet hyprwhspr-rs.service; then
        success "Service is running"
    else
        warn "Service may have failed to start. Check: systemctl --user status hyprwhspr-rs"
    fi
}

install_waybar_module() {
    info "Configuring Waybar module..."
    
    local config_file=""
    for f in "$WAYBAR_CONFIG_DIR/config.jsonc" "$WAYBAR_CONFIG_DIR/config.json" "$WAYBAR_CONFIG_DIR/config"; do
        if [[ -f "$f" ]]; then
            config_file="$f"
            break
        fi
    done
    
    if [[ -z "$config_file" ]]; then
        warn "No Waybar config found in $WAYBAR_CONFIG_DIR"
        warn "Creating new config.jsonc"
        config_file="$WAYBAR_CONFIG_DIR/config.jsonc"
        echo '{"modules-right": []}' > "$config_file"
    fi
    
    # Check if module DEFINITION exists (not just reference in modules array)
    # Look for "custom/hyprwhspr": { pattern which indicates the definition block
    if grep -qE '"custom/hyprwhspr"\s*:\s*\{' "$config_file" 2>/dev/null; then
        success "Waybar module definition already exists"
        return 0
    fi
    
    if command -v python3 &>/dev/null; then
        python3 << PYEOF
import json
import re
import sys

config_file = "$config_file"

with open(config_file, 'r') as f:
    content = f.read()

# Strip JSONC features to make valid JSON
# Remove // comments (but not :// in URLs)
content_clean = re.sub(r'(?<!:)//.*$', '', content, flags=re.MULTILINE)
# Remove /* */ comments
content_clean = re.sub(r'/\*.*?\*/', '', content_clean, flags=re.DOTALL)
# Remove trailing commas before ] or }
content_clean = re.sub(r',(\s*[}\]])', r'\1', content_clean)

try:
    config = json.loads(content_clean)
except json.JSONDecodeError as e:
    print(f"Warning: Could not parse Waybar config as JSON: {e}", file=sys.stderr)
    sys.exit(1)

# Always add/update the module definition
config["custom/hyprwhspr"] = {
    "exec": "cat \${XDG_CACHE_HOME:-\$HOME/.cache}/hyprwhspr-rs/status.json 2>/dev/null || echo '{\"text\":\"󰍭\",\"class\":\"inactive\",\"tooltip\":\"Not running\"}'",
    "return-type": "json",
    "format": "{text}",
    "interval": 1,
    "tooltip": True,
    "on-click": "walker --provider menus:hyprwhspr"
}

# Add to modules-right (first position) if not already there
if "modules-right" in config and isinstance(config["modules-right"], list):
    if "custom/hyprwhspr" not in config["modules-right"]:
        config["modules-right"].insert(0, "custom/hyprwhspr")
elif "modules-left" in config and isinstance(config["modules-left"], list):
    if "custom/hyprwhspr" not in config["modules-left"]:
        config["modules-left"].insert(0, "custom/hyprwhspr")
else:
    config["modules-right"] = ["custom/hyprwhspr"]

with open(config_file, 'w') as f:
    json.dump(config, f, indent=2)

print("Module definition added successfully")
PYEOF
        success "Added custom/hyprwhspr module definition to Waybar config"
    else
        warn "python3 not found - please add the module manually"
        echo ""
        echo "Add this to your Waybar config ($config_file):"
        cat "$REPO_DIR/config/waybar/hyprwhspr-module.jsonc"
        echo ""
        echo "And add \"custom/hyprwhspr\" to modules-right"
    fi
}

install_waybar_css() {
    info "Configuring Waybar CSS..."
    
    local waybar_style="$WAYBAR_CONFIG_DIR/style.css"
    
    if [[ ! -f "$waybar_style" ]]; then
        info "Creating new style.css"
        touch "$waybar_style"
    fi
    
    if grep -q "#custom-hyprwhspr" "$waybar_style" 2>/dev/null; then
        success "Waybar CSS already contains hyprwhspr styles"
        return 0
    fi
    
    echo "" >> "$waybar_style"
    cat "$REPO_DIR/config/waybar/hyprwhspr-style.css" >> "$waybar_style"
    
    success "Appended CSS to $waybar_style"
}

reload_waybar() {
    info "Reloading Waybar..."
    
    if pgrep -x waybar &>/dev/null; then
        pkill -SIGUSR2 waybar || true
        sleep 0.5
        success "Waybar reloaded"
    else
        warn "Waybar not running - start it manually"
    fi
}

install_elephant_menu() {
    if [[ "$WITH_ELEPHANT" != true ]]; then
        return 0
    fi
    
    info "Installing Elephant menu for Walker..."
    
    mkdir -p "$ELEPHANT_MENU_DIR"
    
    local src="$REPO_DIR/config/elephant/hyprwhspr.lua"
    local dst="$ELEPHANT_MENU_DIR/hyprwhspr.lua"
    
    if [[ ! -f "$src" ]]; then
        error "Elephant menu file not found: $src"
        return 1
    fi
    
    cp "$src" "$dst"
    
    success "Elephant menu installed"
    
    if ! command -v elephant &>/dev/null; then
        warn "Elephant not found in PATH"
        warn "Install from: https://github.com/abenz1267/elephant"
    fi
}

print_summary() {
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    success "Installation complete!"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    
    if [[ ${#BACKUPS_MADE[@]} -gt 0 ]]; then
        echo "Backups created:"
        for backup in "${BACKUPS_MADE[@]}"; do
            echo "  $backup"
        done
        echo ""
    fi
    
    echo "The hyprwhspr module should now appear in your Waybar."
    echo ""
    echo "Files:"
    echo "  Config:  $HYPRWHSPR_CONFIG_DIR/"
    echo "  Env:     $HYPRWHSPR_CONFIG_DIR/env"
    echo "  Status:  $XDG_CACHE_HOME/hyprwhspr-rs/status.json"
    echo "  History: $XDG_DATA_HOME/hyprwhspr-rs/transcriptions.json"
    echo ""
    echo "Commands:"
    echo "  Check status:   systemctl --user status hyprwhspr-rs"
    echo "  View logs:      journalctl --user -u hyprwhspr-rs -f"
    echo "  Restart:        systemctl --user restart hyprwhspr-rs"
    echo ""
    if [[ "$WITH_ELEPHANT" == true ]]; then
        echo "Walker menu:      walker --provider menus:hyprwhspr"
        echo ""
    fi
}

main() {
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "  hyprwhspr-rs Waybar Integration Installer"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    
    backup_all_existing_configs
    create_directories
    setup_env_file
    install_systemd_service
    install_waybar_module
    install_waybar_css
    install_elephant_menu
    start_service
    reload_waybar
    print_summary
}

main "$@"
