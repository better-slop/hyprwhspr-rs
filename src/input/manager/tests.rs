use super::*;
use crate::input::shortcuts::ShortcutKind;
use evdev::KeyCode;
use std::collections::HashSet;

struct TestInputSource {
    pressed_keys: HashSet<KeyCode>,
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
    fn pressed_keys(&self) -> &HashSet<KeyCode> {
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
            .await
    );
    let start = event_rx.recv().await.unwrap();
    assert_eq!(start.kind, ShortcutKind::Hold);
    assert_eq!(start.phase, ShortcutPhase::Start);

    manager.source.clear();
    assert!(
        manager
            .dispatch_source_events(vec![InputSourceEvent::KeyStateChanged { key_events: 1 }])
            .await
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

    manager
        .dispatch_source_events(vec![InputSourceEvent::KeyStateChanged { key_events: 2 }])
        .await;
    let _ = event_rx.recv().await.unwrap();

    manager.source.clear();
    assert!(
        manager
            .dispatch_source_events(vec![InputSourceEvent::DeviceSetChanged])
            .await
    );

    let end = event_rx.recv().await.unwrap();
    assert_eq!(end.kind, ShortcutKind::Hold);
    assert_eq!(end.phase, ShortcutPhase::Cancel);
}

#[tokio::test]
async fn source_backpressure_batch_reconciles_shortcuts_from_source_state() {
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let mut source = TestInputSource::new();
    source.press("SUPER+ALT");
    let mut manager = test_manager(source, event_tx);

    manager
        .dispatch_source_events(vec![InputSourceEvent::KeyStateChanged { key_events: 2 }])
        .await;
    let start = event_rx.recv().await.unwrap();
    assert_eq!(start.phase, ShortcutPhase::Start);

    manager.source.clear();
    assert!(
        manager
            .dispatch_source_events(vec![
                InputSourceEvent::KeyStateChanged { key_events: 0 },
                InputSourceEvent::SourceBackpressure {
                    drained_events: MAX_INPUT_EVENTS_PER_TICK,
                    key_events: 0,
                },
            ])
            .await
    );
    let end = event_rx.recv().await.unwrap();
    assert_eq!(end.phase, ShortcutPhase::End);
}
