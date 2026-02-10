const SYSTEMD_UNIT: &str = include_str!("../config/systemd/hyprwhspr-rs.service");
const WAYBAR_MODULE: &str = include_str!("../config/waybar/hyprwhspr-module.jsonc");
const ELEPHANT_MENU: &str = include_str!("../config/elephant/hyprwhspr.lua");
const WAYBAR_INSTALL_SCRIPT: &str = include_str!("../scripts/install-waybar.sh");

#[test]
fn waybar_module_uses_xdg_cache_home() {
    assert!(
        WAYBAR_MODULE.contains("${XDG_CACHE_HOME:-$HOME/.cache}/hyprwhspr-rs/status.json"),
        "waybar module should use XDG_CACHE_HOME fallback"
    );
    assert!(
        !WAYBAR_MODULE.contains("~/.cache/hyprwhspr-rs/status.json"),
        "waybar module should not hardcode ~/.cache"
    );
}

#[test]
fn waybar_install_script_injects_xdg_cache_home() {
    assert!(
        WAYBAR_INSTALL_SCRIPT.contains("${XDG_CACHE_HOME:-$HOME/.cache}/hyprwhspr-rs/status.json"),
        "install script should inject XDG_CACHE_HOME fallback"
    );
    assert!(
        !WAYBAR_INSTALL_SCRIPT.contains("~/.cache/hyprwhspr-rs/status.json"),
        "install script should not inject hardcoded ~/.cache"
    );
}

#[test]
fn elephant_menu_uses_xdg_data_home() {
    assert!(
        ELEPHANT_MENU.contains("XDG_DATA_HOME"),
        "elephant menu should respect XDG_DATA_HOME"
    );
    assert!(
        ELEPHANT_MENU.contains("/hyprwhspr-rs/transcriptions.json"),
        "elephant menu should point at transcriptions.json"
    );
    assert!(
        !ELEPHANT_MENU.contains("/.local/share/hyprwhspr-rs/transcriptions.json"),
        "elephant menu should not hardcode ~/.local/share"
    );
}

#[test]
fn systemd_unit_uses_path_execstart_and_cache_directory() {
    assert!(
        SYSTEMD_UNIT.contains("ExecStart=hyprwhspr-rs"),
        "systemd unit should ExecStart from PATH"
    );
    assert!(
        SYSTEMD_UNIT.contains("CacheDirectory=hyprwhspr-rs"),
        "systemd unit should use CacheDirectory"
    );
    assert!(
        !SYSTEMD_UNIT.contains(".cargo/bin/hyprwhspr-rs"),
        "systemd unit should not hardcode cargo bin path"
    );
    assert!(
        !SYSTEMD_UNIT.contains("ExecStartPre=/bin/sh"),
        "systemd unit should not shell out for cache dir creation"
    );
}
