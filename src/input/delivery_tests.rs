use super::*;
use crate::input::paste_route::PasteShortcut;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    Focus,
    Prepare(Insertion),
    Settle,
    Validate,
    Copy(ClipboardTargets, String),
    Insert(Insertion, String),
    Cleanup,
}

struct ControlledEffects {
    events: Arc<Mutex<Vec<Event>>>,
    capabilities: Capabilities,
    class: String,
    fail: Option<&'static str>,
    validations: usize,
    copy_gate: Option<(
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    )>,
}

impl ControlledEffects {
    fn record(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }
    fn check(&self, stage: &str) -> Result<()> {
        if self.fail == Some(stage) {
            anyhow::bail!("controlled {stage} failure");
        }
        Ok(())
    }
}

impl DeliveryEffects for ControlledEffects {
    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }
    fn focus(&mut self) -> EffectFuture<'_, Option<Focus>> {
        self.record(Event::Focus);
        Box::pin(async {
            self.check("focus")?;
            Ok(Some(Focus {
                address: "0x123".into(),
                class: self.class.clone(),
            }))
        })
    }
    fn prepare(&mut self, insertion: Insertion) -> Result<()> {
        self.record(Event::Prepare(insertion));
        self.check("prepare")
    }
    fn settle_focus(&mut self) -> EffectFuture<'_, ()> {
        self.record(Event::Settle);
        Box::pin(async { self.check("settle") })
    }
    fn validate_focus<'a>(&'a mut self, _: Option<&'a Focus>) -> EffectFuture<'a, ()> {
        self.record(Event::Validate);
        self.validations += 1;
        Box::pin(async {
            self.check(if self.validations == 1 {
                "focus-before-copy"
            } else {
                "focus-after-copy"
            })
        })
    }
    fn publish_clipboard<'a>(
        &'a mut self,
        text: &'a str,
        targets: ClipboardTargets,
    ) -> EffectFuture<'a, ()> {
        self.record(Event::Copy(targets, text.into()));
        let gate = self.copy_gate.take();
        Box::pin(async move {
            if let Some((started, visible)) = gate {
                let _ = started.send(());
                visible
                    .await
                    .context("controlled clipboard readiness deadline exceeded")?;
            }
            self.check("copy")
        })
    }
    fn insert<'a>(&'a mut self, insertion: Insertion, text: &'a str) -> EffectFuture<'a, ()> {
        self.record(Event::Insert(insertion, text.into()));
        Box::pin(async { self.check("insert") })
    }
    fn cleanup(&mut self, _: Insertion) -> Result<()> {
        self.record(Event::Cleanup);
        self.check("cleanup")
    }
}

fn controlled(
    config: DeliveryConfig,
    fail: Option<&'static str>,
) -> (TextInjector, Arc<Mutex<Vec<Event>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let effects = ControlledEffects {
        events: events.clone(),
        capabilities: Capabilities {
            hyprland: true,
            wayland: true,
        },
        class: "browser".into(),
        fail,
        validations: 0,
        copy_gate: None,
    };
    (
        TextInjector::with_effects(config, Box::new(effects)),
        events,
    )
}

#[tokio::test]
async fn direct_typing_preserves_text_and_never_accesses_clipboard() {
    let (mut delivery, events) = controlled(
        DeliveryConfig {
            global_paste_shortcut: true,
            ..Default::default()
        },
        None,
    );
    let text = "Hello, café — 世界!\nNext\tline";
    delivery.deliver(text, DeliveryIntent::Type).await.unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        [
            Event::Focus,
            Event::Prepare(Insertion::Type),
            Event::Settle,
            Event::Validate,
            Event::Insert(Insertion::Type, text.into()),
            Event::Cleanup,
        ]
    );
}

#[tokio::test]
async fn copy_publishes_once_without_focus_or_keyboard_effects() {
    let (mut delivery, events) = controlled(DeliveryConfig::default(), None);
    delivery
        .deliver("history\n\t世界", DeliveryIntent::Copy)
        .await
        .unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        [Event::Copy(
            ClipboardTargets::Regular,
            "history\n\t世界".into()
        )]
    );
}

#[tokio::test]
async fn explicit_empty_copy_publishes_empty_text_instead_of_acknowledging_old_contents() {
    let (mut delivery, events) = controlled(DeliveryConfig::default(), None);
    delivery.deliver("", DeliveryIntent::Copy).await.unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        [Event::Copy(ClipboardTargets::Regular, String::new())]
    );
}

#[tokio::test]
async fn paste_prepares_capability_before_touching_clipboard_and_inserts_once() {
    let (mut delivery, events) = controlled(DeliveryConfig::default(), None);
    delivery
        .deliver("hello", DeliveryIntent::Paste)
        .await
        .unwrap();
    let insertion = Insertion::Paste(PasteRoute::Hyprland(PasteShortcut::CtrlV));
    assert_eq!(
        *events.lock().unwrap(),
        [
            Event::Focus,
            Event::Prepare(insertion),
            Event::Settle,
            Event::Validate,
            Event::Copy(ClipboardTargets::Regular, "hello".into()),
            Event::Validate,
            Event::Insert(insertion, "hello".into()),
            Event::Cleanup,
        ]
    );
}

