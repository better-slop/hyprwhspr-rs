#[cfg(target_os = "linux")]
mod injector;
#[cfg(target_os = "windows")]
mod injector_windows;

#[cfg(target_os = "linux")]
mod shortcuts;
#[cfg(target_os = "windows")]
mod shortcuts_windows;

#[cfg(target_os = "linux")]
pub use injector::TextInjector;
#[cfg(target_os = "windows")]
pub use injector_windows::TextInjector;

#[cfg(target_os = "linux")]
pub use shortcuts::{GlobalShortcuts, ShortcutEvent, ShortcutKind, ShortcutPhase};
#[cfg(target_os = "windows")]
pub use shortcuts_windows::{GlobalShortcuts, ShortcutEvent, ShortcutKind, ShortcutPhase};
