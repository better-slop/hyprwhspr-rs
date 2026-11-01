//! Selection-consuming targets exercise the public delivery interface. Kitty's
//! Shift+Insert action reads PRIMARY, so a fresh regular clipboard alone cannot
//! make that shortcut insert the transcript.

use super::*;
use tokio::sync::oneshot;

#[derive(Default)]
struct Desktop {
    regular: String,
    primary: String,
    inserted: Vec<(Insertion, String)>,
    publications: Vec<ClipboardTargets>,
    confirmed: Vec<ClipboardSelection>,
}

type PrimaryGate = (oneshot::Sender<()>, oneshot::Receiver<()>);

struct SelectionEffects {
    desktop: Arc<Mutex<Desktop>>,
    hyprland: bool,
    class: &'static str,
    primary_gate: Option<PrimaryGate>,
}

impl DeliveryEffects for SelectionEffects {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            hyprland: self.hyprland,
            wayland: true,
        }
    }

    fn focus(&mut self) -> EffectFuture<'_, Option<Focus>> {
        Box::pin(async {
            Ok(Some(Focus {
                address: "target".into(),
                class: self.class.into(),
            }))
        })
    }

    fn prepare(&mut self, _: Insertion) -> Result<()> {
        Ok(())
    }
    fn settle_focus(&mut self) -> EffectFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn validate_focus<'a>(&'a mut self, _: Option<&'a Focus>) -> EffectFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn publish_clipboard<'a>(
        &'a mut self,
        text: &'a str,
        targets: ClipboardTargets,
    ) -> EffectFuture<'a, ()> {
        let gate = self.primary_gate.take();
        Box::pin(async move {
            {
                let mut desktop = self.desktop.lock().unwrap();
                desktop.publications.push(targets);
                desktop.regular = text.into();
                desktop.confirmed.push(ClipboardSelection::Regular);
            }
            if targets == ClipboardTargets::Both {
                if let Some((started, visible)) = gate {
                    let _ = started.send(());
                    visible.await.context("PRIMARY visibility unconfirmed")?;
                }
                let mut desktop = self.desktop.lock().unwrap();
                desktop.primary = text.into();
                desktop.confirmed.push(ClipboardSelection::Primary);
            }
            Ok(())
        })
    }

    fn insert<'a>(&'a mut self, insertion: Insertion, text: &'a str) -> EffectFuture<'a, ()> {
        Box::pin(async move {
            let mut desktop = self.desktop.lock().unwrap();
            let inserted = match insertion {
                Insertion::Type => text.into(),
                Insertion::Paste(
                    PasteRoute::Hyprland(PasteShortcut::ShiftInsert)
                    | PasteRoute::VirtualKeyboard(PasteShortcut::ShiftInsert),
                ) => {
                    assert_eq!(
                        desktop.confirmed,
                        [ClipboardSelection::Regular, ClipboardSelection::Primary]
                    );
                    desktop.primary.clone()
                }
                Insertion::Paste(_) => desktop.regular.clone(),
            };
            desktop.inserted.push((insertion, inserted));
            Ok(())
        })
    }

    fn cleanup(&mut self, _: Insertion) -> Result<()> {
        Ok(())
    }
}

fn desktop_delivery(
    config: DeliveryConfig,
    hyprland: bool,
    class: &'static str,
    primary_gate: Option<PrimaryGate>,
) -> (TextInjector, Arc<Mutex<Desktop>>) {
    let desktop = Arc::new(Mutex::new(Desktop {
        regular: "old clipboard".into(),
        primary: "stale primary".into(),
        ..Default::default()
    }));
    let delivery = TextInjector::with_effects(
        config,
        Box::new(SelectionEffects {
            desktop: desktop.clone(),
            hyprland,
            class,
            primary_gate,
        }),
    );
    (delivery, desktop)
}

