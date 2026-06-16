mod hyprland;
pub mod injector;
pub mod manager;
mod paste_route;
mod registry;
pub mod shortcuts;
mod source;

pub use injector::TextInjector;
pub use manager::{InputManagerHandle, InputSnapshot, InputStats};
pub use registry::list_available_keyboards;
pub use shortcuts::{ShortcutEvent, ShortcutKind, ShortcutPhase};
