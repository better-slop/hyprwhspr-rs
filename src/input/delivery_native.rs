//! Linux/Wayland effects. No dispatch fallback: selecting a capability happens
//! before clipboard publication, and a failed insertion is never replayed.

use super::{Capabilities, DeliveryEffects, EffectFuture, Focus, Insertion};
use crate::input::hyprland::HyprlandDispatcher;
use crate::input::paste_route::{PasteRoute, PasteShortcut};
use anyhow::{bail, Context, Result};
use std::env;
use wrtype::{KeymapBuilder, Modifier, WrtypeClient};

#[path = "delivery_clipboard.rs"]
mod clipboard;

type PreparedPaste = Box<dyn FnOnce() -> Result<()> + Send>;

pub(super) struct NativeEffects {
    wayland: bool,
    hyprland: Option<HyprlandDispatcher>,
    keyboard: Option<WrtypeClient>,
    paste_keyboard: Option<PreparedPaste>,
}

impl NativeEffects {
    pub(super) fn new() -> Self {
        Self {
            wayland: env::var_os("WAYLAND_DISPLAY").is_some(),
            hyprland: HyprlandDispatcher::new(),
            keyboard: None,
            paste_keyboard: None,
        }
    }
}

impl DeliveryEffects for NativeEffects {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            hyprland: self.hyprland.is_some(),
            wayland: self.wayland,
        }
    }

    fn focus(&mut self) -> EffectFuture<'_, Option<Focus>> {
        Box::pin(async {
            match self.hyprland.as_ref() {
                Some(dispatcher) => {
                    let window = dispatcher
                        .active_window()
                        .await?
                        .context("No focused Hyprland window")?;
                    Ok(Some(Focus {
                        address: window.address,
                        class: window.class,
                    }))
                }
                // Generic virtual keyboards address current focus. Without a
                // compositor identity API we cannot validate a specific target.
                None => Ok(None),
            }
        })
    }

    fn prepare(&mut self, insertion: Insertion) -> Result<()> {
        match insertion {
            Insertion::Paste(PasteRoute::Hyprland(_)) => {
                self.hyprland
                    .as_ref()
                    .context("Hyprland paste capability unavailable")?;
            }
            Insertion::Type | Insertion::Paste(PasteRoute::VirtualKeyboard(_)) => {
                if !self.wayland {
                    bail!("Direct typing and virtual keyboard paste require a Wayland compositor supporting the virtual keyboard protocol");
                }
                match insertion {
                    Insertion::Type => {
                        self.keyboard = Some(
                            WrtypeClient::new()
                                .context("Cannot initialize Wayland virtual keyboard")?,
                        )
                    }
                    Insertion::Paste(PasteRoute::VirtualKeyboard(shortcut)) => {
                        self.paste_keyboard = Some(prepare_virtual_paste(shortcut)?)
                    }
                    _ => unreachable!("Hyprland handled above"),
                }
            }
        }
        Ok(())
    }

    fn settle_focus(&mut self) -> EffectFuture<'_, ()> {
        Box::pin(async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok(())
        })
    }

    fn validate_focus<'a>(&'a mut self, expected: Option<&'a Focus>) -> EffectFuture<'a, ()> {
        Box::pin(async move {
            if let Some(expected) = expected {
                let current = self.focus().await?;
                if current.as_ref() != Some(expected) {
                    bail!("Focused window changed before delivery; no insertion attempted");
                }
            }
            Ok(())
        })
    }

    fn publish_clipboard<'a>(&'a mut self, text: &'a str) -> EffectFuture<'a, ()> {
        Box::pin(async move {
            if !self.wayland {
                bail!("Clipboard delivery requires Wayland data-control support");
            }
            clipboard::publish(text).await
        })
    }

    fn insert<'a>(&'a mut self, insertion: Insertion, text: &'a str) -> EffectFuture<'a, ()> {
        Box::pin(async move {
            match insertion {
                Insertion::Type => self
                    .keyboard
                    .as_mut()
                    .context("Typing capability was not prepared")?
                    .type_text(text),
                Insertion::Paste(PasteRoute::Hyprland(shortcut)) => {
                    let dispatcher = self
                        .hyprland
                        .as_ref()
                        .context("Hyprland capability was not prepared")?;
                    match shortcut {
                        PasteShortcut::CtrlV => dispatcher.send_paste_shortcut(false).await,
                        PasteShortcut::CtrlShiftV => dispatcher.send_paste_shortcut(true).await,
                        PasteShortcut::ShiftInsert => dispatcher.send_global_paste_shortcut().await,
                    }
                }
                Insertion::Paste(PasteRoute::VirtualKeyboard(_)) => {
                    self.paste_keyboard
                        .take()
                        .context("Paste capability was not prepared")?()
                }
            }
        })
    }

    fn cleanup(&mut self, _: Insertion) -> Result<()> {
        // Each operation owns a fresh keyboard connection. The paste closure
        // releases key/modifiers on every insertion path before disconnecting.
        self.keyboard = None;
        self.paste_keyboard = None;
        Ok(())
    }
}

