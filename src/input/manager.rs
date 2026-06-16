use anyhow::{Context, Result};
use std::path::PathBuf;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio::time;
use tracing::{debug, error, info, warn};

use crate::config::ShortcutsConfig;
use crate::input::shortcuts::{
    ShortcutController, ShortcutEvent, ShortcutInput, ShortcutPhase, ShortcutSummary,
};
use crate::input::source::{EvdevUdevEventSource, InputEventSource, InputSourceEvent};

const EVDEV_TICK: Duration = Duration::from_millis(10);
const MAX_KEY_EVENTS_PER_TICK: usize = 256;

#[derive(Debug, Clone, Default)]
pub struct InputStats {
    pub udev_events_seen: u64,
    pub udev_events_coalesced: u64,
    pub device_refreshes: u64,
    pub key_events_seen: u64,
    pub shortcut_events_emitted: u64,
    pub loop_busy_ticks: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::shortcuts::ShortcutKind;
    use evdev::Key;
    use std::collections::HashSet;

    struct TestInputSource {
        pressed_keys: HashSet<Key>,
        devices: Vec<PathBuf>,
    }

    impl TestInputSource {
        fn new() -> Self {
            Self {
                pressed_keys: HashSet::new(),
                devices: vec![PathBuf::from("/dev/input/event-test")],
            }
        }

        fn press(&mut self, shortcut: &str) {
            self.pressed_keys = crate::input::shortcuts::parse_shortcut(shortcut).unwrap();
        }

        fn clear(&mut self) {
            self.pressed_keys.clear();
        }
    }

    impl InputEventSource for TestInputSource {
        fn pressed_keys(&self) -> &HashSet<Key> {
            &self.pressed_keys
        }

        fn device_count(&self) -> usize {
            self.devices.len()
        }

        fn device_paths(&self) -> Vec<PathBuf> {
            self.devices.clone()
        }
    }

    fn shortcuts(press: Option<&str>, hold: Option<&str>) -> ShortcutsConfig {
        ShortcutsConfig {
            press: press.map(str::to_string),
            hold: hold.map(str::to_string),
        }
    }

