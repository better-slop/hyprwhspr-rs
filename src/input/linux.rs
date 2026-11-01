//! Linux keyboard activation adapter. Polling, udev coalescing, bounded event
//! drains, and pressed-key reconciliation stay private to this adapter.

use anyhow::Result;
use std::time::{Duration, Instant};
use tokio::time;
use tracing::{debug, warn};

use super::activation::{ActivationHealth, ActivationSource, ShortcutEvent, SourceSnapshot};
use super::shortcuts::{ShortcutController, ShortcutInput};
use super::source::{EvdevUdevEventSource, InputSourceEvent};
use crate::config::ShortcutsConfig;

const EVDEV_TICK: Duration = Duration::from_millis(10);
const MAX_INPUT_EVENTS_PER_TICK: usize = 256;

pub(super) struct LinuxActivationSource {
    source: EvdevUdevEventSource,
    shortcuts: ShortcutController,
    evdev_tick: time::Interval,
    refresh_tick: time::Interval,
    stats: LinuxInputStats,
    last_busy_log: Instant,
}

#[derive(Default)]
struct LinuxInputStats {
    udev_events_seen: u64,
    udev_events_coalesced: u64,
    device_refreshes: u64,
    key_events_seen: u64,
    busy_ticks: u64,
}

impl LinuxActivationSource {
    pub(super) fn validate(shortcuts: ShortcutsConfig) -> Result<()> {
        ShortcutController::new(shortcuts).map(|_| ())
    }

    pub(super) fn open(shortcuts: ShortcutsConfig) -> Result<Self> {
        let shortcuts = ShortcutController::new(shortcuts)?;
        let source = EvdevUdevEventSource::open()?;
        let mut evdev_tick = time::interval(EVDEV_TICK);
        evdev_tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        let mut refresh_tick = time::interval(Duration::from_millis(50));
        refresh_tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        Ok(Self {
            source,
            shortcuts,
            evdev_tick,
            refresh_tick,
            stats: LinuxInputStats::default(),
            last_busy_log: Instant::now(),
        })
    }

    fn transitions(&mut self, events: Vec<InputSourceEvent>) -> Result<Vec<ShortcutEvent>> {
        let mut transitions = Vec::new();
        for event in events {
            let now = Instant::now();
            let pressed_keys = self.source.pressed_keys();
            let input = match event {
                InputSourceEvent::KeyStateChanged { key_events } => {
                    self.stats.key_events_seen += key_events as u64;
                    ShortcutInput::KeyStateChanged { pressed_keys, now }
                }
                InputSourceEvent::DeviceSetChanged => {
                    ShortcutInput::DeviceSetChanged { pressed_keys, now }
                }
                InputSourceEvent::SourceBackpressure {
                    drained_events,
                    key_events,
                } => {
                    debug!(
                        drained_events,
                        key_events,
                        MAX_INPUT_EVENTS_PER_TICK,
                        "Input source reached per-tick event cap"
                    );
                    continue;
                }
            };
            transitions.extend(self.shortcuts.transition(input)?);
        }
        Ok(transitions)
    }

    fn log_activity(&mut self, busy: bool) {
        if busy {
            self.stats.busy_ticks += 1;
        }
        if self.last_busy_log.elapsed() < Duration::from_secs(60) {
            return;
        }
        if self.stats.busy_ticks > 1_000 {
            warn!(
                udev_events_seen = self.stats.udev_events_seen,
                udev_events_coalesced = self.stats.udev_events_coalesced,
                device_refreshes = self.stats.device_refreshes,
                key_events_seen = self.stats.key_events_seen,
                loop_busy_ticks = self.stats.busy_ticks,
                "Linux keyboard activation source busy"
            );
        }
        self.stats.busy_ticks = 0;
        self.last_busy_log = Instant::now();
    }
}

impl ActivationSource for LinuxActivationSource {
    async fn next(&mut self) -> Result<Vec<ShortcutEvent>> {
        let events = tokio::select! {
            event = self.source.next_udev_event(), if self.source.udev_events_enabled() => {
                let stats = self.source.handle_udev_event(event);
                self.stats.udev_events_seen += stats.seen;
                self.stats.udev_events_coalesced += stats.coalesced;
                self.log_activity(stats.seen > 0);
                return Ok(Vec::new());
            }
            _ = self.refresh_tick.tick() => {
                if self.source.refresh_due() {
                    self.stats.device_refreshes += 1;
                    self.source.refresh_devices()?
                } else {
                    Vec::new()
                }
            }
            _ = self.evdev_tick.tick() => {
                self.source.poll_input_events(MAX_INPUT_EVENTS_PER_TICK)?
            }
        };
        self.log_activity(!events.is_empty());
        self.transitions(events)
    }

    fn configure(&mut self, shortcuts: ShortcutsConfig) -> Result<Vec<ShortcutEvent>> {
        self.shortcuts.transition(ShortcutInput::ConfigChanged {
            shortcuts,
            pressed_keys: self.source.pressed_keys(),
            now: Instant::now(),
        })
    }

    fn cancel(&mut self) -> Vec<ShortcutEvent> {
        self.shortcuts.cancel_holds(Instant::now())
    }

    fn snapshot(&self) -> SourceSnapshot {
        SourceSnapshot {
            backend: "linux-evdev",
            health: if self.source.device_count() > 0 {
                ActivationHealth::Ready
            } else {
                ActivationHealth::Unavailable(
                    "No readable keyboard devices; check /dev/input permissions".into(),
                )
            },
            shortcuts: self.shortcuts.summaries(),
        }
    }
}
