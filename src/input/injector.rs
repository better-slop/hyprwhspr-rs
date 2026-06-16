use anyhow::{anyhow, Context, Result};
use arboard::Clipboard;
use enigo::{Keyboard, Settings};
use serde_json::Value;
use std::collections::HashSet;
use std::env;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tracing::{debug, info, warn};
use wl_clipboard_rs::copy::{ClipboardType, Error as WlCopyError, MimeType, Options, Source};
use wrtype::{Modifier, WrtypeClient};

const SHIFT_PASTE_CLASSES: &[&str] = &[
    "Alacritty",
    "kitty",
    "foot",
    "footclient",
    "WezTerm",
    "org.wezfurlong.wezterm",
    "org.gnome.Console",
    "gnome-terminal-server",
    "konsole",
    "yakuake",
    "terminator",
    "tilix",
    "termite",
    "xfce4-terminal",
    "wezterm-gui",
    "rio",
    "WarpTerminal",
    "xterm",
    "urxvt",
    "Ghostty",
    "ghostty",
    "com.mitchellh.ghostty",
];

const SHIFT_PASTE_CLASS_COMPONENTS: &[&str] = &[
    "terminal",
    "console",
    "ghostty",
    "wezterm",
    "kitty",
    "alacritty",
    "warpterminal",
    "rio",
    "foot",
    "konsole",
    "xterm",
    "urxvt",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ClassPasteShortcut {
    CtrlShiftV,
    ShiftInsert,
}

impl ClassPasteShortcut {
    fn as_str(self) -> &'static str {
        match self {
            Self::CtrlShiftV => "ctrl+shift+v",
            Self::ShiftInsert => "shift+insert",
        }
    }
}

struct HyprlandDispatcher {
    socket_path: PathBuf,
}

