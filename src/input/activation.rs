//! Semantic keyboard activation seam. Native adapters own shortcut parsing,
//! registration, device state, and recovery; hosts consume only these events.

use anyhow::Result;
use std::future::Future;
use std::time::Instant;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationHealth {
    Starting,
    Ready,
    Unavailable(String),
    Failed(String),
    Stopped,
}

#[derive(Debug, Clone)]
pub(super) struct SourceSnapshot {
    pub backend: &'static str,
    pub health: ActivationHealth,
    pub shortcuts: Vec<ShortcutSummary>,
}

/// Internal seam shared by native activation and controlled lifecycle tests.
///
/// Adapters remain on their owning runtime thread (libudev is not `Send`).
/// `next` must be cancellation safe: commands may interrupt its wait. Adapters
/// return semantic transitions rather than native keycodes or device handles.
/// A future Windows `handy_keys` adapter can map hotkey IDs/states here without
/// implementing Linux enumeration or evdev state. No Windows adapter is shipped.
///
/// `configure` is atomic on error. `cancel` clears adapter hold state and
/// emits hold cancellation, including any start queued before the host became
/// aware of it. The manager awaits every End/Cancel before exiting.
pub(super) trait ActivationSource {
    fn next(&mut self) -> impl Future<Output = Result<Vec<ShortcutEvent>>>;
    fn configure(&mut self, shortcuts: ShortcutsConfig) -> Result<Vec<ShortcutEvent>>;
    fn cancel(&mut self) -> Vec<ShortcutEvent>;
    fn snapshot(&self) -> SourceSnapshot;
}
