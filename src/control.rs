use crate::config::Config;
use anyhow::{anyhow, bail, Context, Result};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::{mpsc, oneshot, watch};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderState {
    Inactive,
    Recording,
    Processing,
}

impl RecorderState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inactive => "inactive",
            Self::Recording => "recording",
            Self::Processing => "processing",
        }
    }
}

impl fmt::Display for RecorderState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RecorderState {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim() {
            "inactive" => Ok(Self::Inactive),
            "recording" => Ok(Self::Recording),
            "processing" => Ok(Self::Processing),
            other => Err(anyhow!("Unknown recorder state: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordCommand {
    Start,
    Stop,
    Toggle,
    Status,
}

impl RecordCommand {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Toggle => "toggle",
            Self::Status => "status",
        }
    }
}

impl FromStr for RecordCommand {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim() {
            "start" => Ok(Self::Start),
            "stop" => Ok(Self::Stop),
            "toggle" => Ok(Self::Toggle),
            "status" => Ok(Self::Status),
            other => Err(anyhow!("Unknown record command: {other}")),
        }
    }
}

pub type ControlReply = std::result::Result<RecorderState, String>;

#[derive(Debug)]
pub enum ControlRequest {
    Record {
        command: RecordCommand,
        reply_tx: oneshot::Sender<ControlReply>,
    },
    /// Clipboard ownership stays in the daemon after the client receives its
    /// publication acknowledgement and exits. Never retry an uncertain request.
    CopyText {
        text: String,
        reply_tx: oneshot::Sender<std::result::Result<(), String>>,
    },
}

const COPY_PREFIX: &[u8] = b"copy\n";
// Record commands have fixed short verbs. Their framing allowance is separate
// from copy payload policy, so a one-byte copy limit still permits `status\n`.
const MAX_RECORD_FRAME_BYTES: usize = 64;
pub(crate) const MAX_COPY_TEXT_LIMIT: usize = isize::MAX as usize - COPY_PREFIX.len() - 1;

pub(crate) fn validate_copy_text_limit(limit: usize) -> Result<()> {
    if limit == 0 || limit > MAX_COPY_TEXT_LIMIT {
        bail!("max_copy_text_bytes must be between 1 and {MAX_COPY_TEXT_LIMIT}");
    }
    Ok(())
}

fn validate_copy_text_size(length: usize, limit: usize) -> Result<()> {
    validate_copy_text_limit(limit)?;
    if length > limit {
        bail!("Copy text exceeds max_copy_text_bytes limit of {limit} bytes");
    }
    Ok(())
}

/// Bound stdin before allocating the entire payload. Client configuration gives
/// an early resource bound; the daemon independently enforces its active config.
pub async fn read_copy_text(reader: impl AsyncRead + Unpin, limit: usize) -> Result<String> {
    validate_copy_text_limit(limit)?;
    let mut bytes = Vec::new();
    reader
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .context("Cannot read copy text from stdin")?;
    validate_copy_text_size(bytes.len(), limit)?;
    String::from_utf8(bytes).context("Copy stdin must contain valid UTF-8")
}

fn request_read_limit(limit: usize) -> Result<u64> {
    validate_copy_text_limit(limit)?;
    Ok((limit + COPY_PREFIX.len() + 1).max(MAX_RECORD_FRAME_BYTES + 1) as u64)
}

#[derive(Debug, PartialEq, Eq)]
enum WireRequest {
    Record(RecordCommand),
    CopyText(String),
}

fn parse_request(bytes: &[u8], limit: usize) -> Result<WireRequest> {
    validate_copy_text_limit(limit)?;
    let text = std::str::from_utf8(bytes).context("Control request must contain valid UTF-8")?;
    if let Some(text) = text.strip_prefix("copy\n") {
        validate_copy_text_size(text.len(), limit)?;
        return Ok(WireRequest::CopyText(text.to_owned()));
    }
    if bytes.len() > MAX_RECORD_FRAME_BYTES {
        bail!("Record control frame exceeds protocol allowance");
    }
    Ok(WireRequest::Record(RecordCommand::from_str(text)?))
}

#[cfg(target_os = "linux")]
#[path = "control/linux.rs"]
mod platform;

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::*;

    pub struct ControlServer;

    impl ControlServer {
        pub fn spawn(
            _tx: mpsc::Sender<ControlRequest>,
            _config_rx: watch::Receiver<Config>,
        ) -> Result<Option<Self>> {
            Ok(None)
        }
    }

    pub async fn send_record_command(_command: RecordCommand) -> Result<RecorderState> {
        bail!("`hyprwhspr-rs record` is currently only supported on Linux")
    }

    pub async fn send_copy_text(_text: &str, _max_copy_text_bytes: usize) -> Result<()> {
        bail!("`hyprwhspr-rs copy` is only supported on Linux")
    }
}