#[tokio::test]
async fn unavailable_capability_is_an_error_before_clipboard_mutation() {
    let (mut delivery, events) = controlled(DeliveryConfig::default(), Some("prepare"));
    assert!(delivery
        .deliver("hello", DeliveryIntent::Paste)
        .await
        .is_err());
    assert_eq!(events.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn unsupported_paste_has_no_side_effects() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let effects = ControlledEffects {
        events: events.clone(),
        capabilities: Capabilities::default(),
        class: "browser".into(),
        fail: None,
        validations: 0,
        copy_gate: None,
    };
    let mut delivery = TextInjector::with_effects(DeliveryConfig::default(), Box::new(effects));
    let error = delivery
        .deliver("hello", DeliveryIntent::Paste)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Paste requires"));
    assert_eq!(*events.lock().unwrap(), [Event::Focus]);
}

#[tokio::test]
async fn insertion_failure_is_reported_with_no_retry_or_method_switch() {
    for intent in [DeliveryIntent::Type, DeliveryIntent::Paste] {
        let (mut delivery, events) = controlled(DeliveryConfig::default(), Some("insert"));
        let error = delivery.deliver("partial?", intent).await.unwrap_err();
        assert!(error
            .to_string()
            .contains("insertion may be partial or complete"));
        let events = events.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::Insert(..)))
                .count(),
            1
        );
        assert_eq!(events.last(), Some(&Event::Cleanup));
        if intent == DeliveryIntent::Type {
            assert!(!events.iter().any(|event| matches!(event, Event::Copy(..))));
        }
    }
}

#[tokio::test]
async fn pre_dispatch_failure_never_inserts_and_always_cleans_up() {
    for failure in ["settle", "focus-before-copy", "copy", "focus-after-copy"] {
        let (mut delivery, events) = controlled(DeliveryConfig::default(), Some(failure));
        assert!(delivery
            .deliver("hello", DeliveryIntent::Paste)
            .await
            .is_err());
        let events = events.lock().unwrap();
        assert!(!events
            .iter()
            .any(|event| matches!(event, Event::Insert(..))));
        assert_eq!(events.last(), Some(&Event::Cleanup));
    }
}

#[tokio::test]
async fn cleanup_failure_is_not_success_and_does_not_replay_insertion() {
    let (mut delivery, events) = controlled(DeliveryConfig::default(), Some("cleanup"));
    assert!(delivery
        .deliver("hello", DeliveryIntent::Type)
        .await
        .unwrap_err()
        .to_string()
        .contains("do not retry"));
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, Event::Insert(..)))
            .count(),
        1
    );
}

#[tokio::test]
async fn empty_is_noop_but_whitespace_is_preserved() {
    let (mut delivery, events) = controlled(DeliveryConfig::default(), None);
    delivery.deliver("", DeliveryIntent::Type).await.unwrap();
    assert!(events.lock().unwrap().is_empty());
    delivery
        .deliver(" \n\t", DeliveryIntent::Type)
        .await
        .unwrap();
    assert!(events
        .lock()
        .unwrap()
        .contains(&Event::Insert(Insertion::Type, " \n\t".into())));
}

#[tokio::test]
async fn normalized_application_hint_selects_exact_shortcut() {
    let (mut delivery, events) = controlled(
        DeliveryConfig {
            shift_insert_classes: vec![" BROWSER ".into()],
            ..Default::default()
        },
        None,
    );
    delivery
        .deliver("hello", DeliveryIntent::Paste)
        .await
        .unwrap();
    assert!(events
        .lock()
        .unwrap()
        .contains(&Event::Prepare(Insertion::Paste(PasteRoute::Hyprland(
            PasteShortcut::ShiftInsert
        )))));
}

#[tokio::test]
async fn copy_and_paste_wait_for_visibility_and_do_not_insert_after_readiness_failure() {
    for (intent, confirm) in [
        (DeliveryIntent::Copy, false),
        (DeliveryIntent::Copy, true),
        (DeliveryIntent::Paste, false),
        (DeliveryIntent::Paste, true),
    ] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (visible_tx, visible_rx) = tokio::sync::oneshot::channel();
        let effects = ControlledEffects {
            events: events.clone(),
            capabilities: Capabilities {
                hyprland: true,
                wayland: true,
            },
            class: "browser".into(),
            fail: None,
            validations: 0,
            copy_gate: Some((started_tx, visible_rx)),
        };
        let mut delivery = TextInjector::with_effects(DeliveryConfig::default(), Box::new(effects));
        let task = tokio::spawn(async move { delivery.deliver("hello", intent).await });
        started_rx.await.unwrap();
        assert!(!task.is_finished());
        assert!(!events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, Event::Insert(..))));
        if confirm {
            visible_tx.send(()).unwrap();
        } else {
            drop(visible_tx);
        }
        assert_eq!(task.await.unwrap().is_ok(), confirm);
        assert_eq!(
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| matches!(event, Event::Insert(..)))
                .count(),
            usize::from(confirm && intent == DeliveryIntent::Paste)
        );
    }
}

#[path = "delivery_selection_tests.rs"]
mod selection;
