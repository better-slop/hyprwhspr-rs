use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use std::{env, path::PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tracing::debug;

pub(super) struct HyprlandDispatcher {
    socket_path: PathBuf,
}

impl HyprlandDispatcher {
    pub(super) fn new() -> Option<Self> {
        let runtime_dir = env::var("XDG_RUNTIME_DIR").ok()?;
        let signature = env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
        let socket_path = PathBuf::from(runtime_dir)
            .join("hypr")
            .join(signature)
            .join(".socket.sock");

        if socket_path.exists() {
            Some(Self { socket_path })
        } else {
            None
        }
    }

    pub(super) async fn send_paste_shortcut(&self, use_shift: bool) -> Result<()> {
        let modifiers = if use_shift {
            &["ctrl", "shift"][..]
        } else {
            &["ctrl"][..]
        };
        self.send_shortcut(modifiers, "v", Some("active")).await
    }

    pub(super) async fn send_global_paste_shortcut(&self) -> Result<()> {
        // Universal paste: Shift+Insert works in most applications including terminals.
        self.send_shortcut(&["shift"], "Insert", Some("active"))
            .await
    }

    async fn send_shortcut(
        &self,
        modifiers: &[&str],
        key: &str,
        target: Option<&str>,
    ) -> Result<()> {
        let mods_segment = if modifiers.is_empty() {
            String::new()
        } else {
            modifiers.join(" ")
        };
        let target_segment = target.map(|t| format!(", {t}")).unwrap_or_default();
        let command = if mods_segment.is_empty() {
            format!("dispatch sendshortcut {key}{target_segment}")
        } else {
            format!("dispatch sendshortcut {mods_segment}, {key}{target_segment}")
        };
        let response = self.send_command(&command).await?;
        if response.is_empty() || response.eq_ignore_ascii_case("ok") {
            Ok(())
        } else {
            Err(anyhow!("Hyprland sendshortcut error: {response}"))
        }
    }

    pub(super) async fn active_window_class(&self) -> Result<Option<String>> {
        // Try JSON-formatted activewindow first for newer Hyprland releases.
        let json_response = self.send_command("j/activewindow").await?;
        if let Some(class) =
            Self::handle_activewindow_response("j/activewindow", &json_response, true)?
        {
            return Ok(Some(class));
        }

        // Fall back to the plain-text formatter.
        let plain_response = self.send_command("activewindow").await?;
        if let Some(class) =
            Self::handle_activewindow_response("activewindow", &plain_response, false)?
        {
            return Ok(Some(class));
        }

        // Attempt v2 API (yields window address) and resolve via clients list.
        let address_response = self.send_command("activewindowv2").await?;
        if Self::is_unknown_request(&address_response) {
            debug!("Hyprland does not expose activewindow/activewindowv2 on this version");
            return Ok(None);
        }

        let address = address_response
            .split_whitespace()
            .next()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        if let Some(address) = address {
            if let Some(class) = self.lookup_class_by_address(&address).await? {
                return Ok(Some(class));
            }
            debug!(
                address = address.as_str(),
                "Hyprland activewindowv2 address could not be matched to a client class"
            );
        } else {
            debug!("Hyprland activewindowv2 returned no address data");
        }

        Ok(None)
    }

    async fn send_command(&self, command: &str) -> Result<String> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to connect to Hyprland socket at {}",
                    self.socket_path.display()
                )
            })?;

        stream
            .write_all(command.as_bytes())
            .await
            .with_context(|| format!("Failed to send IPC command: {command}"))?;
        stream
            .flush()
            .await
            .context("Failed to flush Hyprland IPC command")?;
        stream
            .shutdown()
            .await
            .context("Failed to finish Hyprland IPC write")?;

        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .context("Failed to read Hyprland IPC response")?;
        let text = String::from_utf8_lossy(&response).trim().to_string();
        debug!(
            command,
            response = text.as_str(),
            "Hyprland IPC response (trimmed)"
        );
        Ok(text)
    }

    fn handle_activewindow_response(
        command: &str,
        response: &str,
        expect_json: bool,
    ) -> Result<Option<String>> {
        let trimmed = response.trim();

        if trimmed.is_empty() {
            debug!(%command, "Hyprland command returned empty string");
            return Ok(None);
        }

        if Self::is_unknown_request(trimmed) {
            debug!(%command, "Hyprland command unsupported on this version");
            return Ok(None);
        }

        if expect_json {
            if let Ok(Some(class)) = Self::extract_window_class_from_response(trimmed) {
                return Ok(Some(class));
            }
        }

        match Self::extract_window_class_from_response(trimmed) {
            Ok(class) => Ok(class),
            Err(err) => {
                debug!(%command, response = trimmed, error = %err, "Hyprland command parse failed");
                Ok(None)
            }
        }
    }

    async fn lookup_class_by_address(&self, address: &str) -> Result<Option<String>> {
        let clients_response = self.send_command("j/clients").await?;
        if Self::is_unknown_request(&clients_response) {
            debug!("Hyprland clients command not available for address lookup");
            return Ok(None);
        }

        if let Some(class) = Self::extract_class_from_clients_json(&clients_response, address) {
            return Ok(Some(class));
        }

        if let Some(class) = Self::extract_class_from_clients_text(&clients_response, address) {
            return Ok(Some(class));
        }

        Ok(None)
    }

    fn extract_window_class_from_response(response: &str) -> Result<Option<String>> {
        if response.is_empty() {
            return Ok(None);
        }

        if let Ok(value) = serde_json::from_str::<Value>(response) {
            return Ok(value
                .get("class")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()));
        }

        for line in response.lines() {
            if let Some((key, value)) = line.trim().split_once(':') {
                if key.trim().eq_ignore_ascii_case("class") {
                    return Ok(Some(value.trim().to_string()));
                }
            }
        }

        Err(anyhow!("No class entry found in Hyprland response"))
    }

    fn extract_class_from_clients_json(text: &str, address: &str) -> Option<String> {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return None;
        };

        let Some(entries) = value.as_array() else {
            return None;
        };

        let target = Self::normalize_address(address);

        for entry in entries {
            let Some(addr) = entry.get("address").and_then(|v| v.as_str()) else {
                continue;
            };
            if Self::normalize_address(addr) == target {
                if let Some(class) = entry
                    .get("class")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .filter(|s| !s.is_empty())
                {
                    return Some(class);
                }
                if let Some(class) = entry
                    .get("initialClass")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .filter(|s| !s.is_empty())
                {
                    return Some(class);
                }
            }
        }

        None
    }

    fn extract_class_from_clients_text(text: &str, address: &str) -> Option<String> {
        let target = Self::normalize_address(address);
        let mut in_target = false;

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                in_target = false;
                continue;
            }

            let lower = trimmed.to_ascii_lowercase();
            if lower.contains(&target) {
                in_target = true;
                if let Some(class) = Self::parse_class_line(trimmed) {
                    return Some(class);
                }
                continue;
            }

            if !in_target {
                continue;
            }

            if let Some(class) = Self::parse_class_line(trimmed) {
                return Some(class);
            }
        }

        None
    }

    fn parse_class_line(line: &str) -> Option<String> {
        let (key, value) = line.split_once(':')?;
        let key = key.trim().to_ascii_lowercase();
        if key == "class" || key == "initialclass" {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
        None
    }

    fn normalize_address(address: &str) -> String {
        let trimmed = address.trim();
        if let Some(stripped) = trimmed.strip_prefix("0x") {
            stripped.to_ascii_lowercase()
        } else {
            trimmed.to_ascii_lowercase()
        }
    }

    fn is_unknown_request(response: &str) -> bool {
        response.trim().eq_ignore_ascii_case("unknown request")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn extracts_class_from_plain_hyprland_output() {
        let sample = r#"
Address: 0x123456
Class: kitty
Title: sample
"#;
        let class = super::HyprlandDispatcher::extract_window_class_from_response(sample).unwrap();
        assert_eq!(class, Some("kitty".to_string()));
    }

    #[test]
    fn extracts_class_from_json_hyprland_output() {
        let sample = r#"{"address":"0x123","class":"foot","title":"shell"}"#;
        let class = super::HyprlandDispatcher::extract_window_class_from_response(sample).unwrap();
        assert_eq!(class, Some("foot".to_string()));
    }
}
