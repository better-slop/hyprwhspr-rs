//! Linux uinput regressions for evdev synchronization after device churn.
//!
//! These tests need write access to `/dev/uinput`, so the normal test gate
//! compiles but does not run them. Run explicitly on a suitably privileged
//! Linux host with:
//!
//! ```text
//! cargo test --test input_churn -- --ignored --nocapture
//! ```

use evdev::uinput::VirtualDevice;
use evdev::{
    AttributeSet, BusType, Device, EventSummary, EventType, InputEvent, InputId, KeyCode,
    SwitchCode,
};
use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const DEVICE_READY_TIMEOUT: Duration = Duration::from_secs(1);
const SYN_DROPPED_CHILD: &str = "HYPRWHSPR_SYN_DROPPED_TEST_CHILD";

fn key_event(key: KeyCode, value: i32) -> InputEvent {
    InputEvent::new(EventType::KEY.0, key.0, value)
}

fn create_test_keyboard() -> io::Result<(PathBuf, VirtualDevice)> {
    let mut keys = AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode::KEY_A);
    keys.insert(KeyCode::KEY_S);
    keys.insert(KeyCode::KEY_D);

    // The pre-0.13 evdev synchronization bug needs adjacent supported switch
    // codes. A held SW_LID makes its sliced iterator repeatedly report code 0
    // instead of advancing to SW_TABLET_MODE after SYN_DROPPED. Upstream fixed
    // the cursor bug in https://github.com/emberian/evdev/pull/160.
    let mut switches = AttributeSet::<SwitchCode>::new();
    switches.insert(SwitchCode::SW_LID);
    switches.insert(SwitchCode::SW_TABLET_MODE);

    let mut output = VirtualDevice::builder()?
        .name("hyprwhspr input churn regression")
        .input_id(InputId::new(BusType::BUS_USB, 0xdead, 0xbeef, 1))
        .with_keys(&keys)?
        .with_switches(&switches)?
        .build()?;

    let path = output
        .enumerate_dev_nodes_blocking()?
        .next()
        .transpose()?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "uinput event node not found"))?;

    wait_for_device_ready(&path)?;
    Ok((path, output))
}

fn wait_for_device_ready(path: &Path) -> io::Result<()> {
    let started = Instant::now();

    loop {
        match Device::open(path) {
            Ok(_) => return Ok(()),
            Err(err)
                if started.elapsed() < DEVICE_READY_TIMEOUT
                    && matches!(
                        err.kind(),
                        io::ErrorKind::NotFound
                            | io::ErrorKind::PermissionDenied
                            | io::ErrorKind::WouldBlock
                    ) =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(err) => return Err(err),
        }
    }
}

/// Runs a potentially wedging regression in a child test process.
///
/// A thread timeout is insufficient here: unwinding would drop the input
/// manager and join its stuck worker. The parent must be able to kill the whole
/// child if evdev synchronization stops making progress.
fn run_bounded_subprocess(child_env: &str, test_name: &str, timeout: Duration) -> io::Result<bool> {
    if env::var_os(child_env).is_some() {
        return Ok(false);
    }

    let mut child = Command::new(env::current_exe()?)
        .args(["--exact", test_name, "--ignored", "--nocapture"])
        .env(child_env, "1")
        .spawn()?;
    let started = Instant::now();

    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(true)
            } else {
                Err(io::Error::other(format!(
                    "child regression {test_name} failed with {status}"
                )))
            };
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("child regression {test_name} exceeded {timeout:?}"),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "requires write access to /dev/uinput"]
fn syn_dropped_compensation_finishes_after_uinput_churn() -> io::Result<()> {
    if run_bounded_subprocess(
        SYN_DROPPED_CHILD,
        "syn_dropped_compensation_finishes_after_uinput_churn",
        Duration::from_secs(2),
    )? {
        return Ok(());
    }

    let (path, mut output) = create_test_keyboard()?;
    let mut input = Device::open(path)?;

    output.emit(&[InputEvent::new(
        EventType::SWITCH.0,
        SwitchCode::SW_LID.0,
        1,
    )])?;

    // Overflow the evdev ring buffer while the reader is idle. The next
    // complete frame leaves SYN_DROPPED compensation pending.
    for _ in 0..1_024 {
        output.emit(&[key_event(KeyCode::KEY_D, 1), key_event(KeyCode::KEY_D, 0)])?;
    }
    assert_eq!(input.fetch_events()?.count(), 0);

    output.emit(&[key_event(KeyCode::KEY_D, 1), key_event(KeyCode::KEY_D, 0)])?;
    let events = input.fetch_events()?.collect::<Vec<_>>();

    assert!(
        events.iter().any(|event| {
            matches!(
                event.destructure(),
                EventSummary::Switch(_, SwitchCode::SW_LID, 1)
            )
        }),
        "compensation omitted the held switch state"
    );

    Ok(())
}
