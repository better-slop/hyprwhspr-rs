use anyhow::{Context, Result};
use evdev::{Device, InputEventKind, Key};
use std::collections::HashSet;
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{
    mpsc as std_mpsc,
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use udev::{EventType, MonitorBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutKind {
    Hold,
    Press,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutPhase {
    Start,
    End,
}

#[derive(Debug, Clone)]
pub struct ShortcutEvent {
    pub triggered_at: Instant,
    pub kind: ShortcutKind,
    pub phase: ShortcutPhase,
}

pub struct GlobalShortcuts {
    devices: Vec<KeyboardDevice>,
    target_keys: HashSet<Key>,
    shortcut_name: String,
    kind: ShortcutKind,
}

struct KeyboardDevice {
    path: PathBuf,
    device: Device,
}

#[derive(Debug, Clone)]
enum InputDeviceEvent {
    Added(PathBuf),
    Removed(PathBuf),
    Changed(PathBuf),
    MonitorUnavailable(String),
}

impl GlobalShortcuts {
    pub fn new(shortcut: &str, kind: ShortcutKind) -> Result<Self> {
        let target_keys = Self::parse_shortcut(shortcut)?;
        let devices = Self::find_keyboard_devices(true)?;

        if devices.is_empty() {
            return Err(anyhow::anyhow!("No keyboard devices found"));
        }

        let mode_label = match kind {
            ShortcutKind::Hold => "hold",
            ShortcutKind::Press => "press",
        };

        info!(
            "Global shortcuts initialized - monitoring {} device(s) for {} shortcut: {}",
            devices.len(),
            mode_label,
            shortcut
        );
        debug!("Target keys: {:?}", target_keys);

        Ok(Self {
            devices,
            target_keys,
            shortcut_name: shortcut.to_string(),
            kind,
        })
    }

    pub fn run(mut self, tx: mpsc::Sender<ShortcutEvent>, stop: Arc<AtomicBool>) -> Result<()> {
        let mut pressed_keys: HashSet<Key> = HashSet::new();
        let mut last_trigger = Instant::now() - Duration::from_secs(10);
        let debounce_duration = Duration::from_millis(500);
        let mut combination_active = false;
        let fallback_rescan_interval = Duration::from_secs(1);
        let mut fallback_rescan_enabled = false;
        let mut last_fallback_rescan = Instant::now();
        let (rescan_tx, rescan_rx) = std_mpsc::channel();
        let monitor_stop = stop.clone();
        std::thread::spawn(move || {
            let monitor_tx = rescan_tx.clone();
            if let Err(err) = Self::watch_input_devices(monitor_tx, monitor_stop) {
                let _ = rescan_tx.send(InputDeviceEvent::MonitorUnavailable(err.to_string()));
            }
        });

        let listen_label = match self.kind {
            ShortcutKind::Hold => "hold",
            ShortcutKind::Press => "press",
        };
        info!(
            "🎯 Listening for {} shortcut: {}",
            listen_label, self.shortcut_name
        );

        'outer: loop {
            if stop.load(Ordering::Relaxed) {
                info!("Stopping shortcut listener: {}", self.shortcut_name);
                break 'outer;
            }

            while let Ok(event) = rescan_rx.try_recv() {
                match event {
                    InputDeviceEvent::MonitorUnavailable(reason) => {
                        if !fallback_rescan_enabled {
                            warn!(
                                "Input device monitor unavailable ({}); falling back to periodic rescan",
                                reason
                            );
                        }
                        fallback_rescan_enabled = true;
                        last_fallback_rescan = Instant::now() - fallback_rescan_interval;
                    }
                    _ => {
                        self.handle_input_device_event(event);
                        pressed_keys.clear();
                        combination_active = false;
                    }
                }
            }

            let mut removed_devices = HashSet::new();

            let target_keys = &self.target_keys;
            let shortcut_name = &self.shortcut_name;

            for entry in &mut self.devices {
                if stop.load(Ordering::Relaxed) {
                    break 'outer;
                }
                // Fetch events from this device
                match entry.device.fetch_events() {
                    Ok(events) => {
                        for event in events {
                            if stop.load(Ordering::Relaxed) {
                                break 'outer;
                            }
                            match event.kind() {
                                InputEventKind::Key(key) => {
                                    let value = event.value();

                                    match value {
                                        // Key pressed
                                        1 => {
                                            pressed_keys.insert(key);

                                            // Check if target combination is pressed
                                            if target_keys.is_subset(&pressed_keys)
                                                && !combination_active
                                            {
                                                let now = Instant::now();

                                                // Debounce: only trigger if enough time has passed
                                                let should_trigger = match self.kind {
                                                    ShortcutKind::Hold => true,
                                                    ShortcutKind::Press => {
                                                        now.duration_since(last_trigger)
                                                            > debounce_duration
                                                    }
                                                };

                                                if should_trigger {
                                                    debug!(
                                                        "✓ Combination active: {:?}",
                                                        target_keys
                                                    );
                                                    info!(
                                                        "✨ Shortcut triggered: {}",
                                                        shortcut_name
                                                    );
                                                    last_trigger = now;
                                                    combination_active = true;

                                                    if let Err(e) = tx.try_send(ShortcutEvent {
                                                        triggered_at: now,
                                                        kind: self.kind,
                                                        phase: ShortcutPhase::Start,
                                                    }) {
                                                        warn!(
                                                            "Failed to send shortcut event: {}",
                                                            e
                                                        );
                                                    }
                                                } else {
                                                    debug!("Shortcut debounced (too soon)");
                                                }
                                            }
                                        }
                                        // Key released
                                        0 => {
                                            pressed_keys.remove(&key);

                                            if combination_active
                                                && !target_keys.is_subset(&pressed_keys)
                                            {
                                                debug!(
                                                    "✗ Combination broken by releasing: {:?}",
                                                    key
                                                );
                                                combination_active = false;

                                                if matches!(self.kind, ShortcutKind::Hold) {
                                                    if let Err(e) = tx.try_send(ShortcutEvent {
                                                        triggered_at: Instant::now(),
                                                        kind: self.kind,
                                                        phase: ShortcutPhase::End,
                                                    }) {
                                                        warn!(
                                                            "Failed to send shortcut release event: {}",
                                                            e
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        if e.kind() != std::io::ErrorKind::WouldBlock {
                            error!("Error fetching events: {}", e);

                            if is_device_disconnect_error(&e) {
                                warn!("Input device went away; removing device");
                                removed_devices.insert(entry.path.clone());
                            }
                        }
                        if stop.load(Ordering::Relaxed) {
                            break 'outer;
                        }
                    }
                }
            }

            if !removed_devices.is_empty() {
                let before = self.devices.len();
                self.devices
                    .retain(|device| !removed_devices.contains(&device.path));
                let removed = before.saturating_sub(self.devices.len());
                if removed > 0 {
                    info!("Removed {} keyboard device(s)", removed);
                }
                pressed_keys.clear();
                combination_active = false;
            }

            if fallback_rescan_enabled
                && last_fallback_rescan.elapsed() >= fallback_rescan_interval
            {
                last_fallback_rescan = Instant::now();
                pressed_keys.clear();
                combination_active = false;
                if let Err(err) = self.refresh_devices() {
                    error!("Failed to refresh keyboard devices: {}", err);
                }
            }

            // Small sleep to prevent busy-waiting
            std::thread::sleep(Duration::from_millis(10));
        }

        Ok(())
    }

    fn parse_shortcut(shortcut: &str) -> Result<HashSet<Key>> {
        let mut keys = HashSet::new();

        for part in shortcut.split('+') {
            let part = part.trim().to_uppercase();
            let key =
                Self::parse_key(&part).with_context(|| format!("Failed to parse key: {}", part))?;
            keys.insert(key);
        }

        if keys.is_empty() {
            return Err(anyhow::anyhow!("Empty shortcut"));
        }

        Ok(keys)
    }

    fn parse_key(key_str: &str) -> Result<Key> {
        match key_str {
            // Modifiers
            "SUPER" | "META" | "WIN" | "WINDOWS" => Ok(Key::KEY_LEFTMETA),
            "ALT" => Ok(Key::KEY_LEFTALT),
            "CTRL" | "CONTROL" => Ok(Key::KEY_LEFTCTRL),
            "SHIFT" => Ok(Key::KEY_LEFTSHIFT),

            // Function keys
            "F1" => Ok(Key::KEY_F1),
            "F2" => Ok(Key::KEY_F2),
            "F3" => Ok(Key::KEY_F3),
            "F4" => Ok(Key::KEY_F4),
            "F5" => Ok(Key::KEY_F5),
            "F6" => Ok(Key::KEY_F6),
            "F7" => Ok(Key::KEY_F7),
            "F8" => Ok(Key::KEY_F8),
            "F9" => Ok(Key::KEY_F9),
            "F10" => Ok(Key::KEY_F10),
            "F11" => Ok(Key::KEY_F11),
            "F12" => Ok(Key::KEY_F12),

            // Letter keys
            "A" => Ok(Key::KEY_A),
            "B" => Ok(Key::KEY_B),
            "C" => Ok(Key::KEY_C),
            "D" => Ok(Key::KEY_D),
            "E" => Ok(Key::KEY_E),
            "F" => Ok(Key::KEY_F),
            "G" => Ok(Key::KEY_G),
            "H" => Ok(Key::KEY_H),
            "I" => Ok(Key::KEY_I),
            "J" => Ok(Key::KEY_J),
            "K" => Ok(Key::KEY_K),
            "L" => Ok(Key::KEY_L),
            "M" => Ok(Key::KEY_M),
            "N" => Ok(Key::KEY_N),
            "O" => Ok(Key::KEY_O),
            "P" => Ok(Key::KEY_P),
            "Q" => Ok(Key::KEY_Q),
            "R" => Ok(Key::KEY_R),
            "S" => Ok(Key::KEY_S),
            "T" => Ok(Key::KEY_T),
            "U" => Ok(Key::KEY_U),
            "V" => Ok(Key::KEY_V),
            "W" => Ok(Key::KEY_W),
            "X" => Ok(Key::KEY_X),
            "Y" => Ok(Key::KEY_Y),
            "Z" => Ok(Key::KEY_Z),

            // Number keys
            "0" => Ok(Key::KEY_0),
            "1" => Ok(Key::KEY_1),
            "2" => Ok(Key::KEY_2),
            "3" => Ok(Key::KEY_3),
            "4" => Ok(Key::KEY_4),
            "5" => Ok(Key::KEY_5),
            "6" => Ok(Key::KEY_6),
            "7" => Ok(Key::KEY_7),
            "8" => Ok(Key::KEY_8),
            "9" => Ok(Key::KEY_9),

            // Special keys
            "SPACE" => Ok(Key::KEY_SPACE),
            "ENTER" | "RETURN" => Ok(Key::KEY_ENTER),
            "ESC" | "ESCAPE" => Ok(Key::KEY_ESC),
            "TAB" => Ok(Key::KEY_TAB),
            "BACKSPACE" => Ok(Key::KEY_BACKSPACE),
            "DELETE" | "DEL" => Ok(Key::KEY_DELETE),
            "INSERT" | "INS" => Ok(Key::KEY_INSERT),
            "HOME" => Ok(Key::KEY_HOME),
            "END" => Ok(Key::KEY_END),
            "PAGEUP" | "PGUP" => Ok(Key::KEY_PAGEUP),
            "PAGEDOWN" | "PGDOWN" => Ok(Key::KEY_PAGEDOWN),

            // Arrow keys
            "UP" => Ok(Key::KEY_UP),
            "DOWN" => Ok(Key::KEY_DOWN),
            "LEFT" => Ok(Key::KEY_LEFT),
            "RIGHT" => Ok(Key::KEY_RIGHT),

            _ => Err(anyhow::anyhow!("Unknown key: {}", key_str)),
        }
    }

    fn find_keyboard_devices(log_devices: bool) -> Result<Vec<KeyboardDevice>> {
        let mut keyboards = Vec::new();

        for (path, device) in evdev::enumerate() {
            if Self::is_keyboard_device(&device) {
                if let Err(err) = set_device_nonblocking(&device) {
                    warn!("Failed to set non-blocking mode for {:?}: {}", path, err);
                }
                let name = device.name().unwrap_or("Unknown");
                if log_devices {
                    info!("Found keyboard device: {} at {:?}", name, path);
                }
                keyboards.push(KeyboardDevice { path, device });
            }
        }

        if keyboards.is_empty() {
            warn!("No keyboard devices found!");
            warn!("Make sure you have read permissions for /dev/input/event*");
            warn!("You may need to add your user to the 'input' group");
        }

        Ok(keyboards)
    }

    pub fn list_available_keyboards() -> Result<Vec<(PathBuf, String)>> {
        let mut keyboards = Vec::new();

        for (path, device) in evdev::enumerate() {
            if Self::is_keyboard_device(&device) {
                let name = device.name().unwrap_or("Unknown").to_string();
                keyboards.push((path, name));
            }
        }

        Ok(keyboards)
    }

    fn refresh_devices(&mut self) -> Result<()> {
        let devices = Self::find_keyboard_devices(false)?;
        let previous = self.devices.len();
        let updated = devices.len();

        if updated == 0 && previous != 0 {
            warn!("No keyboard devices found!");
        } else if updated != previous {
            info!(
                "Keyboard devices refreshed - monitoring {} device(s)",
                updated
            );
        } else {
            debug!("Keyboard devices refreshed - monitoring {} device(s)", updated);
        }

        self.devices = devices;

        Ok(())
    }

    fn is_keyboard_device(device: &Device) -> bool {
        device.supported_keys().is_some_and(|keys| {
            keys.contains(Key::KEY_A) && keys.contains(Key::KEY_S) && keys.contains(Key::KEY_D)
        })
    }

    fn open_keyboard_device(path: &Path) -> Result<Option<KeyboardDevice>> {
        let device = Device::open(path)?;
        if !Self::is_keyboard_device(&device) {
            return Ok(None);
        }
        if let Err(err) = set_device_nonblocking(&device) {
            warn!("Failed to set non-blocking mode for {:?}: {}", path, err);
        }
        let name = device.name().unwrap_or("Unknown");
        info!("Found keyboard device: {} at {:?}", name, path);
        Ok(Some(KeyboardDevice {
            path: path.to_path_buf(),
            device,
        }))
    }

    fn handle_input_device_event(&mut self, event: InputDeviceEvent) {
        match event {
            InputDeviceEvent::Added(path) => self.add_keyboard_device(path),
            InputDeviceEvent::Removed(path) => self.remove_keyboard_device(&path),
            InputDeviceEvent::Changed(path) => {
                self.remove_keyboard_device(&path);
                self.add_keyboard_device(path);
            }
            InputDeviceEvent::MonitorUnavailable(_) => {}
        }
    }

    fn add_keyboard_device(&mut self, path: PathBuf) {
        if self.devices.iter().any(|device| device.path == path) {
            return;
        }
        match Self::open_keyboard_device(&path) {
            Ok(Some(device)) => {
                self.devices.push(device);
                info!(
                    "Keyboard devices refreshed - monitoring {} device(s)",
                    self.devices.len()
                );
            }
            Ok(None) => {
                debug!("Input device added but not a keyboard: {:?}", path);
            }
            Err(err) => {
                warn!("Failed to open input device {:?}: {}", path, err);
            }
        }
    }

    fn remove_keyboard_device(&mut self, path: &Path) {
        let before = self.devices.len();
        self.devices.retain(|device| device.path != path);
        if self.devices.len() != before {
            info!(
                "Keyboard devices refreshed - monitoring {} device(s)",
                self.devices.len()
            );
        }
        if self.devices.is_empty() {
            warn!("No keyboard devices found!");
        }
    }

    fn watch_input_devices(
        tx: std_mpsc::Sender<InputDeviceEvent>,
        stop: Arc<AtomicBool>,
    ) -> Result<()> {
        let monitor = MonitorBuilder::new()?
            .match_subsystem("input")?
            .listen()?;

        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let mut saw_event = false;
            for event in monitor.iter() {
                saw_event = true;
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let Some(path) = event.device().devnode().map(Path::to_path_buf) else {
                    continue;
                };
                if !is_input_event_node(&path) {
                    continue;
                }
                let event_type = match event.event_type() {
                    EventType::Add => Some(InputDeviceEvent::Added(path)),
                    EventType::Remove => Some(InputDeviceEvent::Removed(path)),
                    EventType::Change => Some(InputDeviceEvent::Changed(path)),
                    _ => None,
                };
                if let Some(event_type) = event_type {
                    if tx.send(event_type).is_err() {
                        return Ok(());
                    }
                }
            }
            if !saw_event {
                std::thread::sleep(Duration::from_millis(100));
            }
        }

        Ok(())
    }
}

fn is_device_disconnect_error(err: &io::Error) -> bool {
    match err.raw_os_error() {
        Some(code) if code == libc::ENODEV || code == libc::EBADF || code == libc::ENXIO => true,
        _ => false,
    }
}

fn set_device_nonblocking(device: &Device) -> Result<()> {
    let fd = device.as_raw_fd();

    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(anyhow::anyhow!(
            "fcntl(F_GETFL) failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    if (flags & libc::O_NONBLOCK) != 0 {
        return Ok(());
    }

    let result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if result < 0 {
        return Err(anyhow::anyhow!(
            "fcntl(F_SETFL) failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    Ok(())
}

fn is_input_event_node(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|node| node.starts_with("/dev/input/event"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn detects_enodev_as_disconnect() {
        let err = io::Error::from_raw_os_error(libc::ENODEV);
        assert!(is_device_disconnect_error(&err));
    }

    #[test]
    fn does_not_treat_would_block_as_disconnect() {
        let err = io::Error::from(io::ErrorKind::WouldBlock);
        assert!(!is_device_disconnect_error(&err));
    }

    #[test]
    fn does_not_treat_other_errors_as_disconnect() {
        let err = io::Error::from(io::ErrorKind::BrokenPipe);
        assert!(!is_device_disconnect_error(&err));
    }

    #[test]
    fn filters_input_event_nodes() {
        assert!(is_input_event_node(Path::new("/dev/input/event0")));
        assert!(is_input_event_node(Path::new("/dev/input/event10")));
        assert!(!is_input_event_node(Path::new("/dev/input/mouse0")));
        assert!(!is_input_event_node(Path::new("/tmp/event0")));
    }
}
