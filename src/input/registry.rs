use anyhow::Result;
use evdev::{Device, InputEventKind, Key};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};

pub(super) struct KeyboardRegistry {
    devices: Vec<KeyboardDevice>,
    pressed_keys: HashSet<Key>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct PollOutcome {
    pub key_events: usize,
    pub devices_changed: bool,
    pub backpressure: bool,
}

struct KeyboardDevice {
    path: PathBuf,
    name: String,
    device: Device,
}

impl KeyboardRegistry {
    pub(super) fn open_initial() -> Result<Self> {
        let devices = Self::find_keyboard_devices(true)?;
        if devices.is_empty() {
            warn!("No keyboard devices found!");
            warn!("Make sure you have read permissions for /dev/input/event*");
            warn!("You may need to add your user to the 'input' group");
        }

        Ok(Self {
            devices,
            pressed_keys: HashSet::new(),
        })
    }

    pub(super) fn pressed_keys(&self) -> &HashSet<Key> {
        &self.pressed_keys
    }

    pub(super) fn device_count(&self) -> usize {
        self.devices.len()
    }

    pub(super) fn device_paths(&self) -> Vec<PathBuf> {
        self.devices
            .iter()
            .map(|device| device.path.clone())
            .collect()
    }

    pub(super) fn poll_key_events(&mut self, max_events: usize) -> Result<PollOutcome> {
        let mut outcome = PollOutcome::default();
        let mut removed_devices = HashSet::new();

        for entry in &mut self.devices {
            if is_device_node_stale(entry.device.as_raw_fd(), &entry.path) {
                warn!(
                    "Keyboard device node changed or disappeared at {:?}; refreshing input devices",
                    entry.path
                );
                removed_devices.insert(entry.path.clone());
                continue;
            }

            match entry.device.fetch_events() {
                Ok(events) => {
                    for event in events {
                        if let InputEventKind::Key(key) = event.kind() {
                            match event.value() {
                                1 => {
                                    self.pressed_keys.insert(key);
                                    outcome.key_events += 1;
                                }
                                0 => {
                                    self.pressed_keys.remove(&key);
                                    outcome.key_events += 1;
                                }
                                _ => {}
                            }
                        }

                        if outcome.key_events >= max_events {
                            outcome.backpressure = true;
                            return Ok(outcome);
                        }
                    }
                }
                Err(err) => {
                    if err.kind() != io::ErrorKind::WouldBlock {
                        error!("Error fetching input events from {:?}: {}", entry.path, err);
                        if is_device_disconnect_error(&err) {
                            removed_devices.insert(entry.path.clone());
                        }
                    }
                }
            }
        }

        if !removed_devices.is_empty() {
            self.devices
                .retain(|device| !removed_devices.contains(&device.path));
            self.pressed_keys.clear();
            outcome.devices_changed = true;
            self.refresh()?;
        }

        Ok(outcome)
    }

    pub(super) fn refresh(&mut self) -> Result<bool> {
        let previous_paths: HashSet<PathBuf> = self
            .devices
            .iter()
            .map(|device| device.path.clone())
            .collect();
        let devices = Self::find_keyboard_devices(false)?;
        let updated_paths: HashSet<PathBuf> =
            devices.iter().map(|device| device.path.clone()).collect();
        let changed = previous_paths != updated_paths;

        if changed {
            info!(
                "Keyboard devices refreshed - monitoring {} device(s)",
                devices.len()
            );
            debug!(
                devices = ?devices
                    .iter()
                    .map(|device| (&device.path, &device.name))
                    .collect::<Vec<_>>(),
                "Keyboard device set changed"
            );
            self.pressed_keys.clear();
        } else {
            debug!(
                "Keyboard devices refreshed - monitoring {} device(s)",
                devices.len()
            );
        }

        self.devices = devices;
        Ok(changed)
    }

    fn find_keyboard_devices(log_devices: bool) -> Result<Vec<KeyboardDevice>> {
        let mut keyboards = Vec::new();

        for (path, device) in evdev::enumerate() {
            if is_keyboard_device(&device) {
                if let Err(err) = set_device_nonblocking(&device) {
                    warn!("Failed to set non-blocking mode for {:?}: {}", path, err);
                }
                let name = device.name().unwrap_or("Unknown").to_string();
                if log_devices {
                    info!("Found keyboard device: {} at {:?}", name, path);
                }
                keyboards.push(KeyboardDevice { path, name, device });
            }
        }

        Ok(keyboards)
    }
}

pub fn list_available_keyboards() -> Result<Vec<(PathBuf, String)>> {
    let mut keyboards = Vec::new();

    for (path, device) in evdev::enumerate() {
        if is_keyboard_device(&device) {
            let name = device.name().unwrap_or("Unknown").to_string();
            keyboards.push((path, name));
        }
    }

    Ok(keyboards)
}

fn is_keyboard_device(device: &Device) -> bool {
    device.supported_keys().is_some_and(|keys| {
        keys.contains(Key::KEY_A) && keys.contains(Key::KEY_S) && keys.contains(Key::KEY_D)
    })
}

fn is_device_disconnect_error(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(code) if code == libc::ENODEV || code == libc::EBADF || code == libc::ENXIO
    )
}

fn is_device_node_stale(fd: RawFd, path: &Path) -> bool {
    match fd_matches_path(fd, path) {
        Ok(matches) => !matches,
        Err(err) if err.kind() == io::ErrorKind::NotFound => true,
        Err(err) if is_device_disconnect_error(&err) => true,
        Err(err) => {
            debug!("Failed to validate input device node {:?}: {}", path, err);
            false
        }
    }
}

fn fd_matches_path(fd: RawFd, path: &Path) -> io::Result<bool> {
    let mut fd_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd, fd_stat.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd_stat = unsafe { fd_stat.assume_init() };
    let path_metadata = fs::metadata(path)?;

    Ok(path_metadata.dev() == fd_stat.st_dev && path_metadata.ino() == fd_stat.st_ino)
}

fn set_device_nonblocking(device: &Device) -> Result<()> {
    let fd = device.as_raw_fd();

    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(anyhow::anyhow!(
            "fcntl(F_GETFL) failed: {}",
            io::Error::last_os_error()
        ));
    }

    if (flags & libc::O_NONBLOCK) != 0 {
        return Ok(());
    }

    let result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if result < 0 {
        return Err(anyhow::anyhow!(
            "fcntl(F_SETFL) failed: {}",
            io::Error::last_os_error()
        ));
    }

    Ok(())
}

pub(super) fn is_input_event_node(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|node| node.starts_with("/dev/input/event"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "hyprwhspr-rs-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ))
    }

    #[test]
    fn open_fd_matches_existing_path() {
        let path = temp_path("matching-fd");
        fs::write(&path, b"device").unwrap();
        let file = File::open(&path).unwrap();

        assert!(fd_matches_path(file.as_raw_fd(), &path).unwrap());
        assert!(!is_device_node_stale(file.as_raw_fd(), &path));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn recreated_path_marks_open_fd_stale() {
        let path = temp_path("recreated-fd");
        fs::write(&path, b"old-device").unwrap();
        let file = File::open(&path).unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"new-device").unwrap();

        assert!(!fd_matches_path(file.as_raw_fd(), &path).unwrap());
        assert!(is_device_node_stale(file.as_raw_fd(), &path));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn missing_path_marks_open_fd_stale() {
        let path = temp_path("missing-fd");
        fs::write(&path, b"device").unwrap();
        let file = File::open(&path).unwrap();
        fs::remove_file(&path).unwrap();

        assert!(is_device_node_stale(file.as_raw_fd(), &path));
    }
}
