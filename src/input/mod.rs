pub mod activation;
#[cfg(test)]
mod churn_tests;
mod hyprland;
pub mod injector;
mod linux;
pub mod manager;
mod paste_route;
mod registry;
mod shortcuts;
mod source;

pub use activation::{ActivationHealth, ShortcutEvent, ShortcutKind, ShortcutPhase};
pub use injector::{DeliveryConfig, DeliveryIntent, TextInjector};
pub use manager::{InputManagerHandle, InputSnapshot, InputStats};
pub use registry::list_available_keyboards;
