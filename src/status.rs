use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// XDG-compliant paths for hyprwhspr data
pub mod paths {
    use std::path::PathBuf;

    /// ~/.cache/hyprwhspr/ - ephemeral status for Waybar
    pub fn cache_dir() -> PathBuf {
        directories::BaseDirs::new()
            .map(|d| d.cache_dir().join("hyprwhspr"))
            .unwrap_or_else(|| PathBuf::from("/tmp/hyprwhspr"))
    }

    /// ~/.local/share/hyprwhspr/ - persistent transcription history
    pub fn data_dir() -> PathBuf {
        directories::BaseDirs::new()
            .map(|d| d.data_dir().join("hyprwhspr"))
            .unwrap_or_else(|| PathBuf::from("/tmp/hyprwhspr"))
    }

    pub fn status_file() -> PathBuf {
        cache_dir().join("status.json")
    }

    pub fn history_file() -> PathBuf {
        data_dir().join("transcriptions.json")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WaybarState {
    Inactive,
    Active,
    Processing,
    Error,
}

impl WaybarState {
    fn icon(&self) -> &'static str {
        match self {
            Self::Inactive => "󰍭",  // mic off icon - always visible
            Self::Active => "󰍬",    // mic on icon
            Self::Processing => "󰍬",
            Self::Error => "󰍭",     // mic off with error styling
        }
    }

    fn class(&self) -> &'static str {
        match self {
            Self::Inactive => "inactive",
            Self::Active => "active",
            Self::Processing => "processing",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct WaybarStatus {
    text: String,
    tooltip: String,
    class: String,
    alt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionEntry {
    pub text: String,
    pub timestamp: String,
}

/// Writes recording status for Waybar to read (JSON format)
pub struct StatusWriter {
    status_file: PathBuf,
    history_file: PathBuf,
    max_history: usize,
}

impl StatusWriter {
    pub fn new() -> Result<Self> {
        let status_file = paths::status_file();
        let history_file = paths::history_file();

        fs::create_dir_all(paths::cache_dir()).context("Failed to create cache directory")?;
        fs::create_dir_all(paths::data_dir()).context("Failed to create data directory")?;

        Ok(Self {
            status_file,
            history_file,
            max_history: 20,
        })
    }

    /// Update Waybar status with state and tooltip
    pub fn set_state(&self, state: WaybarState, tooltip: &str) -> Result<()> {
        let status = WaybarStatus {
            text: state.icon().to_string(),
            tooltip: tooltip.to_string(),
            class: state.class().to_string(),
            alt: state.class().to_string(),
        };

        let json = serde_json::to_string(&status).context("Failed to serialize status")?;
        fs::write(&self.status_file, &json).context("Failed to write status file")?;

        tracing::debug!(state = ?state, tooltip = %tooltip, "Updated Waybar status");

        self.signal_waybar();
        Ok(())
    }

    /// Legacy method for backward compatibility
    pub fn set_recording(&self, recording: bool) -> Result<()> {
        if recording {
            self.set_state(WaybarState::Active, "Recording...")
        } else {
            self.set_state(WaybarState::Inactive, "Ready")
        }
    }

    /// Set processing state (transcribing)
    pub fn set_processing(&self) -> Result<()> {
        self.set_state(WaybarState::Processing, "Transcribing...")
    }

    /// Set error state with message
    pub fn set_error(&self, message: &str) -> Result<()> {
        self.set_state(WaybarState::Error, &format!("Error: {}", message))
    }

    pub fn is_recording(&self) -> bool {
        if let Ok(content) = fs::read_to_string(&self.status_file) {
            if let Ok(status) = serde_json::from_str::<WaybarStatus>(&content) {
                return status.class == "active";
            }
        }
        false
    }

    /// Save transcription to history (for Walker/Elephant integration)
    pub fn save_transcription(&self, text: &str) -> Result<()> {
        let mut entries: Vec<TranscriptionEntry> = fs::read_to_string(&self.history_file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        let timestamp = time::OffsetDateTime::now_local()
            .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
            .format(
                &time::format_description::parse("[year]-[month]-[day] [hour]:[minute]")
                    .expect("valid format"),
            )
            .unwrap_or_else(|_| "unknown".to_string());

        entries.insert(
            0,
            TranscriptionEntry {
                text: text.to_string(),
                timestamp,
            },
        );

        entries.truncate(self.max_history);

        let json =
            serde_json::to_string_pretty(&entries).context("Failed to serialize history")?;
        fs::write(&self.history_file, json).context("Failed to write history file")?;

        tracing::debug!(entries = entries.len(), "Saved transcription to history");
        Ok(())
    }

    /// Signal Waybar to refresh the custom module
    fn signal_waybar(&self) {
        // SIGRTMIN+8 for custom module refresh
        // Use full path and run synchronously to ensure it executes
        let result = Command::new("/usr/bin/pkill")
            .args(["-RTMIN+8", "waybar"])
            .status();

        if let Err(e) = result {
            tracing::debug!("Failed to signal waybar: {}", e);
        }
    }

    /// Clean up status file on shutdown
    pub fn cleanup(&self) -> Result<()> {
        if self.status_file.exists() {
            fs::remove_file(&self.status_file).context("Failed to remove status file")?;
            self.signal_waybar();
        }
        Ok(())
    }
}

impl Default for StatusWriter {
    fn default() -> Self {
        Self::new().expect("Failed to create StatusWriter")
    }
}