pub use platform::{send_copy_text, send_record_command, ControlServer};

fn parse_copy_response(raw: &str) -> Result<()> {
    if raw.trim() == "ok" {
        return Ok(());
    }
    if let Some(message) = raw.trim().strip_prefix("err ") {
        bail!(message.to_owned());
    }
    bail!("Copy acknowledgement missing or malformed; clipboard publication may have happened; do not retry");
}

fn parse_response(raw: &str) -> Result<RecorderState> {
    let body = raw.trim();
    if let Some(state) = body.strip_prefix("ok ") {
        return RecorderState::from_str(state);
    }
    if let Some(message) = body.strip_prefix("err ") {
        bail!(message.to_string());
    }
    bail!("Malformed control response: {body}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_state_strings() {
        assert_eq!(
            RecorderState::from_str("recording").unwrap(),
            RecorderState::Recording
        );
        assert_eq!(
            RecorderState::from_str("processing").unwrap(),
            RecorderState::Processing
        );
    }

    #[test]
    fn parses_command_strings() {
        assert_eq!(
            RecordCommand::from_str("start").unwrap(),
            RecordCommand::Start
        );
        assert_eq!(
            RecordCommand::from_str("stop").unwrap(),
            RecordCommand::Stop
        );
        assert_eq!(
            RecordCommand::from_str("toggle").unwrap(),
            RecordCommand::Toggle
        );
        assert_eq!(
            RecordCommand::from_str("status").unwrap(),
            RecordCommand::Status
        );
    }

    #[test]
    fn rejects_unknown_response() {
        assert!(parse_response("wat").is_err());
        assert!(parse_response("err nope").is_err());
    }

    #[test]
    fn copy_frame_preserves_empty_and_multiline_payloads() {
        assert_eq!(
            parse_request(b"copy\n", 32).unwrap(),
            WireRequest::CopyText(String::new())
        );
        assert_eq!(
            parse_request(b"copy\n\n\tend\n", 32).unwrap(),
            WireRequest::CopyText("\n\tend\n".into())
        );
        assert_eq!(
            parse_request(b"status\n", 1).unwrap(),
            WireRequest::Record(RecordCommand::Status)
        );
        assert!(parse_request(b"", 32).is_err());
        assert!(parse_request(b"copy\n\xff", 32).is_err());
    }

    #[test]
    fn copy_frame_enforces_byte_bound() {
        let mut bytes = COPY_PREFIX.to_vec();
        bytes.resize(4 + COPY_PREFIX.len(), b'x');
        assert!(parse_request(&bytes, 4).is_ok());
        bytes.push(b'x');
        assert!(parse_request(&bytes, 4)
            .unwrap_err()
            .to_string()
            .contains("max_copy_text_bytes limit of 4 bytes"));
    }

    #[test]
    fn copy_limit_counts_utf8_bytes_and_leaves_record_commands_independent() {
        let frame = "copy\n🙂".as_bytes();
        assert!(parse_request(frame, 4).is_ok());
        assert!(parse_request(frame, 3).is_err());
        for command in [b"start".as_slice(), b"stop\n", b"toggle", b"status\r\n"] {
            assert!(matches!(
                parse_request(command, 1).unwrap(),
                WireRequest::Record(_)
            ));
        }
    }

    #[tokio::test]
    async fn stdin_respects_configured_limit_and_preserves_literal_utf8() {
        let text = "🙂\n\t$`";
        assert_eq!(
            read_copy_text(text.as_bytes(), text.len()).await.unwrap(),
            text
        );
        assert!(read_copy_text(text.as_bytes(), text.len() - 1)
            .await
            .is_err());
        assert_eq!(read_copy_text(b"".as_slice(), 1).await.unwrap(), "");
        assert!(read_copy_text(b"\xff".as_slice(), 1).await.is_err());
        assert!(read_copy_text(b"x".as_slice(), 0).await.is_err());
        assert!(read_copy_text(b"x".as_slice(), usize::MAX).await.is_err());
    }

    #[test]
    fn copy_requires_publication_acknowledgement() {
        assert!(parse_copy_response("ok\n").is_ok());
        assert!(parse_copy_response("err clipboard unavailable\n").is_err());
        assert!(parse_copy_response("")
            .unwrap_err()
            .to_string()
            .contains("do not retry"));
        assert!(parse_copy_response("ok inactive\n").is_err());
    }
}
