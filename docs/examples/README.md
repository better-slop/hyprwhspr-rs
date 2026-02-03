# Integration Examples

## Official Integrations

Hyprwhspr-rs currently ships with three main integrations: a [Hyprland](https://github.com/hyprwm/Hyprland) module, a [Waybar](https://github.com/Alexays/Waybar) module, and a [Walker](https://github.com/abenz1267/walker)/[Elephant](https://github.com/abenz1267/elephant) plugin. These services ship with Omarchy by default, but to support more setups, below you can find the data shape for building your own integrations.

## `${XDG_CACHE_HOME}/hyprwhspr-rs/status.json` (fallback: `/tmp/hyprwhspr-rs/status.json`)

Hyprwhspr-rs streams the active status to this json file.

```json
{
  
}
```

## `${XDG_DATA_HOME}/hyprwhspr-rs/transcriptions.json` (fallback: `/tmp/hyprwhspr-rs/transcriptions.json`)

```json
{
  
}
```
