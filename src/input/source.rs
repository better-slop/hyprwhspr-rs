use anyhow::{Context, Result};
use futures_util::StreamExt;
use std::collections::HashSet;
use std::future;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio_udev::{AsyncMonitorSocket, EventType, MonitorBuilder};
use tracing::warn;

use crate::input::registry::{KeyboardRegistry, PollOutcome, is_input_event_node};

const UDEV_DEBOUNCE: Duration = Duration::from_millis(150);
const UDEV_MAX_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InputSourceEvent {
    KeyStateChanged { key_events: usize },
    DeviceSetChanged,
    SourceBackpressure { key_events: usize },
}

pub(super) trait InputEventSource {
    fn pressed_keys(&self) -> &HashSet<evdev::Key>;
    fn device_count(&self) -> usize;
    fn device_paths(&self) -> Vec<PathBuf>;
}

pub(super) struct EvdevUdevEventSource {
    registry: KeyboardRegistry,
    coalescer: UdevCoalescer,
    udev_stream: Option<AsyncMonitorSocket>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct UdevEventStats {
    pub seen: u64,
    pub coalesced: u64,
}

#[derive(Default)]
struct UdevCoalescer {
    dirty_since: Option<Instant>,
    last_event_at: Option<Instant>,
    fallback_rescan: bool,
    last_fallback_rescan: Option<Instant>,
}

impl EvdevUdevEventSource {
    pub(super) fn open() -> Result<Self> {
        let registry = KeyboardRegistry::open_initial()?;
        let mut coalescer = UdevCoalescer::default();
        let udev_stream = match create_udev_stream() {
            Ok(stream) => Some(stream),
            Err(err) => {
                warn!(
                    "Input device monitor unavailable ({}); falling back to periodic rescan",
                    err
                );
                coalescer.fallback_rescan = true;
                None
            }
        };

        Ok(Self {
            registry,
            coalescer,
            udev_stream,
        })
    }

    pub(super) fn poll_key_events(&mut self, max_events: usize) -> Result<Vec<InputSourceEvent>> {
        events_from_poll(self.registry.poll_key_events(max_events)?)
    }

    pub(super) fn udev_events_enabled(&self) -> bool {
        self.udev_stream.is_some() && !self.coalescer.has_pending_refresh()
    }

    pub(super) async fn next_udev_event(&mut self) -> Option<Result<tokio_udev::Event, io::Error>> {
        match &mut self.udev_stream {
            Some(stream) => stream.next().await,
            None => future::pending().await,
        }
    }

    pub(super) fn handle_udev_event(
        &mut self,
        event: Option<Result<tokio_udev::Event, io::Error>>,
    ) -> UdevEventStats {
        match event {
            Some(Ok(event)) => self.coalescer.mark_dirty_event(&event),
            Some(Err(err)) => {
                warn!(
                    "Input device monitor unavailable ({}); falling back to periodic rescan",
                    err
                );
                self.coalescer.fallback_rescan = true;
                self.udev_stream = None;
                UdevEventStats::default()
            }
            None => {
                warn!("Input device monitor closed; falling back to periodic rescan");
                self.coalescer.fallback_rescan = true;
                self.udev_stream = None;
                UdevEventStats::default()
            }
        }
    }

    pub(super) fn refresh_due(&mut self) -> bool {
        self.coalescer.refresh_due()
    }

    pub(super) fn refresh_devices(&mut self) -> Result<Vec<InputSourceEvent>> {
        let changed = self.registry.refresh()?;
        self.coalescer.clear_dirty();

        Ok(if changed {
            vec![InputSourceEvent::DeviceSetChanged]
        } else {
            Vec::new()
        })
    }
}

impl InputEventSource for EvdevUdevEventSource {
    fn pressed_keys(&self) -> &HashSet<evdev::Key> {
        self.registry.pressed_keys()
    }

    fn device_count(&self) -> usize {
        self.registry.device_count()
    }

    fn device_paths(&self) -> Vec<PathBuf> {
        self.registry.device_paths()
    }
}

fn events_from_poll(outcome: PollOutcome) -> Result<Vec<InputSourceEvent>> {
    let mut events = Vec::new();

    if outcome.key_events > 0 {
        events.push(InputSourceEvent::KeyStateChanged {
            key_events: outcome.key_events,
        });
    }
    if outcome.devices_changed {
        events.push(InputSourceEvent::DeviceSetChanged);
    }
    if outcome.backpressure {
        events.push(InputSourceEvent::SourceBackpressure {
            key_events: outcome.key_events,
        });
    }

    Ok(events)
}

impl UdevCoalescer {
    fn mark_dirty_event(&mut self, event: &tokio_udev::Event) -> UdevEventStats {
        let Some(path) = event.device().devnode().map(Path::to_path_buf) else {
            return UdevEventStats::default();
        };
        if !is_input_event_node(&path) {
            return UdevEventStats::default();
        }
        if !matches!(
            event.event_type(),
            EventType::Add | EventType::Remove | EventType::Change
        ) {
            return UdevEventStats::default();
        }

        let now = Instant::now();
        let mut stats = UdevEventStats {
            seen: 1,
            ..Default::default()
        };
        if self.dirty_since.is_some() {
            stats.coalesced = 1;
        } else {
            self.dirty_since = Some(now);
        }
        self.last_event_at = Some(now);
        stats
    }

    fn refresh_due(&mut self) -> bool {
        if self.fallback_rescan {
            let now = Instant::now();
            let due = self
                .last_fallback_rescan
                .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(1));
            if due {
                self.last_fallback_rescan = Some(now);
                return true;
            }
        }

        let Some(dirty_since) = self.dirty_since else {
            return false;
        };
        let now = Instant::now();
        let quiet_for = self.last_event_at.map(|at| now.duration_since(at));
        now.duration_since(dirty_since) >= UDEV_MAX_WAIT
            || quiet_for.is_some_and(|elapsed| elapsed >= UDEV_DEBOUNCE)
    }

    fn clear_dirty(&mut self) {
        self.dirty_since = None;
        self.last_event_at = None;
    }

    fn has_pending_refresh(&self) -> bool {
        self.dirty_since.is_some()
    }
}

fn create_udev_stream() -> Result<AsyncMonitorSocket> {
    let monitor = MonitorBuilder::new()?
        // Keep the kernel filter at subsystem=input. Some keyboards do not
        // consistently expose a devtype/tag that is safe to require here.
        .match_subsystem("input")?
        .listen()
        .context("failed to listen for input udev events")?;
    AsyncMonitorSocket::new(monitor).context("failed to create async udev monitor socket")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_outcome_reports_backpressure_as_source_event() {
        let events = events_from_poll(PollOutcome {
            key_events: 256,
            devices_changed: false,
            backpressure: true,
        })
        .unwrap();

        assert_eq!(
            events,
            vec![
                InputSourceEvent::KeyStateChanged { key_events: 256 },
                InputSourceEvent::SourceBackpressure { key_events: 256 }
            ]
        );
    }

    #[test]
    fn device_set_change_is_semantic_source_event() {
        let events = events_from_poll(PollOutcome {
            key_events: 0,
            devices_changed: true,
            backpressure: false,
        })
        .unwrap();

        assert_eq!(events, vec![InputSourceEvent::DeviceSetChanged]);
    }
}
