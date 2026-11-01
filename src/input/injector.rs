//! Delivery policy: effect order, focus checks, and the no-retry boundary.
//! Native connections and clipboard serving are hidden behind an internal seam.

use anyhow::{anyhow, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};

use super::paste_route::{
    class_paste_hint_for_class, normalize_hint_classes, plan_paste_route, PasteRoute,
    PasteRouteContext,
};
use crate::config::Config;

#[path = "delivery_native.rs"]
mod native;
#[cfg(test)]
#[path = "delivery_tests.rs"]
mod tests;

/// Explicit output intent. Typing never reads or writes the clipboard.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryIntent {
    Copy,
    Type,
    #[default]
    Paste,
}

/// Paste policy shared by production and interactive delivery. Output intent is
/// a per-operation argument, independent of whether the clipboard retains text.
#[derive(Debug, Clone, Default)]
pub struct DeliveryConfig {
    pub shift_paste: bool,
    pub global_paste_shortcut: bool,
    pub shift_classes: Vec<String>,
    pub shift_insert_classes: Vec<String>,
}

impl From<&Config> for DeliveryConfig {
    fn from(config: &Config) -> Self {
        Self {
            shift_paste: config.shift_paste,
            global_paste_shortcut: config.global_paste_shortcut,
            shift_classes: config.paste_hints.shift.clone(),
            shift_insert_classes: config.paste_hints.shift_insert.clone(),
        }
    }
}

/// Owns native delivery resources, including the clipboard serving lifetime.
/// Copy and paste replace the regular clipboard with text and retain it while
/// this process runs. Restoration is unsupported: a text snapshot cannot
/// preserve rich formats or establish ownership of a newer selection.
pub struct TextInjector {
    config: DeliveryConfig,
    effects: Box<dyn DeliveryEffects>,
}

impl TextInjector {
    /// Construction has no desktop effects. Acquire the requested capabilities
    /// during delivery, before any clipboard write or key event.
    pub fn new(config: DeliveryConfig) -> Self {
        Self::with_effects(config, Box::new(native::NativeEffects::new()))
    }

    fn with_effects(mut config: DeliveryConfig, effects: Box<dyn DeliveryEffects>) -> Self {
        config.shift_classes = normalize_hint_classes(config.shift_classes);
        config.shift_insert_classes = normalize_hint_classes(config.shift_insert_classes);
        Self { config, effects }
    }

    /// Deliver already-normalized text. An insertion error means the target may
    /// have received some or all text; callers must not retry that operation.
    /// Preserve Unicode, whitespace, newline and tab characters unchanged.
    pub async fn deliver(&mut self, text: &str, intent: DeliveryIntent) -> Result<()> {
        if intent == DeliveryIntent::Copy {
            return self.effects.publish_clipboard(text).await;
        }
        if text.is_empty() {
            return Ok(());
        }

        let focus = self
            .effects
            .focus()
            .await
            .context("Cannot determine delivery focus")?;
        let insertion = match intent {
            DeliveryIntent::Type => Insertion::Type,
            DeliveryIntent::Paste => {
                let capabilities = self.effects.capabilities();
                let class_hint = focus.as_ref().and_then(|window| {
                    class_paste_hint_for_class(
                        &window.class,
                        &self.config.shift_classes,
                        &self.config.shift_insert_classes,
                    )
                });
                let route = plan_paste_route(PasteRouteContext {
                    global_paste_shortcut: self.config.global_paste_shortcut,
                    default_shift_paste: self.config.shift_paste,
                    class_hint,
                    hyprland_available: capabilities.hyprland,
                    wayland_env: capabilities.wayland,
                })
                .context("Paste requires Hyprland IPC or a Wayland virtual keyboard")?;
                Insertion::Paste(route)
            }
            DeliveryIntent::Copy => unreachable!("copy handled before keyboard preparation"),
        };

        self.effects
            .prepare(insertion)
            .context("Delivery capability unavailable")?;
        // All exits after preparation release native resources. Dispatch once:
        // an IPC/write error does not prove the application received nothing.
        let result = self.perform(text, insertion, focus.as_ref()).await;
        let cleanup = self.effects.cleanup(insertion);
        match (result, cleanup) {
            (Err(error), Err(cleanup)) => {
                Err(error.context(format!("Keyboard cleanup also failed: {cleanup:#}")))
            }
            (Err(error), _) => Err(error),
            (Ok(()), Err(error)) => {
                Err(error.context("Text was dispatched, but keyboard cleanup failed; do not retry"))
            }
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    async fn perform(
        &mut self,
        text: &str,
        insertion: Insertion,
        focus: Option<&Focus>,
    ) -> Result<()> {
        self.effects.settle_focus().await?;
        self.effects.validate_focus(focus).await?;
        if matches!(insertion, Insertion::Paste(_)) {
            self.effects.publish_clipboard(text).await?;
            self.effects.validate_focus(focus).await?;
        }
        self.effects.insert(insertion, text).await.map_err(|error| {
            anyhow!("Text delivery failed; insertion may be partial or complete; no retry attempted: {error:#}")
        })
    }
}

type EffectFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, Default)]
struct Capabilities {
    hyprland: bool,
    wayland: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Focus {
    address: String,
    class: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Insertion {
    Type,
    Paste(PasteRoute),
}

/// Exercise this boundary through `TextInjector::deliver`. Preparation opens
/// connections; it must never insert text or alter the clipboard.
trait DeliveryEffects: Send {
    fn capabilities(&self) -> Capabilities;
    fn focus(&mut self) -> EffectFuture<'_, Option<Focus>>;
    fn prepare(&mut self, insertion: Insertion) -> Result<()>;
    fn settle_focus(&mut self) -> EffectFuture<'_, ()>;
    fn validate_focus<'a>(&'a mut self, focus: Option<&'a Focus>) -> EffectFuture<'a, ()>;
    fn publish_clipboard<'a>(&'a mut self, text: &'a str) -> EffectFuture<'a, ()>;
    fn insert<'a>(&'a mut self, insertion: Insertion, text: &'a str) -> EffectFuture<'a, ()>;
    fn cleanup(&mut self, insertion: Insertion) -> Result<()>;
}
