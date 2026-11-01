//! Keyboard activation lifecycle, independent of the native event mechanism.

use anyhow::{Context, Result};
use std::thread::{self, JoinHandle};
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

use super::activation::{
    ActivationHealth, ActivationSource, ShortcutEvent, ShortcutPhase, ShortcutSummary,
};
use super::linux::LinuxActivationSource;
use crate::config::ShortcutsConfig;

#[derive(Debug, Clone, Default)]
pub struct InputStats {
    pub shortcut_events_emitted: u64,
    pub loop_busy_ticks: u64,
}

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub struct InputSnapshot {
    pub backend: &'static str,
    pub health: ActivationHealth,
    pub shortcuts: Vec<ShortcutSummary>,
    pub app_busy: bool,
    pub stats: InputStats,
}

impl InputSnapshot {
    fn starting() -> Self {
        Self {
            backend: "initializing",
            health: ActivationHealth::Starting,
            shortcuts: Vec::new(),
            app_busy: false,
            stats: InputStats::default(),
        }
    }
}

pub struct InputManagerHandle {
    command_tx: mpsc::UnboundedSender<InputManagerCommand>,
    event_rx: mpsc::Receiver<ShortcutEvent>,
    snapshot_rx: watch::Receiver<InputSnapshot>,
    handle: Option<JoinHandle<()>>,
}

enum InputManagerCommand {
    UpdateShortcuts(ShortcutsConfig),
    SetAppBusy(bool),
    Shutdown,
}

struct InputManagerRuntime<S> {
    command_rx: mpsc::UnboundedReceiver<InputManagerCommand>,
    event_tx: mpsc::Sender<ShortcutEvent>,
    snapshot_tx: watch::Sender<InputSnapshot>,
    source: S,
    stats: InputStats,
    app_busy: bool,
}

impl InputManagerHandle {
    pub fn spawn(shortcuts: ShortcutsConfig) -> Result<Self> {
        LinuxActivationSource::validate(shortcuts.clone())?;
        Self::spawn_with_source(move || LinuxActivationSource::open(shortcuts), 10)
    }

    fn spawn_with_source<S: ActivationSource + 'static>(
        open: impl FnOnce() -> Result<S> + Send + 'static,
        event_capacity: usize,
    ) -> Result<Self> {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::channel(event_capacity);
        let (snapshot_tx, snapshot_rx) = watch::channel(InputSnapshot::starting());
        let handle = thread::spawn(move || {
            if let Err(err) = run_input_thread(open, event_tx, command_rx, snapshot_tx) {
                error!("Input manager stopped: {:#}", err);
            }
        });
        Ok(Self {
            command_tx,
            event_rx,
            snapshot_rx,
            handle: Some(handle),
        })
    }

    /// Wait for a semantic transition. Cancellation safe in a host select loop.
    pub async fn next_event(&mut self) -> Option<ShortcutEvent> {
        self.event_rx.recv().await
    }

    pub fn update_shortcuts(&self, shortcuts: ShortcutsConfig) -> Result<()> {
        LinuxActivationSource::validate(shortcuts.clone())?;
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

    /// Last health remains available after native failure or shutdown.
    pub fn snapshot(&self) -> InputSnapshot {
        self.snapshot_rx.borrow().clone()
    }

    /// Stop native activation and discard pending delivery before joining.
    /// Session hosts clean up active capture before stopping activation.
    pub fn stop(&mut self) {
        self.event_rx.close();
        while self.event_rx.try_recv().is_ok() {}
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

fn run_input_thread<S: ActivationSource>(
    open: impl FnOnce() -> Result<S>,
    event_tx: mpsc::Sender<ShortcutEvent>,
    command_rx: mpsc::UnboundedReceiver<InputManagerCommand>,
    snapshot_tx: watch::Sender<InputSnapshot>,
) -> Result<()> {
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .context("failed to build input manager runtime");
    let runtime = match result {
        Ok(runtime) => runtime,
        Err(err) => {
            snapshot_tx.send_modify(|snapshot| {
                snapshot.health = ActivationHealth::Failed(format!("{err:#}"))
            });
            return Err(err);
        }
    };
    runtime.block_on(async move {
        let source = match open() {
            Ok(source) => source,
            Err(err) => {
                snapshot_tx.send_modify(|snapshot| {
                    snapshot.health = ActivationHealth::Failed(format!("{err:#}"))
                });
                return Err(err);
            }
        };
        InputManagerRuntime::with_source(event_tx, command_rx, snapshot_tx, source)
            .run()
            .await
    })
}

impl<S: ActivationSource> InputManagerRuntime<S> {
    fn with_source(
        event_tx: mpsc::Sender<ShortcutEvent>,
        command_rx: mpsc::UnboundedReceiver<InputManagerCommand>,
        snapshot_tx: watch::Sender<InputSnapshot>,
        source: S,
    ) -> Self {
        Self {
            command_rx,
            event_tx,
            snapshot_tx,
            source,
            stats: InputStats::default(),
            app_busy: false,
        }
    }

    async fn run(mut self) -> Result<()> {
        info!("Input manager started");
        self.publish_snapshot(None);
        loop {
            let result = tokio::select! {
                command = self.command_rx.recv() => {
                    match command {
                        Some(InputManagerCommand::UpdateShortcuts(shortcuts)) => self.source.configure(shortcuts),
                        Some(InputManagerCommand::SetAppBusy(busy)) => {
                            self.app_busy = busy;
                            Ok(Vec::new())
                        }
                        Some(InputManagerCommand::Shutdown) | None => {
                            let events = self.source.cancel();
                            self.emit_events(events).await;
                            self.publish_snapshot(Some(ActivationHealth::Stopped));
                            info!("Input manager shutting down");
                            return Ok(());
                        }
                    }
                }
                result = self.source.next() => result,
            };
            match result {
                Ok(events) => {
                    if !events.is_empty() {
                        self.stats.loop_busy_ticks += 1;
                    }
                    self.emit_events(events).await;
                    self.publish_snapshot(None);
                }
                Err(err) => {
                    let events = self.source.cancel();
                    self.emit_events(events).await;
                    self.publish_snapshot(Some(ActivationHealth::Failed(format!("{err:#}"))));
                    return Err(err);
                }
            }
        }
    }

    async fn emit_events(&mut self, events: Vec<ShortcutEvent>) {
        for event in events {
            let result = if matches!(event.phase, ShortcutPhase::End | ShortcutPhase::Cancel) {
                // Terminal events may stop an active recording. Backpressure must
                // delay delivery rather than silently leave that recording active.
                self.event_tx
                    .send(event)
                    .await
                    .map_err(|err| err.to_string())
            } else {
                self.event_tx.try_send(event).map_err(|err| err.to_string())
            };
            match result {
                Ok(()) => self.stats.shortcut_events_emitted += 1,
                Err(err) if self.event_tx.is_closed() => debug!("App receiver closed: {}", err),
                Err(err) => warn!("Failed to send shortcut start event: {}", err),
            }
        }
    }

    fn publish_snapshot(&self, health: Option<ActivationHealth>) {
        let source = self.source.snapshot();
        self.snapshot_tx.send_replace(InputSnapshot {
            backend: source.backend,
            health: health.unwrap_or(source.health),
            shortcuts: source.shortcuts,
            app_busy: self.app_busy,
            stats: self.stats.clone(),
        });
    }
}
