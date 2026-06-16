pub mod injector;
pub mod manager;
mod registry;
pub mod shortcuts;

pub use injector::TextInjector;
pub use manager::{InputManagerHandle, InputSnapshot, InputStats};
pub use registry::list_available_keyboards;
pub use shortcuts::{ShortcutEvent, ShortcutKind, ShortcutPhase};
