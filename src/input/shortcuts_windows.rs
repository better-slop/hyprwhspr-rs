use anyhow::{Context, Result};
use handy_keys::{Hotkey, HotkeyId, HotkeyManager, HotkeyState};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{info, warn};

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

pub struct GlobalShortcuts {
    manager: HotkeyManager,
    hotkey_id: HotkeyId,
    shortcut_name: String,
    kind: ShortcutKind,
}

impl GlobalShortcuts {
    pub fn new(shortcut: &str, kind: ShortcutKind) -> Result<Self> {
        let hotkey: Hotkey = shortcut
            .parse()
            .with_context(|| format!("Failed to parse shortcut: {shortcut}"))?;
        let manager =
            HotkeyManager::new_with_blocking().context("Failed to initialize global shortcuts")?;
        let hotkey_id = manager
            .register(hotkey)
            .with_context(|| format!("Failed to register shortcut: {shortcut}"))?;

        let mode_label = match kind {
            ShortcutKind::Hold => "hold",
            ShortcutKind::Press => "press",
        };

        info!(
            "Global shortcuts initialized - monitoring {} shortcut: {}",
            mode_label, shortcut
        );

        Ok(Self {
            manager,
            hotkey_id,
            shortcut_name: shortcut.to_string(),
            kind,
        })
    }

    pub fn run(self, tx: mpsc::Sender<ShortcutEvent>, stop: Arc<AtomicBool>) -> Result<()> {
        let listen_label = match self.kind {
            ShortcutKind::Hold => "hold",
            ShortcutKind::Press => "press",
        };
        info!(
            "Listening for {} shortcut: {}",
            listen_label, self.shortcut_name
        );

        while !stop.load(Ordering::Relaxed) {
            let event = match self.manager.try_recv() {
                Some(event) => event,
                None => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
            };

            if event.id != self.hotkey_id {
                continue;
            }

            let phase = match (self.kind, event.state) {
                (ShortcutKind::Press, HotkeyState::Pressed) => Some(ShortcutPhase::Start),
                (ShortcutKind::Press, HotkeyState::Released) => None,
                (ShortcutKind::Hold, HotkeyState::Pressed) => Some(ShortcutPhase::Start),
                (ShortcutKind::Hold, HotkeyState::Released) => Some(ShortcutPhase::End),
            };

            let Some(phase) = phase else {
                continue;
            };

            if let Err(err) = tx.try_send(ShortcutEvent {
                triggered_at: Instant::now(),
                kind: self.kind,
                phase,
            }) {
                warn!("Failed to send shortcut event: {}", err);
            }
        }

        info!("Stopping shortcut listener: {}", self.shortcut_name);
        Ok(())
    }
}
