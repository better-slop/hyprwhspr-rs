# Integration Examples

## Official Integrations

Hyprwhspr-rs currently ships with three main integrations: a [Hyprland](https://github.com/hyprwm/Hyprland) module, a [Waybar](https://github.com/Alexays/Waybar) module, and a [Walker](https://github.com/abenz1267/walker)/[Elephant](https://github.com/abenz1267/elephant) plugin. These services ship with Omarchy by default, but to support more setups, below you can find the data shape for building your own integrations.

**Status Indicator**

- `${XDG_CACHE_HOME}/hyprwhspr-rs/status.json` (fallback: `/tmp/hyprwhspr-rs/status.json`)

  Hyprwhspr-rs streams the active status to this json file.
  
  ```jsonc
  {
    "text": "󰍭",              // Display text (usually an icon)
    "tooltip": "Not running", // Tooltip string (examples: "Ready", "Recording...", "Transcribing...", "Not running", "Error: <message>")
    "class": "inactive",      // inactive | active | processing | error
    "alt": "inactive"         // Mirrors class (for bar alt text)
  }
  ```

**Transcription History**

- `${XDG_DATA_HOME}/hyprwhspr-rs/transcriptions.json` (fallback: `/tmp/hyprwhspr-rs/transcriptions.json`)

  ```json
  [
    {
      "text": "Use NixOS for declarative, reproducible, and reliable system configuration.",
      "timestamp": "2026-02-03 14:22"
    },
    {
      "text": "Use the same data in QuickShell.",
      "timestamp": "2026-02-03 14:21"
    }
  ]
  ```