    fn test_manager(
        source: TestInputSource,
        event_tx: mpsc::Sender<ShortcutEvent>,
    ) -> InputManagerRuntime<TestInputSource> {
        let (_command_tx, command_rx) = mpsc::unbounded_channel();
        InputManagerRuntime::with_source(
            shortcuts(None, Some("SUPER+ALT")),
            event_tx,
            command_rx,
            source,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn source_key_state_starts_and_ends_hold_shortcut() {
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut source = TestInputSource::new();
        source.press("SUPER+ALT");
        let mut manager = test_manager(source, event_tx);

        assert!(
            manager
                .dispatch_source_events(vec![InputSourceEvent::KeyStateChanged { key_events: 2 }])
        );
        let start = event_rx.recv().await.unwrap();
        assert_eq!(start.kind, ShortcutKind::Hold);
        assert_eq!(start.phase, ShortcutPhase::Start);

        manager.source.clear();
        assert!(
            manager
                .dispatch_source_events(vec![InputSourceEvent::KeyStateChanged { key_events: 1 }])
        );
        let end = event_rx.recv().await.unwrap();
        assert_eq!(end.kind, ShortcutKind::Hold);
        assert_eq!(end.phase, ShortcutPhase::End);
    }

    #[tokio::test]
    async fn source_device_change_releases_active_hold_without_key_event() {
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut source = TestInputSource::new();
        source.press("SUPER+ALT");
        let mut manager = test_manager(source, event_tx);

        manager.dispatch_source_events(vec![InputSourceEvent::KeyStateChanged { key_events: 2 }]);
        let _ = event_rx.recv().await.unwrap();

        manager.source.clear();
        assert!(manager.dispatch_source_events(vec![InputSourceEvent::DeviceSetChanged]));

        let end = event_rx.recv().await.unwrap();
        assert_eq!(end.kind, ShortcutKind::Hold);
        assert_eq!(end.phase, ShortcutPhase::End);
    }

    #[test]
    fn source_backpressure_is_busy_but_does_not_reconcile_shortcuts() {
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let source = TestInputSource::new();
        let mut manager = test_manager(source, event_tx);

        assert!(
            manager.dispatch_source_events(vec![InputSourceEvent::SourceBackpressure {
                key_events: MAX_KEY_EVENTS_PER_TICK,
            }])
        );
        assert!(event_rx.try_recv().is_err());
    }
}

#[derive(Debug, Clone)]
pub struct InputSnapshot {
    pub device_count: usize,
    pub devices: Vec<PathBuf>,
    pub shortcuts: Vec<ShortcutSummary>,
    pub app_busy: bool,
    pub stats: InputStats,
}

pub struct InputManagerHandle {
    command_tx: mpsc::UnboundedSender<InputManagerCommand>,
    handle: Option<JoinHandle<()>>,
}

enum InputManagerCommand {
    UpdateShortcuts(ShortcutsConfig),
    SetAppBusy(bool),
    Snapshot(oneshot::Sender<InputSnapshot>),
    Shutdown,
}

struct InputManagerRuntime<S = EvdevUdevEventSource> {
    command_rx: mpsc::UnboundedReceiver<InputManagerCommand>,
    event_tx: mpsc::Sender<ShortcutEvent>,
    source: S,
    shortcuts: ShortcutController,
    stats: InputStats,
    app_busy: bool,
    last_busy_log: Instant,
}

enum ShortcutTransitionCause {
    KeyStateChanged,
    DeviceSetChanged,
}

impl InputManagerHandle {
    pub fn spawn(
        shortcuts: ShortcutsConfig,
        event_tx: mpsc::Sender<ShortcutEvent>,
    ) -> Result<Self> {
        ShortcutController::new(shortcuts.clone())?;
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel(1);

        let handle = thread::spawn(move || {
            let result = run_input_thread(shortcuts, event_tx, command_rx);
            if let Err(err) = result {
                error!("Input manager stopped: {:#}", err);
            }
            let _ = init_tx.send(());
        });

        // Surface fast thread-start failures without forcing the app to block on the manager.
        if let Ok(()) = init_rx.recv_timeout(Duration::from_millis(1)) {
            warn!("Input manager exited during startup");
        }

        Ok(Self {
            command_tx,
            handle: Some(handle),
        })
    }

    pub fn update_shortcuts(&self, shortcuts: ShortcutsConfig) -> Result<()> {
        ShortcutController::new(shortcuts.clone())?;
        self.command_tx
            .send(InputManagerCommand::UpdateShortcuts(shortcuts))
            .context("input manager is not running")
    }

    pub fn set_app_busy(&self, app_busy: bool) {
        if let Err(err) = self
            .command_tx
            .send(InputManagerCommand::SetAppBusy(app_busy))
        {
            debug!("Failed to update input manager app-busy state: {}", err);
        }
    }

    pub async fn snapshot(&self) -> Result<InputSnapshot> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(InputManagerCommand::Snapshot(tx))
            .context("input manager is not running")?;
        rx.await.context("input manager dropped snapshot reply")
    }

    pub fn stop(&mut self) {
        let _ = self.command_tx.send(InputManagerCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            if let Err(err) = handle.join() {
                error!("Input manager thread panicked: {:?}", err);
            }
        }
    }
}

impl Drop for InputManagerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_input_thread(
    shortcuts: ShortcutsConfig,
    event_tx: mpsc::Sender<ShortcutEvent>,
    command_rx: mpsc::UnboundedReceiver<InputManagerCommand>,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .context("failed to build input manager runtime")?;

    runtime.block_on(async move {
        let manager = InputManagerRuntime::new(shortcuts, event_tx, command_rx)?;
        manager.run().await
    })
}

impl InputManagerRuntime<EvdevUdevEventSource> {
    fn new(
        shortcuts: ShortcutsConfig,
        event_tx: mpsc::Sender<ShortcutEvent>,
        command_rx: mpsc::UnboundedReceiver<InputManagerCommand>,
    ) -> Result<Self> {
        let source = EvdevUdevEventSource::open()?;
        Self::with_source(shortcuts, event_tx, command_rx, source)
    }

    async fn run(mut self) -> Result<()> {
        let mut evdev_tick = time::interval(EVDEV_TICK);
        evdev_tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        let mut refresh_tick = time::interval(Duration::from_millis(50));
        refresh_tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

        info!("Input manager started");

        loop {
            let mut busy = false;

            tokio::select! {
                command = self.command_rx.recv() => {
                    let should_shutdown = self.shutting_down(command.as_ref());
                    busy = self.handle_command(command)?;
                    if should_shutdown {
                        break;
                    }
                }
                event = self.source.next_udev_event(), if self.source.udev_events_enabled() => {
                    let stats = self.source.handle_udev_event(event);
                    self.stats.udev_events_seen += stats.seen;
                    self.stats.udev_events_coalesced += stats.coalesced;
                    busy = stats.seen > 0;
                }
                _ = refresh_tick.tick() => {
                    if self.source.refresh_due() {
                        self.stats.device_refreshes += 1;
                        let events = self.source.refresh_devices()?;
                        busy = self.dispatch_source_events(events);
                    }
                }
                _ = evdev_tick.tick() => {
                    let events = self.source.poll_key_events(MAX_KEY_EVENTS_PER_TICK)?;
                    busy = self.dispatch_source_events(events);
                }
            }

            if busy {
                self.stats.loop_busy_ticks += 1;
                self.log_if_busy();
            }
        }

        Ok(())
    }
}

impl<S: InputEventSource> InputManagerRuntime<S> {
    fn with_source(
        shortcuts: ShortcutsConfig,
        event_tx: mpsc::Sender<ShortcutEvent>,
        command_rx: mpsc::UnboundedReceiver<InputManagerCommand>,
        source: S,
    ) -> Result<Self> {
        let shortcuts = ShortcutController::new(shortcuts)?;

        Ok(Self {
            command_rx,
            event_tx,
            source,
            shortcuts,
            stats: InputStats::default(),
            app_busy: false,
            last_busy_log: Instant::now(),
        })
    }

