//! Privileged Linux regressions for the keyboard registry's uinput boundary.

use super::registry::KeyboardRegistry;
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, BusType, Device, EventType, InputEvent, InputId, KeyCode, SwitchCode};
use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const CHILD_ENV: &str = "HYPRWHSPR_REGISTRY_CHURN_TEST_CHILD";
const DEVICE_TIMEOUT: Duration = Duration::from_secs(1);
const TEST_NAME: &str = "input::churn_tests::registry_poll_finishes_during_uinput_churn";

fn key_event(key: KeyCode, value: i32) -> InputEvent {
    InputEvent::new(EventType::KEY.0, key.0, value)
}

fn create_test_keyboard() -> io::Result<(PathBuf, VirtualDevice)> {
    let mut keys = AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode::KEY_A);
    keys.insert(KeyCode::KEY_S);
    keys.insert(KeyCode::KEY_D);

    let mut switches = AttributeSet::<SwitchCode>::new();
    switches.insert(SwitchCode::SW_LID);
    switches.insert(SwitchCode::SW_TABLET_MODE);

    let mut output = VirtualDevice::builder()?
        .name("hyprwhspr registry churn regression")
        .input_id(InputId::new(BusType::BUS_USB, 0xdead, 0xbeef, 1))
        .with_keys(&keys)?
        .with_switches(&switches)?
        .build()?;
    let path = output
        .enumerate_dev_nodes_blocking()?
        .next()
        .transpose()?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "uinput event node not found"))?;

    wait_for_device(&path, true)?;
    Ok((path, output))
}

fn wait_for_device(path: &Path, present: bool) -> io::Result<()> {
    let started = Instant::now();
    loop {
        let found = Device::open(path).is_ok();
        if found == present {
            return Ok(());
        }
        if started.elapsed() >= DEVICE_TIMEOUT {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("device {} did not become present={present}", path.display()),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_registry(
    registry: &mut KeyboardRegistry,
    path: &Path,
    present: bool,
) -> anyhow::Result<()> {
    let started = Instant::now();
    loop {
        registry.refresh()?;
        let found = registry.device_paths().iter().any(|device| device == path);
        if found == present {
            return Ok(());
        }
        if started.elapsed() >= DEVICE_TIMEOUT {
            anyhow::bail!(
                "registry did not observe {} present={present}; devices={:?}",
                path.display(),
                registry.device_paths()
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn run_bounded_child() -> io::Result<bool> {
    if env::var_os(CHILD_ENV).is_some() {
        return Ok(false);
    }

    let mut child = Command::new(env::current_exe()?)
        .args(["--exact", TEST_NAME, "--ignored", "--nocapture"])
        .env(CHILD_ENV, "1")
        .spawn()?;
    let started = Instant::now();

    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(true)
            } else {
                Err(io::Error::other(format!(
                    "child regression failed with {status}"
                )))
            };
        }
        if started.elapsed() >= Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "registry churn regression exceeded 30 seconds",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "requires write access to /dev/uinput"]
fn registry_poll_finishes_during_uinput_churn() -> anyhow::Result<()> {
    if run_bounded_child()? {
        return Ok(());
    }

    let mut registry = KeyboardRegistry::open_initial()?;

    // Repeat the issue reproducer's add, overflow, synchronized poll, and
    // removal cycle. Explicit refreshes avoid relying on container udev
    // multicast, while still exercising the production registry and evdev fd.
    for _ in 0..10 {
        let (path, mut output) = create_test_keyboard()?;
        wait_for_registry(&mut registry, &path, true)?;

        output.emit(&[InputEvent::new(
            EventType::SWITCH.0,
            SwitchCode::SW_LID.0,
            1,
        )])?;
        registry.poll_input_events(256)?;

        for _ in 0..1_024 {
            output.emit(&[key_event(KeyCode::KEY_D, 1), key_event(KeyCode::KEY_D, 0)])?;
        }
        registry.poll_input_events(256)?;

        output.emit(&[key_event(KeyCode::KEY_D, 1), key_event(KeyCode::KEY_D, 0)])?;
        let outcome = registry.poll_input_events(256)?;
        assert!(outcome.drained_events <= 256);

        drop(output);
        wait_for_device(&path, false)?;
        wait_for_registry(&mut registry, &path, false)?;
    }

    Ok(())
}
