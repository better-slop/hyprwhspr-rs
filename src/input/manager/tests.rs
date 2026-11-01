//! Controlled adapter drives the production loop without evdev, timers, or
//! devices. Acknowledgements establish that native transitions reached the
//! manager while its bounded app queue is full.

use super::*;
use crate::input::activation::{ShortcutKind, SourceSnapshot};
use std::future::Future;
use std::io;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

#[derive(Clone, Copy)]
enum NativeAction {
    Begin,
    End,
    Detach,
    PermissionDenied,
}

struct ControlledMessage {
    action: NativeAction,
    applied: oneshot::Sender<()>,
}

struct ControlledSource {
    signals: mpsc::UnboundedReceiver<ControlledMessage>,
    hold: Option<String>,
    held: bool,
}

fn event(phase: ShortcutPhase) -> ShortcutEvent {
    ShortcutEvent {
        kind: ShortcutKind::Hold,
        phase,
        triggered_at: Instant::now(),
    }
}

fn permission_denied() -> anyhow::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "activation permission revoked",
    )
    .into()
}

impl ActivationSource for ControlledSource {
    async fn next(&mut self) -> Result<Vec<ShortcutEvent>> {
        let message = self.signals.recv().await.expect("test owns signal sender");
        let result = match message.action {
            NativeAction::Begin => {
                self.held = true;
                Ok(vec![event(ShortcutPhase::Start)])
            }
            NativeAction::End => {
                self.held = false;
                Ok(vec![event(ShortcutPhase::End)])
            }
            NativeAction::Detach => {
                self.held = false;
                Ok(vec![event(ShortcutPhase::Cancel)])
            }
            NativeAction::PermissionDenied => Err(permission_denied()),
        };
        let _ = message.applied.send(());
        result
    }

    fn configure(&mut self, shortcuts: ShortcutsConfig) -> Result<Vec<ShortcutEvent>> {
        if shortcuts.hold.as_deref() == Some("DENIED") {
            return Err(permission_denied());
        }
        let release = self.held && self.hold != shortcuts.hold;
        self.hold = shortcuts.hold;
        if release {
            self.held = false;
        }
        Ok(if release {
            vec![event(ShortcutPhase::End)]
        } else {
            Vec::new()
        })
    }

    fn cancel(&mut self) -> Vec<ShortcutEvent> {
        if std::mem::take(&mut self.held) {
            vec![event(ShortcutPhase::Cancel)]
        } else {
            Vec::new()
        }
    }

    fn snapshot(&self) -> SourceSnapshot {
        SourceSnapshot {
            backend: "controlled",
            health: ActivationHealth::Ready,
            shortcuts: self
                .hold
                .iter()
                .map(|name| ShortcutSummary {
                    name: name.clone(),
                    kind: ShortcutKind::Hold,
                    active: self.held,
                })
                .collect(),
        }
    }
}

struct Harness {
    commands: mpsc::UnboundedSender<InputManagerCommand>,
    signals: mpsc::UnboundedSender<ControlledMessage>,
    events: mpsc::Receiver<ShortcutEvent>,
    status: watch::Receiver<InputSnapshot>,
    task: tokio::task::JoinHandle<Result<()>>,
}

