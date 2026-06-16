use anyhow::{Context, Result};
use evdev::Key;
use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::config::ShortcutsConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutKind {
    Hold,
    Press,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutPhase {
    Start,
    End,
}

#[derive(Debug, Clone)]
pub struct ShortcutEvent {
    pub triggered_at: Instant,
    pub kind: ShortcutKind,
    pub phase: ShortcutPhase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutSummary {
    pub kind: ShortcutKind,
    pub name: String,
    pub active: bool,
}

#[derive(Debug)]
pub(super) struct ShortcutController {
    bindings: Vec<ShortcutBinding>,
}

#[derive(Debug)]
struct ShortcutBinding {
    kind: ShortcutKind,
    name: String,
    keys: HashSet<Key>,
    active: bool,
    last_trigger: Instant,
}

impl ShortcutController {
    pub(super) fn new(shortcuts: ShortcutsConfig) -> Result<Self> {
        let mut controller = Self {
            bindings: Vec::new(),
        };
        controller.replace_shortcuts(shortcuts, &HashSet::new())?;
        Ok(controller)
    }

    pub(super) fn replace_shortcuts(
        &mut self,
        shortcuts: ShortcutsConfig,
        pressed_keys: &HashSet<Key>,
    ) -> Result<Vec<ShortcutEvent>> {
        let mut releases = Vec::new();
        let mut next = Vec::new();

        if let Some(shortcut) = shortcuts.press {
            next.push(ShortcutBinding::new(ShortcutKind::Press, shortcut)?);
        }
        if let Some(shortcut) = shortcuts.hold {
            next.push(ShortcutBinding::new(ShortcutKind::Hold, shortcut)?);
        }

        for old in &self.bindings {
            if old.kind == ShortcutKind::Hold
                && old.active
                && !next
                    .iter()
                    .any(|new| new.kind == old.kind && new.name == old.name)
            {
                releases.push(ShortcutEvent {
                    triggered_at: Instant::now(),
                    kind: ShortcutKind::Hold,
                    phase: ShortcutPhase::End,
                });
            }
        }

        for binding in &mut next {
            if let Some(old) = self
                .bindings
                .iter()
                .find(|old| old.kind == binding.kind && old.name == binding.name)
            {
                binding.active = old.active && binding.keys.is_subset(pressed_keys);
                binding.last_trigger = old.last_trigger;
            }
        }

        self.bindings = next;
        Ok(releases)
    }

    pub(super) fn apply_key_state(
        &mut self,
        pressed_keys: &HashSet<Key>,
        now: Instant,
    ) -> Vec<ShortcutEvent> {
        let mut events = Vec::new();

        for binding in &mut self.bindings {
            let combination_pressed = binding.keys.is_subset(pressed_keys);

            if combination_pressed && !binding.active {
                let should_trigger = match binding.kind {
                    ShortcutKind::Hold => true,
                    ShortcutKind::Press => {
                        now.duration_since(binding.last_trigger) > Duration::from_millis(500)
                    }
                };

                if should_trigger {
                    binding.active = true;
                    binding.last_trigger = now;
                    events.push(ShortcutEvent {
                        triggered_at: now,
                        kind: binding.kind,
                        phase: ShortcutPhase::Start,
                    });
                }
            } else if !combination_pressed && binding.active {
                binding.active = false;
                if binding.kind == ShortcutKind::Hold {
                    events.push(ShortcutEvent {
                        triggered_at: now,
                        kind: ShortcutKind::Hold,
                        phase: ShortcutPhase::End,
                    });
                }
            }
        }

        events
    }

    pub(super) fn on_device_set_changed(&mut self) -> Option<ShortcutEvent> {
        let mut released = false;

        for binding in &mut self.bindings {
            if binding.kind == ShortcutKind::Hold && binding.active {
                binding.active = false;
                released = true;
            }
        }

        released.then(|| ShortcutEvent {
            triggered_at: Instant::now(),
            kind: ShortcutKind::Hold,
            phase: ShortcutPhase::End,
        })
    }

    pub(super) fn summaries(&self) -> Vec<ShortcutSummary> {
        self.bindings
            .iter()
            .map(|binding| ShortcutSummary {
                kind: binding.kind,
                name: binding.name.clone(),
                active: binding.active,
            })
            .collect()
    }
}

impl ShortcutBinding {
    fn new(kind: ShortcutKind, name: String) -> Result<Self> {
        Ok(Self {
            kind,
            keys: parse_shortcut(&name)?,
            name,
            active: false,
            last_trigger: Instant::now() - Duration::from_secs(10),
        })
    }
}

pub(super) fn parse_shortcut(shortcut: &str) -> Result<HashSet<Key>> {
    let mut keys = HashSet::new();

    for part in shortcut.split('+') {
        let part = part.trim().to_uppercase();
        let key = parse_key(&part).with_context(|| format!("Failed to parse key: {}", part))?;
        keys.insert(key);
    }

    if keys.is_empty() {
        return Err(anyhow::anyhow!("Empty shortcut"));
    }

    Ok(keys)
}

fn parse_key(key_str: &str) -> Result<Key> {
    match key_str {
        "SUPER" | "META" | "WIN" | "WINDOWS" => Ok(Key::KEY_LEFTMETA),
        "ALT" => Ok(Key::KEY_LEFTALT),
        "CTRL" | "CONTROL" => Ok(Key::KEY_LEFTCTRL),
        "SHIFT" => Ok(Key::KEY_LEFTSHIFT),
        "F1" => Ok(Key::KEY_F1),
        "F2" => Ok(Key::KEY_F2),
        "F3" => Ok(Key::KEY_F3),
        "F4" => Ok(Key::KEY_F4),
        "F5" => Ok(Key::KEY_F5),
        "F6" => Ok(Key::KEY_F6),
        "F7" => Ok(Key::KEY_F7),
        "F8" => Ok(Key::KEY_F8),
        "F9" => Ok(Key::KEY_F9),
        "F10" => Ok(Key::KEY_F10),
        "F11" => Ok(Key::KEY_F11),
        "F12" => Ok(Key::KEY_F12),
        "A" => Ok(Key::KEY_A),
        "B" => Ok(Key::KEY_B),
        "C" => Ok(Key::KEY_C),
        "D" => Ok(Key::KEY_D),
        "E" => Ok(Key::KEY_E),
        "F" => Ok(Key::KEY_F),
        "G" => Ok(Key::KEY_G),
        "H" => Ok(Key::KEY_H),
        "I" => Ok(Key::KEY_I),
        "J" => Ok(Key::KEY_J),
        "K" => Ok(Key::KEY_K),
        "L" => Ok(Key::KEY_L),
        "M" => Ok(Key::KEY_M),
        "N" => Ok(Key::KEY_N),
        "O" => Ok(Key::KEY_O),
        "P" => Ok(Key::KEY_P),
        "Q" => Ok(Key::KEY_Q),
        "R" => Ok(Key::KEY_R),
        "S" => Ok(Key::KEY_S),
        "T" => Ok(Key::KEY_T),
        "U" => Ok(Key::KEY_U),
        "V" => Ok(Key::KEY_V),
        "W" => Ok(Key::KEY_W),
        "X" => Ok(Key::KEY_X),
        "Y" => Ok(Key::KEY_Y),
        "Z" => Ok(Key::KEY_Z),
        "0" => Ok(Key::KEY_0),
        "1" => Ok(Key::KEY_1),
        "2" => Ok(Key::KEY_2),
        "3" => Ok(Key::KEY_3),
        "4" => Ok(Key::KEY_4),
        "5" => Ok(Key::KEY_5),
        "6" => Ok(Key::KEY_6),
        "7" => Ok(Key::KEY_7),
        "8" => Ok(Key::KEY_8),
        "9" => Ok(Key::KEY_9),
        "SPACE" => Ok(Key::KEY_SPACE),
        "ENTER" | "RETURN" => Ok(Key::KEY_ENTER),
        "ESC" | "ESCAPE" => Ok(Key::KEY_ESC),
        "TAB" => Ok(Key::KEY_TAB),
        "BACKSPACE" => Ok(Key::KEY_BACKSPACE),
        "DELETE" | "DEL" => Ok(Key::KEY_DELETE),
        "INSERT" | "INS" => Ok(Key::KEY_INSERT),
        "HOME" => Ok(Key::KEY_HOME),
        "END" => Ok(Key::KEY_END),
        "PAGEUP" | "PGUP" => Ok(Key::KEY_PAGEUP),
        "PAGEDOWN" | "PGDOWN" => Ok(Key::KEY_PAGEDOWN),
        "UP" => Ok(Key::KEY_UP),
        "DOWN" => Ok(Key::KEY_DOWN),
        "LEFT" => Ok(Key::KEY_LEFT),
        "RIGHT" => Ok(Key::KEY_RIGHT),
        _ => Err(anyhow::anyhow!("Unknown key: {}", key_str)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(press: Option<&str>, hold: Option<&str>) -> ShortcutsConfig {
        ShortcutsConfig {
            press: press.map(str::to_string),
            hold: hold.map(str::to_string),
        }
    }

    #[test]
    fn press_shortcut_debounces() {
        let mut controller = ShortcutController::new(config(Some("SUPER+R"), None)).unwrap();
        let keys = parse_shortcut("SUPER+R").unwrap();
        let now = Instant::now();

        let first = controller.apply_key_state(&keys, now);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].kind, ShortcutKind::Press);
        assert_eq!(first[0].phase, ShortcutPhase::Start);

        let released = HashSet::new();
        controller.apply_key_state(&released, now + Duration::from_millis(10));
        let second = controller.apply_key_state(&keys, now + Duration::from_millis(100));
        assert!(second.is_empty());
    }

    #[test]
    fn hold_shortcut_emits_start_and_end() {
        let mut controller = ShortcutController::new(config(None, Some("SUPER+ALT"))).unwrap();
        let keys = parse_shortcut("SUPER+ALT").unwrap();
        let now = Instant::now();

        let start = controller.apply_key_state(&keys, now);
        assert_eq!(start.len(), 1);
        assert_eq!(start[0].phase, ShortcutPhase::Start);

        let released = HashSet::new();
        let end = controller.apply_key_state(&released, now + Duration::from_millis(10));
        assert_eq!(end.len(), 1);
        assert_eq!(end[0].phase, ShortcutPhase::End);
    }

    #[test]
    fn device_churn_emits_one_hold_end() {
        let mut controller = ShortcutController::new(config(None, Some("SUPER+ALT"))).unwrap();
        let keys = parse_shortcut("SUPER+ALT").unwrap();
        controller.apply_key_state(&keys, Instant::now());

        let first = controller.on_device_set_changed();
        let second = controller.on_device_set_changed();

        assert_eq!(first.unwrap().phase, ShortcutPhase::End);
        assert!(second.is_none());
    }

    #[test]
    fn config_update_removing_active_hold_emits_end() {
        let mut controller = ShortcutController::new(config(None, Some("SUPER+ALT"))).unwrap();
        let keys = parse_shortcut("SUPER+ALT").unwrap();
        controller.apply_key_state(&keys, Instant::now());

        let releases = controller
            .replace_shortcuts(config(Some("SUPER+R"), None), &keys)
            .unwrap();

        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].kind, ShortcutKind::Hold);
        assert_eq!(releases[0].phase, ShortcutPhase::End);
    }
}