fn prepare_virtual_paste(shortcut: PasteShortcut) -> Result<PreparedPaste> {
    let (connection, mut state) =
        wrtype::connect_wayland().context("Cannot initialize Wayland paste keyboard")?;
    let mut keymap = KeymapBuilder::new();
    let key = if shortcut == PasteShortcut::ShiftInsert {
        "Insert"
    } else {
        "v"
    };
    let keycode = keymap.get_keycode_for_key_name(key)?;
    // wrtype 0.1's high-level send_shortcut uploads the keymap while modifiers
    // are held. Compositors can reset modifiers on keymap replacement, turning
    // Ctrl+V into a literal v. Its public primitives let us finish keymap setup
    // before any modifiers: https://docs.rs/wrtype/0.1.0/wrtype/wayland/index.html
    state.upload_keymap(&keymap.generate_keymap())?;
    connection
        .roundtrip()
        .context("Cannot prepare Wayland paste keymap")?;
    Ok(Box::new(move || {
        dispatch_virtual_shortcut(shortcut, |action| {
            match action {
                KeyAction::Modifiers(modifiers) => state.set_modifiers(modifiers)?,
                KeyAction::Press => state.press_key(keycode)?,
                KeyAction::Release => state.release_key(keycode)?,
            }
            connection
                .roundtrip()
                .context("Wayland paste keyboard roundtrip failed")?;
            Ok(())
        })
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyAction {
    Modifiers(u32),
    Press,
    Release,
}

fn dispatch_virtual_shortcut(
    shortcut: PasteShortcut,
    mut send: impl FnMut(KeyAction) -> Result<()>,
) -> Result<()> {
    let modifiers = match shortcut {
        PasteShortcut::CtrlV => Modifier::Ctrl as u32,
        PasteShortcut::CtrlShiftV => Modifier::Ctrl as u32 | Modifier::Shift as u32,
        PasteShortcut::ShiftInsert => Modifier::Shift as u32,
    };
    let insertion = send(KeyAction::Modifiers(modifiers)).and_then(|_| send(KeyAction::Press));
    // Do not use ? before cleanup: a failed flush may follow a successful key
    // press. Always try key release AND modifier reset, even after either fails.
    let release = send(KeyAction::Release);
    let reset = send(KeyAction::Modifiers(0));
    insertion
        .and(release)
        .and(reset)
        .context("Wayland paste shortcut or cleanup failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_uses_exact_modifiers_then_cleans_up() {
        for (shortcut, modifiers) in [
            (PasteShortcut::CtrlV, 4),
            (PasteShortcut::CtrlShiftV, 5),
            (PasteShortcut::ShiftInsert, 1),
        ] {
            let mut actions = Vec::new();
            dispatch_virtual_shortcut(shortcut, |action| {
                actions.push(action);
                Ok(())
            })
            .unwrap();
            assert_eq!(
                actions,
                [
                    KeyAction::Modifiers(modifiers),
                    KeyAction::Press,
                    KeyAction::Release,
                    KeyAction::Modifiers(0)
                ]
            );
        }
    }

    #[test]
    fn shortcut_cleanup_attempts_key_release_and_modifier_reset_after_any_failure() {
        for failed in [
            KeyAction::Modifiers(5),
            KeyAction::Press,
            KeyAction::Release,
            KeyAction::Modifiers(0),
        ] {
            let mut actions = Vec::new();
            let result = dispatch_virtual_shortcut(PasteShortcut::CtrlShiftV, |action| {
                actions.push(action);
                if action == failed {
                    bail!("controlled native failure");
                }
                Ok(())
            });
            assert!(result.is_err());
            assert_eq!(
                &actions[actions.len() - 2..],
                &[KeyAction::Release, KeyAction::Modifiers(0)]
            );
        }
    }
}