impl Harness {
    fn start() -> Self {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (signals, signal_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::channel(1);
        let (snapshot_tx, status) = watch::channel(InputSnapshot::starting());
        let source = ControlledSource {
            signals: signal_rx,
            hold: Some("native-hold".into()),
            held: false,
        };
        let manager = InputManagerRuntime::with_source(event_tx, command_rx, snapshot_tx, source);
        let task = tokio::spawn(manager.run());
        Self {
            commands,
            signals,
            events,
            status,
            task,
        }
    }

    async fn signal(&self, action: NativeAction) {
        let (applied, reply) = oneshot::channel();
        self.signals
            .send(ControlledMessage { action, applied })
            .unwrap();
        bounded(reply).await.unwrap();
    }

    async fn phase(&mut self) -> ShortcutPhase {
        bounded(self.events.recv())
            .await
            .expect("activation event")
            .phase
    }

    async fn stop(self) -> InputSnapshot {
        self.commands.send(InputManagerCommand::Shutdown).unwrap();
        bounded(self.task).await.unwrap().unwrap();
        self.status.borrow().clone()
    }
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .expect("activation lifecycle timed out")
}

#[tokio::test]
async fn end_and_detach_cancel_survive_a_full_event_queue() {
    for terminal in [NativeAction::End, NativeAction::Detach] {
        let mut h = Harness::start();
        h.signal(NativeAction::Begin).await;
        // Capacity one now holds Start. The adapter observes the release before
        // the app reads it, forcing the production dispatcher to await capacity.
        h.signal(terminal).await;
        assert_eq!(h.phase().await, ShortcutPhase::Start);
        assert_eq!(
            h.phase().await,
            match terminal {
                NativeAction::End => ShortcutPhase::End,
                _ => ShortcutPhase::Cancel,
            }
        );
        let snapshot = h.stop().await;
        assert_eq!(snapshot.stats.shortcut_events_emitted, 2);
        assert_eq!(snapshot.health, ActivationHealth::Stopped);
    }
}

#[tokio::test]
async fn queued_starts_are_bounded_but_end_is_delivered() {
    let mut h = Harness::start();
    h.signal(NativeAction::Begin).await;
    h.signal(NativeAction::Begin).await;
    h.signal(NativeAction::End).await;
    assert_eq!(h.phase().await, ShortcutPhase::Start);
    assert_eq!(h.phase().await, ShortcutPhase::End);
    assert_eq!(h.stop().await.stats.shortcut_events_emitted, 2);
}

#[tokio::test]
async fn shutdown_cancels_active_hold() {
    let mut h = Harness::start();
    h.signal(NativeAction::Begin).await;
    h.commands.send(InputManagerCommand::Shutdown).unwrap();
    tokio::task::yield_now().await;
    assert_eq!(h.phase().await, ShortcutPhase::Start);
    assert_eq!(h.phase().await, ShortcutPhase::Cancel);
    bounded(h.task).await.unwrap().unwrap();
    assert_eq!(h.status.borrow().health, ActivationHealth::Stopped);
}

#[tokio::test]
async fn command_disconnect_cancels_active_hold() {
    let h = Harness::start();
    h.signal(NativeAction::Begin).await;
    let Harness {
        commands,
        mut events,
        status,
        task,
        ..
    } = h;
    drop(commands);
    assert_eq!(
        bounded(events.recv()).await.unwrap().phase,
        ShortcutPhase::Start
    );
    assert_eq!(
        bounded(events.recv()).await.unwrap().phase,
        ShortcutPhase::Cancel
    );
    bounded(task).await.unwrap().unwrap();
    assert_eq!(status.borrow().health, ActivationHealth::Stopped);
}

#[tokio::test]
async fn source_permission_error_cancels_before_reporting_failed_health() {
    let mut h = Harness::start();
    h.signal(NativeAction::Begin).await;
    h.signal(NativeAction::PermissionDenied).await;
    assert_eq!(h.phase().await, ShortcutPhase::Start);
    assert_eq!(h.phase().await, ShortcutPhase::Cancel);
    let err = bounded(h.task).await.unwrap().unwrap_err();
    assert!(err.to_string().contains("permission revoked"));
    assert!(
        matches!(&h.status.borrow().health, ActivationHealth::Failed(message)
        if message.contains("permission revoked"))
    );
    assert!(!h.status.borrow().shortcuts[0].active);
}

#[tokio::test]
async fn config_refresh_releases_hold_and_publishes_new_configuration() {
    let mut h = Harness::start();
    h.signal(NativeAction::Begin).await;
    h.commands
        .send(InputManagerCommand::UpdateShortcuts(ShortcutsConfig {
            press: None,
            hold: Some("different-native-hold".into()),
        }))
        .unwrap();
    assert_eq!(h.phase().await, ShortcutPhase::Start);
    assert_eq!(h.phase().await, ShortcutPhase::End);
    let snapshot = h.stop().await;
    assert_eq!(snapshot.shortcuts[0].name, "different-native-hold");
    assert!(!snapshot.shortcuts[0].active);
}

#[tokio::test]
async fn config_permission_error_cancels_without_replacing_configuration() {
    let mut h = Harness::start();
    h.signal(NativeAction::Begin).await;
    h.commands
        .send(InputManagerCommand::UpdateShortcuts(ShortcutsConfig {
            press: None,
            hold: Some("DENIED".into()),
        }))
        .unwrap();
    assert_eq!(h.phase().await, ShortcutPhase::Start);
    assert_eq!(h.phase().await, ShortcutPhase::Cancel);
    assert!(bounded(h.task).await.unwrap().is_err());
    assert_eq!(h.status.borrow().shortcuts[0].name, "native-hold");
    assert!(matches!(
        h.status.borrow().health,
        ActivationHealth::Failed(_)
    ));
}

#[tokio::test]
async fn closed_app_receiver_does_not_block_shutdown() {
    let h = Harness::start();
    h.signal(NativeAction::Begin).await;
    drop(h.events);
    h.commands.send(InputManagerCommand::Shutdown).unwrap();
    bounded(h.task).await.unwrap().unwrap();
    assert_eq!(h.status.borrow().health, ActivationHealth::Stopped);
}

#[test]
fn public_stop_and_drop_close_full_delivery_before_joining() {
    for drop_manager in [false, true] {
        let (signals, signal_rx) = mpsc::unbounded_channel();
        let mut manager = InputManagerHandle::spawn_with_source(
            move || {
                Ok(ControlledSource {
                    signals: signal_rx,
                    hold: Some("native-hold".into()),
                    held: false,
                })
            },
            1,
        )
        .unwrap();
        let status = manager.snapshot_rx.clone();
        for action in [NativeAction::Begin, NativeAction::End] {
            let (applied, reply) = oneshot::channel();
            signals.send(ControlledMessage { action, applied }).unwrap();
            reply.blocking_recv().unwrap();
        }
        // Start fills delivery; End is now awaiting capacity. Public teardown
        // must own closure, without a host-side receiver drop or drain.
        let (finished, done) = std::sync::mpsc::channel();
        let stopper = std::thread::spawn(move || {
            if drop_manager {
                drop(manager);
            } else {
                manager.stop();
            }
            finished.send(()).unwrap();
        });
        done.recv_timeout(Duration::from_secs(2))
            .expect("public teardown deadlocked behind full delivery queue");
        stopper.join().unwrap();
        assert_eq!(status.borrow().health, ActivationHealth::Stopped);
    }
}