    fn handle_command(&mut self, command: Option<InputManagerCommand>) -> Result<bool> {
        match command {
            Some(InputManagerCommand::UpdateShortcuts(shortcuts)) => {
                let pressed_keys = self.source.pressed_keys().clone();
                for event in self.shortcuts.transition(ShortcutInput::ConfigChanged {
                    shortcuts,
                    pressed_keys: &pressed_keys,
                    now: Instant::now(),
                })? {
                    self.emit_shortcut(event);
                }
                Ok(true)
            }
            Some(InputManagerCommand::SetAppBusy(app_busy)) => {
                self.app_busy = app_busy;
                Ok(false)
            }
            Some(InputManagerCommand::Snapshot(reply_tx)) => {
                let _ = reply_tx.send(self.snapshot());
                Ok(false)
            }
            Some(InputManagerCommand::Shutdown) | None => {
                info!("Input manager shutting down");
                Ok(false)
            }
        }
    }

    fn shutting_down(&self, command: Option<&InputManagerCommand>) -> bool {
        matches!(command, Some(InputManagerCommand::Shutdown) | None)
    }

    fn dispatch_source_events(&mut self, events: Vec<InputSourceEvent>) -> bool {
        let mut busy = false;

        for event in events {
            match event {
                InputSourceEvent::KeyStateChanged { key_events } => {
                    self.stats.key_events_seen += key_events as u64;
                    self.transition_shortcuts(ShortcutTransitionCause::KeyStateChanged);
                    busy = true;
                }
                InputSourceEvent::DeviceSetChanged => {
                    self.transition_shortcuts(ShortcutTransitionCause::DeviceSetChanged);
                    busy = true;
                }
                InputSourceEvent::SourceBackpressure { key_events } => {
                    debug!(
                        key_events,
                        max_key_events_per_tick = MAX_KEY_EVENTS_PER_TICK,
                        "Input source reached per-tick key event cap"
                    );
                    busy = true;
                }
            }
        }

        busy
    }

    fn transition_shortcuts(&mut self, cause: ShortcutTransitionCause) {
        let now = Instant::now();
        let pressed_keys = self.source.pressed_keys().clone();
        let input = match cause {
            ShortcutTransitionCause::KeyStateChanged => ShortcutInput::KeyStateChanged {
                pressed_keys: &pressed_keys,
                now,
            },
            ShortcutTransitionCause::DeviceSetChanged => ShortcutInput::DeviceSetChanged {
                pressed_keys: &pressed_keys,
                now,
            },
        };

        match self.shortcuts.transition(input) {
            Ok(events) => {
                for event in events {
                    self.emit_shortcut(event);
                }
            }
            Err(err) => warn!("Failed to apply shortcut transition: {:#}", err),
        }
    }

    fn emit_shortcut(&mut self, event: ShortcutEvent) {
        let phase = event.phase;
        if let Err(err) = self.event_tx.try_send(event) {
            match phase {
                ShortcutPhase::Start => warn!("Failed to send shortcut start event: {}", err),
                ShortcutPhase::End => {
                    debug!("Dropped shortcut end event under backpressure: {}", err)
                }
            }
            return;
        }
        self.stats.shortcut_events_emitted += 1;
    }

    fn snapshot(&self) -> InputSnapshot {
        InputSnapshot {
            device_count: self.source.device_count(),
            devices: self.source.device_paths(),
            shortcuts: self.shortcuts.summaries(),
            app_busy: self.app_busy,
            stats: self.stats.clone(),
        }
    }

    fn log_if_busy(&mut self) {
        if self.last_busy_log.elapsed() < Duration::from_secs(60) {
            return;
        }

        if !self.app_busy && self.stats.loop_busy_ticks > 1_000 {
            warn!(
                udev_events_seen = self.stats.udev_events_seen,
                udev_events_coalesced = self.stats.udev_events_coalesced,
                device_refreshes = self.stats.device_refreshes,
                key_events_seen = self.stats.key_events_seen,
                shortcut_events_emitted = self.stats.shortcut_events_emitted,
                loop_busy_ticks = self.stats.loop_busy_ticks,
                "Input manager busy while idle"
            );
        }

        self.stats.loop_busy_ticks = 0;
        self.last_busy_log = Instant::now();
    }
}
