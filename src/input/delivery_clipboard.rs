//! Clipboard publication with an observable readiness barrier. wl-clipboard-rs
//! acknowledges preparation before its owner thread flushes the selection, so
//! keyboard dispatch/client acknowledgement must wait for visible UTF-8 text.

use anyhow::{bail, Context, Result};
use std::io::{ErrorKind, Read};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};
use wl_clipboard_rs::{copy, paste};

const PUBLISH_DEADLINE: Duration = Duration::from_secs(1);
const OBSERVATION_INTERVAL: Duration = Duration::from_millis(10);

pub(super) async fn publish(text: &str) -> Result<()> {
    let bytes = text.as_bytes().to_vec();
    let deadline = Instant::now() + PUBLISH_DEADLINE;
    // One worker for publication AND all observations. A Tokio timeout cannot
    // cancel a native protocol roundtrip already blocked inside the library.
    // Pipe payload reads below are nonblocking and honor the same deadline;
    // after a stalled protocol call returns, the worker checks its deadline.
    let worker = tokio::task::spawn_blocking(move || {
        ensure_before_deadline(deadline)?;
        let mut options = copy::Options::new();
        options.clipboard(copy::ClipboardType::Regular);
        options
            .copy(
                copy::Source::Bytes(bytes.clone().into_boxed_slice()),
                copy::MimeType::Text,
            )
            .context("Cannot publish text to Wayland clipboard")?;
        wait_until_visible(
            &bytes,
            |limit| observe_text(limit, deadline),
            || {
                ensure_before_deadline(deadline)?;
                std::thread::sleep(
                    OBSERVATION_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                );
                ensure_before_deadline(deadline)
            },
        )
    });
    tokio::time::timeout(PUBLISH_DEADLINE, worker).await
        .context("Clipboard readiness deadline exceeded; publication may have happened; no insertion attempted")?
        .context("Clipboard publisher task failed")?
        .context("Clipboard publication was not confirmed; publication may have happened; no insertion attempted")
}

fn wait_until_visible(
    expected: &[u8],
    mut observe: impl FnMut(usize) -> Result<Option<Vec<u8>>>,
    mut wait: impl FnMut() -> Result<()>,
) -> Result<()> {
    loop {
        if observe(expected.len().saturating_add(1))?.as_deref() == Some(expected) {
            return Ok(());
        }
        // Only read-only observations repeat. Never republish or retry input.
        wait()?;
    }
}

fn ensure_before_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        bail!("Clipboard readiness deadline exceeded");
    }
    Ok(())
}

fn observe_text(limit: usize, deadline: Instant) -> Result<Option<Vec<u8>>> {
    ensure_before_deadline(deadline)?;
    let result = paste::get_contents(
        paste::ClipboardType::Regular,
        paste::Seat::Unspecified,
        paste::MimeType::Specific("text/plain;charset=utf-8"),
    );
    let (pipe, _) = match result {
        Ok(contents) => contents,
        Err(paste::Error::ClipboardEmpty | paste::Error::NoMimeType) => return Ok(None),
        Err(error) => return Err(error).context("Cannot observe Wayland clipboard readiness"),
    };
    // Matching text proves visibility, not clipboard ownership or generation.
    // We deliberately do not offer restoration based on this observation.
    read_bounded(pipe, limit, deadline).map(Some)
}

fn read_bounded(
    mut reader: impl Read + AsRawFd,
    limit: usize,
    deadline: Instant,
) -> Result<Vec<u8>> {
    let fd = reader.as_raw_fd();
    // SAFETY: this valid descriptor remains owned by reader throughout the call.
    // Only this worker reads it; O_NONBLOCK prevents a foreign clipboard owner
    // from keeping a payload read blocked beyond the readiness deadline.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error())
            .context("Cannot make clipboard pipe nonblocking");
    }
    let mut contents = Vec::new();
    let mut buffer = [0u8; 8192];
    while contents.len() < limit {
        ensure_before_deadline(deadline)?;
        let milliseconds = deadline
            .saturating_duration_since(Instant::now())
            .min(OBSERVATION_INTERVAL)
            .as_millis()
            .max(1) as i32;
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pollfd points to one initialized descriptor, and the timeout
        // is bounded by the remaining deadline (rounded up by at most 1 ms).
        let ready = unsafe { libc::poll(&mut pollfd, 1, milliseconds) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(error).context("Cannot poll clipboard readiness pipe");
        }
        if ready == 0 {
            continue;
        }
        let remaining = (limit - contents.len()).min(buffer.len());
        match reader.read(&mut buffer[..remaining]) {
            Ok(0) => return Ok(contents),
            Ok(length) => contents.extend_from_slice(&buffer[..length]),
            Err(error)
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) =>
            {
                continue
            }
            Err(error) => return Err(error).context("Cannot read clipboard readiness pipe"),
        }
    }
    Ok(contents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    #[test]
    fn delayed_visibility_waits_for_exact_text_without_republishing() {
        let mut observations = [None, Some(b"old".to_vec()), Some(b"ready".to_vec())].into_iter();
        let mut waits = 0;
        wait_until_visible(
            b"ready",
            |limit| {
                assert_eq!(limit, 6);
                Ok(observations.next().unwrap())
            },
            || {
                waits += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(waits, 2);
    }

    #[test]
    fn deadline_prevents_acknowledging_unconfirmed_text() {
        let error = wait_until_visible(
            b"expected",
            |_| Ok(Some(b"old".to_vec())),
            || bail!("controlled readiness deadline exceeded"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("deadline exceeded"));
    }

    #[test]
    fn payload_read_stops_at_expected_length_plus_one() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"oversized").unwrap();
        let contents = read_bounded(reader, 4, Instant::now() + Duration::from_secs(1)).unwrap();
        assert_eq!(contents, b"over");
    }

    #[test]
    fn stalled_payload_cannot_hold_reader_beyond_deadline() {
        let (reader, _writer) = UnixStream::pair().unwrap();
        let error =
            read_bounded(reader, 2, Instant::now() + Duration::from_millis(10)).unwrap_err();
        assert!(error.to_string().contains("deadline exceeded"));
    }

    #[test]
    fn empty_copy_requires_empty_visible_payload() {
        let mut observations = [Some(b"x".to_vec()), Some(Vec::new())].into_iter();
        wait_until_visible(
            b"",
            |limit| {
                assert_eq!(limit, 1);
                Ok(observations.next().unwrap())
            },
            || Ok(()),
        )
        .unwrap();
    }
}