#[tokio::test]
async fn shift_insert_replaces_stale_primary_before_dispatch() {
    for hyprland in [false, true] {
        for global_paste_shortcut in [false, true] {
            let (mut delivery, desktop) = desktop_delivery(
                DeliveryConfig {
                    global_paste_shortcut,
                    shift_insert_classes: if global_paste_shortcut {
                        Vec::new()
                    } else {
                        vec!["kitty".into()]
                    },
                    ..Default::default()
                },
                hyprland,
                "kitty",
                None,
            );
            delivery
                .deliver("new transcript", DeliveryIntent::Paste)
                .await
                .unwrap();
            let desktop = desktop.lock().unwrap();
            let route = if hyprland {
                PasteRoute::Hyprland(PasteShortcut::ShiftInsert)
            } else {
                PasteRoute::VirtualKeyboard(PasteShortcut::ShiftInsert)
            };
            assert_eq!(
                desktop.inserted,
                [(Insertion::Paste(route), "new transcript".into())]
            );
            assert_eq!(desktop.publications, [ClipboardTargets::Both]);
        }
    }
}

#[tokio::test]
async fn copy_and_regular_paste_preserve_primary_while_typing_preserves_both() {
    for (intent, shift_paste) in [
        (DeliveryIntent::Copy, false),
        (DeliveryIntent::Paste, false),
        (DeliveryIntent::Paste, true),
        (DeliveryIntent::Type, false),
    ] {
        let (mut delivery, desktop) = desktop_delivery(
            DeliveryConfig {
                shift_paste,
                global_paste_shortcut: matches!(
                    intent,
                    DeliveryIntent::Copy | DeliveryIntent::Type
                ),
                ..Default::default()
            },
            true,
            "browser",
            None,
        );
        delivery.deliver("new transcript", intent).await.unwrap();
        let desktop = desktop.lock().unwrap();
        assert_eq!(desktop.primary, "stale primary");
        if intent == DeliveryIntent::Type {
            assert_eq!(desktop.regular, "old clipboard");
            assert!(desktop.publications.is_empty());
            assert!(desktop.confirmed.is_empty());
        } else {
            assert_eq!(desktop.regular, "new transcript");
            assert_eq!(desktop.publications, [ClipboardTargets::Regular]);
            assert_eq!(desktop.confirmed, [ClipboardSelection::Regular]);
        }
        if intent == DeliveryIntent::Copy {
            assert!(desktop.inserted.is_empty());
        } else {
            let insertion = if intent == DeliveryIntent::Type {
                Insertion::Type
            } else {
                Insertion::Paste(PasteRoute::Hyprland(if shift_paste {
                    PasteShortcut::CtrlShiftV
                } else {
                    PasteShortcut::CtrlV
                }))
            };
            assert_eq!(desktop.inserted, [(insertion, "new transcript".into())]);
        }
    }
}

#[tokio::test]
async fn regular_visibility_alone_cannot_allow_shift_insert_dispatch() {
    for confirm_primary in [false, true] {
        let (started_tx, started_rx) = oneshot::channel();
        let (visible_tx, visible_rx) = oneshot::channel();
        let (mut delivery, desktop) = desktop_delivery(
            DeliveryConfig {
                global_paste_shortcut: true,
                ..Default::default()
            },
            false,
            "kitty",
            Some((started_tx, visible_rx)),
        );
        let task = tokio::spawn(async move {
            delivery
                .deliver("new transcript", DeliveryIntent::Paste)
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), started_rx)
            .await
            .unwrap()
            .unwrap();
        {
            let desktop = desktop.lock().unwrap();
            assert_eq!(desktop.regular, "new transcript");
            assert_eq!(desktop.primary, "stale primary");
            assert!(desktop.inserted.is_empty());
        }
        assert!(!task.is_finished());
        if confirm_primary {
            visible_tx.send(()).unwrap();
        } else {
            drop(visible_tx);
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.is_ok(), confirm_primary);
        let desktop = desktop.lock().unwrap();
        assert_eq!(desktop.publications, [ClipboardTargets::Both]);
        assert_eq!(desktop.inserted.len(), usize::from(confirm_primary));
    }
}
