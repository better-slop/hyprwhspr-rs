use anyhow::{Context, Result};
use evdev::KeyCode;
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
    Cancel,
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

pub(super) enum ShortcutInput<'a> {
    KeyStateChanged {
        pressed_keys: &'a HashSet<KeyCode>,
        now: Instant,
    },
    DeviceSetChanged {
        pressed_keys: &'a HashSet<KeyCode>,
        now: Instant,
    },
    ConfigChanged {
        shortcuts: ShortcutsConfig,
        pressed_keys: &'a HashSet<KeyCode>,
        now: Instant,
    },
}

#[derive(Debug)]
pub(super) struct ShortcutController {
    bindings: Vec<ShortcutBinding>,
}

#[derive(Debug)]
struct ShortcutBinding {
    kind: ShortcutKind,
    name: String,
    keys: HashSet<KeyCode>,
    active: bool,
    last_trigger: Instant,
}

impl ShortcutController {
    pub(super) fn new(shortcuts: ShortcutsConfig) -> Result<Self> {
        let mut controller = Self {
            bindings: Vec::new(),
        };
        controller.transition(ShortcutInput::ConfigChanged {
            shortcuts,
            pressed_keys: &HashSet::new(),
            now: Instant::now(),
        })?;
        Ok(controller)
    }

    pub(super) fn transition(&mut self, input: ShortcutInput<'_>) -> Result<Vec<ShortcutEvent>> {
        match input {
            ShortcutInput::KeyStateChanged { pressed_keys, now } => {
                Ok(self.apply_key_state(pressed_keys, now))
            }
            ShortcutInput::DeviceSetChanged { pressed_keys, now } => {
                let _ = pressed_keys;
                Ok(self.cancel_holds(now))
            }
            ShortcutInput::ConfigChanged {
                shortcuts,
                pressed_keys,
                now,
            } => self.replace_shortcuts(shortcuts, pressed_keys, now),
        }
    }

