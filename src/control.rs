use anyhow::{anyhow, bail, Context, Result};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use tokio::sync::{mpsc, oneshot};

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

/// Bound both allocation and text transport. The prefix is additional framing.
pub const MAX_COPY_TEXT_BYTES: usize = 1024 * 1024;
const COPY_PREFIX: &[u8] = b"copy\n";

#[derive(Debug, PartialEq, Eq)]
enum WireRequest {
    Record(RecordCommand),
    CopyText(String),
}

fn parse_request(bytes: &[u8]) -> Result<WireRequest> {
    if bytes.len() > MAX_COPY_TEXT_BYTES + COPY_PREFIX.len() {
        bail!("Control request exceeds 1 MiB text limit");
    }
    let text = std::str::from_utf8(bytes).context("Control request must contain valid UTF-8")?;
    if let Some(text) = text.strip_prefix("copy\n") {
        return Ok(WireRequest::CopyText(text.to_owned()));
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
        pub fn spawn(_tx: mpsc::Sender<ControlRequest>) -> Result<Option<Self>> {
            Ok(None)
        }
    }

    pub async fn send_record_command(_command: RecordCommand) -> Result<RecorderState> {
        bail!("`hyprwhspr-rs record` is currently only supported on Linux")
    }

    pub async fn send_copy_text(_text: &str) -> Result<()> {
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
            parse_request(b"copy\n").unwrap(),
            WireRequest::CopyText(String::new())
        );
        assert_eq!(
            parse_request(b"copy\n\n\tend\n").unwrap(),
            WireRequest::CopyText("\n\tend\n".into())
        );
        assert_eq!(
            parse_request(b"status\n").unwrap(),
            WireRequest::Record(RecordCommand::Status)
        );
        assert!(parse_request(b"").is_err());
        assert!(parse_request(b"copy\n\xff").is_err());
    }

    #[test]
    fn copy_frame_enforces_byte_bound() {
        let mut bytes = COPY_PREFIX.to_vec();
        bytes.resize(MAX_COPY_TEXT_BYTES + COPY_PREFIX.len(), b'x');
        assert!(parse_request(&bytes).is_ok());
        bytes.push(b'x');
        assert!(parse_request(&bytes)
            .unwrap_err()
            .to_string()
            .contains("1 MiB"));
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
