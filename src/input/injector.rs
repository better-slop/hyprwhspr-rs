use anyhow::{Context, Result};
use arboard::Clipboard;
use enigo::{Keyboard, Settings};
use std::env;
use tracing::{debug, info, warn};
use wl_clipboard_rs::copy::{ClipboardType, Error as WlCopyError, MimeType, Options, Source};
use wrtype::{Modifier, WrtypeClient};

use crate::input::hyprland::HyprlandDispatcher;
use crate::input::paste_route::{
    PasteRoute, PasteRouteContext, PasteShortcut, class_paste_hint_for_class,
    normalize_hint_classes, plan_paste_routes,
};

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
            debug!(
                "Wayland session detected without Hyprland IPC; virtual keyboard fallback will be used"
            );
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

        let class_hint = self.active_class_paste_hint().await;
        let routes = plan_paste_routes(PasteRouteContext {
            global_paste_shortcut: self.global_paste_shortcut,
            default_shift_paste: self.default_shift_paste,
            class_hint,
            hyprland_available: self.hyprland_dispatcher.is_some(),
            wayland_env: self.wayland_env,
        });

        for route in routes {
            match self.try_paste_route(route).await {
                Ok(()) => return Ok(()),
                Err(err) => warn!(route = ?route, "Paste route failed: {err:?}"),
            }
        }

        Ok(())
    }

    async fn active_class_paste_hint(&self) -> Option<PasteShortcut> {
        if self.global_paste_shortcut {
            return None;
        }
        let mut class_hint = None;
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
                                default = self.default_shift_paste,
                                "Hyprland active window classification has no explicit paste rule"
                            );
                        }
                    }
                }
                Err(err) => {
                    warn!("Failed to query Hyprland active window class: {err:?}");
                }
            }
        }

        class_hint
    }

    async fn try_paste_route(&mut self, route: PasteRoute) -> Result<()> {
        match route {
            PasteRoute::Hyprland(shortcut) => self.try_hyprland_paste(shortcut).await,
            PasteRoute::VirtualKeyboard(shortcut) => self.try_virtual_keyboard_paste(shortcut),
            PasteRoute::Enigo(shortcut) => self.try_enigo_paste(shortcut),
        }
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

    async fn try_hyprland_paste(&mut self, shortcut: PasteShortcut) -> Result<()> {
        if let Some(dispatcher) = self.hyprland_dispatcher.as_ref() {
            debug!(shortcut = shortcut.as_str(), "Hyprland paste route attempt");
            match shortcut {
                PasteShortcut::CtrlV => dispatcher.send_paste_shortcut(false).await?,
                PasteShortcut::CtrlShiftV => dispatcher.send_paste_shortcut(true).await?,
                PasteShortcut::ShiftInsert => dispatcher.send_global_paste_shortcut().await?,
            };
            info!("✅ Text injected via Hyprland paste route");
            return Ok(());
        }

        Err(anyhow::anyhow!("Hyprland paste route unavailable"))
    }

    fn try_virtual_keyboard_paste(&mut self, shortcut: PasteShortcut) -> Result<()> {
        if let Some(client) = self.ensure_wrtype_client() {
            debug!(
                shortcut = shortcut.as_str(),
                "Wayland virtual keyboard paste route attempt"
            );
            let result = match shortcut {
                PasteShortcut::CtrlV => send_virtual_keyboard_paste(client, false),
                PasteShortcut::CtrlShiftV => send_virtual_keyboard_paste(client, true),
                PasteShortcut::ShiftInsert => send_virtual_keyboard_global_paste(client),
            };

            match result {
                Ok(_) => {
                    info!("✅ Text injected via Wayland virtual keyboard paste route");
                    return Ok(());
                }
                Err(err) => {
                    self.invalidate_wrtype_client();
                    return Err(err);
                }
            }
        }

        Err(anyhow::anyhow!(
            "Wayland virtual keyboard route unavailable"
        ))
    }

    fn try_enigo_paste(&mut self, shortcut: PasteShortcut) -> Result<()> {
        debug!(shortcut = shortcut.as_str(), "Enigo paste route attempt");
        match shortcut {
            PasteShortcut::CtrlV | PasteShortcut::CtrlShiftV => self.inject_via_enigo_shift_paste(),
            PasteShortcut::ShiftInsert => self.inject_via_enigo_global_paste(),
        }
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