    fn replace_shortcuts(
        &mut self,
        shortcuts: ShortcutsConfig,
        pressed_keys: &HashSet<KeyCode>,
        now: Instant,
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
                    triggered_at: now,
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

    fn apply_key_state(
        &mut self,
        pressed_keys: &HashSet<KeyCode>,
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

    fn cancel_holds(&mut self, now: Instant) -> Vec<ShortcutEvent> {
        let mut events = Vec::new();

        for binding in &mut self.bindings {
            if binding.kind == ShortcutKind::Hold {
                binding.active = false;
                events.push(ShortcutEvent {
                    triggered_at: now,
                    kind: ShortcutKind::Hold,
                    phase: ShortcutPhase::Cancel,
                });
            }
        }

        events
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

pub(super) fn parse_shortcut(shortcut: &str) -> Result<HashSet<KeyCode>> {
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

fn parse_key(key_str: &str) -> Result<KeyCode> {
    match key_str {
        "SUPER" | "META" | "WIN" | "WINDOWS" => Ok(KeyCode::KEY_LEFTMETA),
        "ALT" => Ok(KeyCode::KEY_LEFTALT),
        "CTRL" | "CONTROL" => Ok(KeyCode::KEY_LEFTCTRL),
        "SHIFT" => Ok(KeyCode::KEY_LEFTSHIFT),
        "F1" => Ok(KeyCode::KEY_F1),
        "F2" => Ok(KeyCode::KEY_F2),
        "F3" => Ok(KeyCode::KEY_F3),
        "F4" => Ok(KeyCode::KEY_F4),
        "F5" => Ok(KeyCode::KEY_F5),
        "F6" => Ok(KeyCode::KEY_F6),
        "F7" => Ok(KeyCode::KEY_F7),
        "F8" => Ok(KeyCode::KEY_F8),
        "F9" => Ok(KeyCode::KEY_F9),
        "F10" => Ok(KeyCode::KEY_F10),
        "F11" => Ok(KeyCode::KEY_F11),
        "F12" => Ok(KeyCode::KEY_F12),
        "A" => Ok(KeyCode::KEY_A),
        "B" => Ok(KeyCode::KEY_B),
        "C" => Ok(KeyCode::KEY_C),
        "D" => Ok(KeyCode::KEY_D),
        "E" => Ok(KeyCode::KEY_E),
        "F" => Ok(KeyCode::KEY_F),
        "G" => Ok(KeyCode::KEY_G),
        "H" => Ok(KeyCode::KEY_H),
        "I" => Ok(KeyCode::KEY_I),
        "J" => Ok(KeyCode::KEY_J),
        "K" => Ok(KeyCode::KEY_K),
        "L" => Ok(KeyCode::KEY_L),
        "M" => Ok(KeyCode::KEY_M),
        "N" => Ok(KeyCode::KEY_N),
        "O" => Ok(KeyCode::KEY_O),
        "P" => Ok(KeyCode::KEY_P),
        "Q" => Ok(KeyCode::KEY_Q),
        "R" => Ok(KeyCode::KEY_R),
        "S" => Ok(KeyCode::KEY_S),
        "T" => Ok(KeyCode::KEY_T),
        "U" => Ok(KeyCode::KEY_U),
        "V" => Ok(KeyCode::KEY_V),
        "W" => Ok(KeyCode::KEY_W),
        "X" => Ok(KeyCode::KEY_X),
        "Y" => Ok(KeyCode::KEY_Y),
        "Z" => Ok(KeyCode::KEY_Z),
        "0" => Ok(KeyCode::KEY_0),
        "1" => Ok(KeyCode::KEY_1),
        "2" => Ok(KeyCode::KEY_2),
        "3" => Ok(KeyCode::KEY_3),
        "4" => Ok(KeyCode::KEY_4),
        "5" => Ok(KeyCode::KEY_5),
        "6" => Ok(KeyCode::KEY_6),
        "7" => Ok(KeyCode::KEY_7),
        "8" => Ok(KeyCode::KEY_8),
        "9" => Ok(KeyCode::KEY_9),
        "SPACE" => Ok(KeyCode::KEY_SPACE),
        "ENTER" | "RETURN" => Ok(KeyCode::KEY_ENTER),
        "ESC" | "ESCAPE" => Ok(KeyCode::KEY_ESC),
        "TAB" => Ok(KeyCode::KEY_TAB),
        "BACKSPACE" => Ok(KeyCode::KEY_BACKSPACE),
        "DELETE" | "DEL" => Ok(KeyCode::KEY_DELETE),
        "INSERT" | "INS" => Ok(KeyCode::KEY_INSERT),
        "HOME" => Ok(KeyCode::KEY_HOME),
        "END" => Ok(KeyCode::KEY_END),
        "PAGEUP" | "PGUP" => Ok(KeyCode::KEY_PAGEUP),
        "PAGEDOWN" | "PGDOWN" => Ok(KeyCode::KEY_PAGEDOWN),
        "UP" => Ok(KeyCode::KEY_UP),
        "DOWN" => Ok(KeyCode::KEY_DOWN),
        "LEFT" => Ok(KeyCode::KEY_LEFT),
        "RIGHT" => Ok(KeyCode::KEY_RIGHT),
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

    fn key_transition(
        controller: &mut ShortcutController,
        pressed_keys: &HashSet<KeyCode>,
        now: Instant,
    ) -> Vec<ShortcutEvent> {
        controller
            .transition(ShortcutInput::KeyStateChanged { pressed_keys, now })
            .unwrap()
    }

    #[test]
    fn press_shortcut_debounces() {
        let mut controller = ShortcutController::new(config(Some("SUPER+R"), None)).unwrap();
        let keys = parse_shortcut("SUPER+R").unwrap();
        let now = Instant::now();

        let first = key_transition(&mut controller, &keys, now);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].kind, ShortcutKind::Press);
        assert_eq!(first[0].phase, ShortcutPhase::Start);

        let released = HashSet::new();
        key_transition(&mut controller, &released, now + Duration::from_millis(10));
        let second = key_transition(&mut controller, &keys, now + Duration::from_millis(100));
        assert!(second.is_empty());
    }

    #[test]
    fn hold_shortcut_emits_start_and_end() {
        let mut controller = ShortcutController::new(config(None, Some("SUPER+ALT"))).unwrap();
        let keys = parse_shortcut("SUPER+ALT").unwrap();
        let now = Instant::now();

        let start = key_transition(&mut controller, &keys, now);
        assert_eq!(start.len(), 1);
        assert_eq!(start[0].phase, ShortcutPhase::Start);

        let released = HashSet::new();
        let end = key_transition(&mut controller, &released, now + Duration::from_millis(10));
        assert_eq!(end.len(), 1);
        assert_eq!(end[0].phase, ShortcutPhase::End);
    }

    #[test]
    fn device_churn_emits_hold_cancel_even_when_inactive() {
        let mut controller = ShortcutController::new(config(None, Some("SUPER+ALT"))).unwrap();
        let now = Instant::now();

        let released = HashSet::new();
        let first = controller
            .transition(ShortcutInput::DeviceSetChanged {
                pressed_keys: &released,
                now: now + Duration::from_millis(10),
            })
            .unwrap();

        assert_eq!(first.len(), 1);
        assert_eq!(first[0].phase, ShortcutPhase::Cancel);
    }

    #[test]
    fn config_update_removing_active_hold_emits_end() {
        let mut controller = ShortcutController::new(config(None, Some("SUPER+ALT"))).unwrap();
        let keys = parse_shortcut("SUPER+ALT").unwrap();
        let now = Instant::now();
        key_transition(&mut controller, &keys, now);

        let releases = controller
            .transition(ShortcutInput::ConfigChanged {
                shortcuts: config(Some("SUPER+R"), None),
                pressed_keys: &keys,
                now: now + Duration::from_millis(10),
            })
            .unwrap();

        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].kind, ShortcutKind::Hold);
        assert_eq!(releases[0].phase, ShortcutPhase::End);
    }

    #[test]
    fn cleared_pressed_keys_releases_active_hold() {
        let mut controller = ShortcutController::new(config(None, Some("SUPER+ALT"))).unwrap();
        let keys = parse_shortcut("SUPER+ALT").unwrap();
        key_transition(&mut controller, &keys, Instant::now());

        let end = key_transition(&mut controller, &HashSet::new(), Instant::now());

        assert_eq!(end.len(), 1);
        assert_eq!(end[0].kind, ShortcutKind::Hold);
        assert_eq!(end[0].phase, ShortcutPhase::End);
    }

    #[test]
    fn device_change_cause_cancels_without_restart() {
        let mut controller = ShortcutController::new(config(None, Some("SUPER+ALT"))).unwrap();
        let keys = parse_shortcut("SUPER+ALT").unwrap();
        let now = Instant::now();
        key_transition(&mut controller, &keys, now);

        let events = controller
            .transition(ShortcutInput::DeviceSetChanged {
                pressed_keys: &keys,
                now: now + Duration::from_millis(10),
            })
            .unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].phase, ShortcutPhase::Cancel);
    }
}
