use anyhow::{Context, Result};
use futures_util::StreamExt;
use std::future;
use std::io;
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio::time;
use tokio_udev::{AsyncMonitorSocket, EventType, MonitorBuilder};
use tracing::{debug, error, info, warn};

use crate::config::ShortcutsConfig;
use crate::input::registry::{is_input_event_node, KeyboardRegistry};
use crate::input::shortcuts::{ShortcutController, ShortcutEvent, ShortcutPhase, ShortcutSummary};

const EVDEV_TICK: Duration = Duration::from_millis(10);
const UDEV_DEBOUNCE: Duration = Duration::from_millis(150);
const UDEV_MAX_WAIT: Duration = Duration::from_secs(1);
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

struct InputManagerRuntime {
    command_rx: mpsc::UnboundedReceiver<InputManagerCommand>,
    event_tx: mpsc::Sender<ShortcutEvent>,
    registry: KeyboardRegistry,
    shortcuts: ShortcutController,
    coalescer: UdevCoalescer,
    stats: InputStats,
    app_busy: bool,
    last_busy_log: Instant,
}

#[derive(Default)]
struct UdevCoalescer {
    dirty_since: Option<Instant>,
    last_event_at: Option<Instant>,
    fallback_rescan: bool,
    last_fallback_rescan: Option<Instant>,
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

impl InputManagerRuntime {
    fn new(
        shortcuts: ShortcutsConfig,
        event_tx: mpsc::Sender<ShortcutEvent>,
        command_rx: mpsc::UnboundedReceiver<InputManagerCommand>,
    ) -> Result<Self> {
        let registry = KeyboardRegistry::open_initial()?;
        let shortcuts = ShortcutController::new(shortcuts)?;

        Ok(Self {
            command_rx,
            event_tx,
            registry,
            shortcuts,
            coalescer: UdevCoalescer::default(),
            stats: InputStats::default(),
            app_busy: false,
            last_busy_log: Instant::now(),
        })
    }

    async fn run(mut self) -> Result<()> {
        let mut udev_stream = match create_udev_stream() {
            Ok(stream) => Some(stream),
            Err(err) => {
                warn!(
                    "Input device monitor unavailable ({}); falling back to periodic rescan",
                    err
                );
                self.coalescer.fallback_rescan = true;
                None
            }
        };
        let mut evdev_tick = time::interval(EVDEV_TICK);
        evdev_tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        let mut refresh_tick = time::interval(Duration::from_millis(50));
        refresh_tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

        info!("Input manager started");

        loop {
            let mut busy = false;

            tokio::select! {
                command = self.command_rx.recv() => {
                    match command {
                        Some(InputManagerCommand::UpdateShortcuts(shortcuts)) => {
                            let releases = self.shortcuts.replace_shortcuts(shortcuts, self.registry.pressed_keys())?;
                            for event in releases {
                                self.emit_shortcut(event);
                            }
                            busy = true;
                        }
                        Some(InputManagerCommand::SetAppBusy(app_busy)) => {
                            self.app_busy = app_busy;
                        }
                        Some(InputManagerCommand::Snapshot(reply_tx)) => {
                            let _ = reply_tx.send(self.snapshot());
                        }
                        Some(InputManagerCommand::Shutdown) | None => {
                            info!("Input manager shutting down");
                            break;
                        }
                    }
                }
                event = next_udev_event(&mut udev_stream), if udev_stream.is_some() && !self.coalescer.has_pending_refresh() => {
                    match event {
                        Some(Ok(event)) => {
                            if self.coalescer.mark_dirty_event(&event, &mut self.stats) {
                                busy = true;
                            }
                        }
                        Some(Err(err)) => {
                            warn!("Input device monitor unavailable ({}); falling back to periodic rescan", err);
                            self.coalescer.fallback_rescan = true;
                            udev_stream = None;
                        }
                        None => {
                            warn!("Input device monitor closed; falling back to periodic rescan");
                            self.coalescer.fallback_rescan = true;
                            udev_stream = None;
                        }
                    }
                }
                _ = refresh_tick.tick() => {
                    if self.coalescer.refresh_due() {
                        self.refresh_devices()?;
                        busy = true;
                    }
                }
                _ = evdev_tick.tick() => {
                    let key_events = self.registry.poll_key_events(MAX_KEY_EVENTS_PER_TICK)?;
                    if key_events > 0 {
                        self.stats.key_events_seen += key_events as u64;
                        let now = Instant::now();
                        for event in self.shortcuts.apply_key_state(self.registry.pressed_keys(), now) {
                            self.emit_shortcut(event);
                        }
                        busy = true;
                    }
                }
            }

            if busy {
                self.stats.loop_busy_ticks += 1;
                self.log_if_busy();
            }
        }

        Ok(())
    }

    fn refresh_devices(&mut self) -> Result<()> {
        let changed = self.registry.refresh()?;
        self.stats.device_refreshes += 1;
        self.coalescer.clear_dirty();

        if changed {
            if let Some(event) = self.shortcuts.on_device_set_changed() {
                self.emit_shortcut(event);
            }
        }

        Ok(())
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
            device_count: self.registry.device_count(),
            devices: self.registry.device_paths(),
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

impl UdevCoalescer {
    fn mark_dirty_event(&mut self, event: &tokio_udev::Event, stats: &mut InputStats) -> bool {
        let Some(path) = event.device().devnode().map(Path::to_path_buf) else {
            return false;
        };
        if !is_input_event_node(&path) {
            return false;
        }
        if !matches!(
            event.event_type(),
            EventType::Add | EventType::Remove | EventType::Change
        ) {
            return false;
        }

        stats.udev_events_seen += 1;
        let now = Instant::now();
        if self.dirty_since.is_some() {
            stats.udev_events_coalesced += 1;
        } else {
            self.dirty_since = Some(now);
        }
        self.last_event_at = Some(now);
        true
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

async fn next_udev_event(
    stream: &mut Option<AsyncMonitorSocket>,
) -> Option<Result<tokio_udev::Event, io::Error>> {
    match stream {
        Some(stream) => stream.next().await,
        None => future::pending().await,
    }
}