impl HyprlandDispatcher {
    fn new() -> Option<Self> {
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

    async fn send_paste_shortcut(&self, use_shift: bool) -> Result<()> {
        let modifiers = if use_shift {
            &["ctrl", "shift"][..]
        } else {
            &["ctrl"][..]
        };
        self.send_shortcut(modifiers, "v", Some("active")).await
    }

    async fn send_global_paste_shortcut(&self) -> Result<()> {
        // Universal paste: Shift+Insert works in most applications including terminals
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

    async fn active_window_class(&self) -> Result<Option<String>> {
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

pub struct TextInjector {
    clipboard: Clipboard,
    extra_shift_classes: Vec<String>,
    extra_shift_insert_classes: Vec<String>,
    default_shift_paste: bool,
    global_paste_shortcut: bool,
    hyprland_dispatcher: Option<HyprlandDispatcher>,
    wrtype_client: Option<WrtypeClient>,
    wrtype_attempted: bool,
    wayland_env: bool,
    wayland_clipboard_enabled: bool,
}

impl TextInjector {
    pub fn new(
        shift_paste_default: bool,
        global_paste_shortcut: bool,
        extra_shift_classes: Vec<String>,
        extra_shift_insert_classes: Vec<String>,
        _auto_copy_clipboard: bool,
    ) -> Result<Self> {
        let clipboard = Clipboard::new().context("Failed to initialize clipboard")?;

        let wayland_env = env::var("WAYLAND_DISPLAY").is_ok();
        let hyprland_dispatcher = HyprlandDispatcher::new();

        if hyprland_dispatcher.is_some() {
            debug!("Hyprland IPC detected; enabling sendshortcut paste integration");
        } else if wayland_env {
            debug!("Wayland session detected without Hyprland IPC; virtual keyboard fallback will be used");
        }

        Ok(Self {
            clipboard,
            extra_shift_classes: normalize_hint_classes(extra_shift_classes),
            extra_shift_insert_classes: normalize_hint_classes(extra_shift_insert_classes),
            default_shift_paste: shift_paste_default,
            global_paste_shortcut,
            hyprland_dispatcher,
            wrtype_client: None,
            wrtype_attempted: false,
            wayland_env,
            wayland_clipboard_enabled: wayland_env,
        })
    }

    pub async fn inject_text(&mut self, text: &str) -> Result<()> {
        if text.trim().is_empty() {
            debug!("No text to inject (empty or whitespace)");
            return Ok(());
        }

        info!("Injecting text: {} characters", text.len());

        // Copy to clipboard using available backends
        self.copy_processed_text(text)?;

        // Small delay to ensure window focus is ready for input (especially on Wayland/XWayland)
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let use_global_paste = self.global_paste_shortcut;

        if use_global_paste {
            // Global mode always overrides class-based hint routing.
            return self.inject_via_shift_insert().await;
        }

        // Window-based paste mode: use Ctrl+V or Ctrl+Shift+V based on window class
        let mut class_hint: Option<ClassPasteShortcut> = None;
        let default_shift = self.default_shift_paste;

        if let Some(dispatcher) = self.hyprland_dispatcher.as_ref() {
            match dispatcher.active_window_class().await {
                Ok(class_opt) => {
                    if let Some(class) = class_opt {
                        if let Some(shortcut) = class_paste_hint_for_class(
                            &class,
                            &self.extra_shift_classes,
                            &self.extra_shift_insert_classes,
                        ) {
                            debug!(
                                class = class.as_str(),
                                shortcut = shortcut.as_str(),
                                "Hyprland active window classification"
                            );
                            class_hint = Some(shortcut);
                        } else {
                            debug!(
                                class = class.as_str(),
                                default = default_shift,
                                "Hyprland active window classification has no explicit paste rule"
                            );
                        }
                    }
                }
                Err(err) => {
                    warn!("Failed to query Hyprland active window class: {err:?}");
                }
            }

            if matches!(class_hint, Some(ClassPasteShortcut::ShiftInsert)) {
                debug!("Class hint selected Shift+Insert paste path");
                return self.inject_via_shift_insert().await;
            }

            let use_shift = match class_hint {
                Some(ClassPasteShortcut::CtrlShiftV) => true,
                _ => default_shift,
            };
            debug!(use_shift, "Hyprland sendshortcut paste attempt");

            match dispatcher.send_paste_shortcut(use_shift).await {
                Ok(_) => {
                    info!("✅ Text injected via Hyprland sendshortcut");
                    return Ok(());
                }
                Err(err) => {
                    warn!("Hyprland sendshortcut paste failed: {err:?}");
                }
            }
        }

        if let Some(client) = self.ensure_wrtype_client() {
            let use_shift = match class_hint {
                Some(ClassPasteShortcut::CtrlShiftV) => true,
                _ => default_shift,
            };
            match send_virtual_keyboard_paste(client, use_shift) {
                Ok(_) => {
                    info!("✅ Text injected via Wayland virtual keyboard");
                    return Ok(());
                }
                Err(err) => {
                    warn!("Wayland virtual keyboard paste failed: {err:?}");
                    self.invalidate_wrtype_client();
                }
            }
        }

        debug!("Falling back to Ctrl+Shift+V paste via Enigo");
        self.inject_via_enigo_shift_paste()
    }

    fn copy_processed_text(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }

        if self.wayland_clipboard_enabled {
            match self.copy_wayland_clipboard(text) {
                Ok(_) => {
                    debug!("Text copied to Wayland clipboard");
                }
                Err(err) => {
                    warn!("Wayland clipboard copy failed (falling back to arboard): {err:?}");
                    self.wayland_clipboard_enabled = false;
                }
            }
        }

        self.clipboard
            .set_text(text)
            .context("Failed to copy text to clipboard")?;
        debug!("Text copied to clipboard");
        Ok(())
    }

    async fn inject_via_shift_insert(&mut self) -> Result<()> {
        if let Some(dispatcher) = self.hyprland_dispatcher.as_ref() {
            debug!("Hyprland sendshortcut paste attempt (Shift+Insert)");
            match dispatcher.send_global_paste_shortcut().await {
                Ok(_) => {
                    info!("✅ Text injected via Hyprland paste (Shift+Insert)");
                    return Ok(());
                }
                Err(err) => {
                    warn!("Hyprland paste (Shift+Insert) failed: {err:?}");
                }
            }
        }

        if let Some(client) = self.ensure_wrtype_client() {
            debug!("Wayland virtual keyboard paste attempt (Shift+Insert)");
            match send_virtual_keyboard_global_paste(client) {
                Ok(_) => {
                    info!("✅ Text injected via Wayland virtual keyboard paste (Shift+Insert)");
                    return Ok(());
                }
                Err(err) => {
                    warn!("Wayland virtual keyboard paste (Shift+Insert) failed: {err:?}");
                    self.invalidate_wrtype_client();
                }
            }
        }

        debug!("Falling back to Shift+Insert paste via Enigo");
        self.inject_via_enigo_global_paste()
    }

    fn copy_wayland_clipboard(&self, text: &str) -> Result<(), WlCopyError> {
        let bytes = text.as_bytes().to_vec();

        let mut both = Options::new();
        both.clipboard(ClipboardType::Both);
        match both.copy(
            Source::Bytes(bytes.clone().into_boxed_slice()),
            MimeType::Text,
        ) {
            Ok(_) => Ok(()),
            Err(WlCopyError::PrimarySelectionUnsupported) => {
                let mut regular = Options::new();
                regular.clipboard(ClipboardType::Regular);
                regular.copy(Source::Bytes(bytes.into_boxed_slice()), MimeType::Text)
            }
            Err(err) => Err(err),
        }
    }

    fn ensure_wrtype_client(&mut self) -> Option<&mut WrtypeClient> {
        if !self.wayland_env {
            return None;
        }

        if self.wrtype_client.is_none() && !self.wrtype_attempted {
            self.wrtype_attempted = true;
            match WrtypeClient::new() {
                Ok(client) => {
                    debug!("Initialized Wayland virtual keyboard client");
                    self.wrtype_client = Some(client);
                }
                Err(err) => {
                    warn!("Failed to initialize Wayland virtual keyboard client: {err:?}");
                }
            }
        }

        self.wrtype_client.as_mut()
    }

    fn invalidate_wrtype_client(&mut self) {
        self.wrtype_client = None;
        self.wrtype_attempted = false;
    }

    fn inject_via_enigo_shift_paste(&mut self) -> Result<()> {
        use enigo::{Direction, Key};
        // Initialize fallback keyboard injection only when needed to avoid
        // keeping a persistent virtual keyboard active for the entire app lifetime.
        let mut enigo = enigo::Enigo::new(&Settings::default())
            .context("Failed to initialize Enigo for text injection")?;

        enigo
            .key(Key::Control, Direction::Press)
            .context("Failed to press Ctrl")?;
        enigo
            .key(Key::Shift, Direction::Press)
            .context("Failed to press Shift")?;
        enigo
            .key(Key::Unicode('v'), Direction::Click)
            .context("Failed to press V")?;
        enigo
            .key(Key::Shift, Direction::Release)
            .context("Failed to release Shift")?;
        enigo
            .key(Key::Control, Direction::Release)
            .context("Failed to release Ctrl")?;

        info!("✅ Text injected via Enigo fallback paste");
        Ok(())
    }

    fn inject_via_enigo_global_paste(&mut self) -> Result<()> {
        use enigo::{Direction, Key};
        // Initialize fallback keyboard injection only when needed to avoid
        // keeping a persistent virtual keyboard active for the entire app lifetime.
        let mut enigo = enigo::Enigo::new(&Settings::default())
            .context("Failed to initialize Enigo for text injection")?;

        // Universal paste: Shift+Insert works in most applications including terminals
        enigo
            .key(Key::Shift, Direction::Press)
            .context("Failed to press Shift")?;
        enigo
            .key(Key::Insert, Direction::Click)
            .context("Failed to press Insert")?;
        enigo
            .key(Key::Shift, Direction::Release)
            .context("Failed to release Shift")?;

        info!("✅ Text injected via Enigo universal paste (Shift+Insert)");
        Ok(())
    }
}

fn send_virtual_keyboard_paste(client: &mut WrtypeClient, use_shift: bool) -> Result<()> {
    if use_shift {
        client.send_shortcut(&[Modifier::Ctrl, Modifier::Shift], "v")
    } else {
        client.send_shortcut(&[Modifier::Ctrl], "v")
    }
}

fn send_virtual_keyboard_global_paste(client: &mut WrtypeClient) -> Result<()> {
    // Universal paste: Shift+Insert works in most applications including terminals
    client.send_shortcut(&[Modifier::Shift], "Insert")
}

fn class_paste_hint_for_class(
    class: &str,
    extra_shift_classes: &[String],
    extra_shift_insert_classes: &[String],
) -> Option<ClassPasteShortcut> {
    let shift_index = class_hint_index(class, extra_shift_classes);
    let shift_insert_index = class_hint_index(class, extra_shift_insert_classes);

    match (shift_index, shift_insert_index) {
        (Some(shift), Some(shift_insert)) => {
            if shift <= shift_insert {
                Some(ClassPasteShortcut::CtrlShiftV)
            } else {
                Some(ClassPasteShortcut::ShiftInsert)
            }
        }
        (Some(_), None) => Some(ClassPasteShortcut::CtrlShiftV),
        (None, Some(_)) => Some(ClassPasteShortcut::ShiftInsert),
        (None, None) => {
            if built_in_shift_hint_for_class(class) {
                Some(ClassPasteShortcut::CtrlShiftV)
            } else {
                None
            }
        }
    }
}

fn built_in_shift_hint_for_class(class: &str) -> bool {
    if SHIFT_PASTE_CLASSES
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(class))
    {
        return true;
    }

    let lower = class.to_ascii_lowercase();
    for component in lower.split(['.', '-', '_']) {
        if SHIFT_PASTE_CLASS_COMPONENTS.iter().any(|c| c == &component) {
            return true;
        }
    }

    false
}

fn class_hint_index(class: &str, hints: &[String]) -> Option<usize> {
    if hints.is_empty() {
        return None;
    }

    let lower = class.to_ascii_lowercase();
    let components: HashSet<&str> = lower.split(['.', '-', '_']).collect();
    hints.iter().position(|hint| {
        if hint == &lower {
            return true;
        }
        components.contains(hint.as_str())
    })
}

fn normalize_hint_classes(entries: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    let mut seen = HashSet::new();
    for entry in entries {
        let normalized_entry = entry.trim().to_ascii_lowercase();
        if !normalized_entry.is_empty() && seen.insert(normalized_entry.clone()) {
            normalized.push(normalized_entry);
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn class_paste_hint_uses_first_class_instantiation() {
        let shift_classes = normalize_hint_classes(vec!["foo".to_string(), "zed".to_string()]);
        let shift_insert_classes =
            normalize_hint_classes(vec!["zed".to_string(), "bar".to_string()]);

        assert_eq!(
            class_paste_hint_for_class("dev.zed.Zed", &shift_classes, &shift_insert_classes),
            Some(ClassPasteShortcut::ShiftInsert)
        );
    }

    #[test]
    fn class_hint_lookup_supports_component_names() {
        let hints = normalize_hint_classes(vec!["zed".to_string()]);
        assert_eq!(class_hint_index("dev.zed.Zed", &hints), Some(0));
    }

    #[test]
    fn class_paste_hint_falls_back_to_built_in_terminal_rules() {
        let shift_classes = normalize_hint_classes(Vec::new());
        let shift_insert_classes = normalize_hint_classes(Vec::new());
        assert_eq!(
            class_paste_hint_for_class("kitty", &shift_classes, &shift_insert_classes),
            Some(ClassPasteShortcut::CtrlShiftV)
        );
    }

    #[test]
    fn normalize_hint_classes_dedupes_preserving_first_order() {
        let normalized = normalize_hint_classes(vec![
            "ZED".to_string(),
            "zed".to_string(),
            " dev.zed.zed ".to_string(),
            "".to_string(),
        ]);
        assert_eq!(
            normalized,
            vec!["zed".to_string(), "dev.zed.zed".to_string()]
        );
    }
}
