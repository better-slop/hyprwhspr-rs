//! Bounded EOF-framed Unix transport for recorder and clipboard requests.

use super::*;
use std::env;
use std::fs;
use std::io::ErrorKind;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;

pub struct ControlServer {
    path: PathBuf,
    task: JoinHandle<()>,
}

impl ControlServer {
    pub fn spawn(
        tx: mpsc::Sender<ControlRequest>,
        config_rx: watch::Receiver<Config>,
    ) -> Result<Option<Self>> {
        let path = control_socket_path()?;
        prepare_socket_path(&path)?;
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("Failed to bind control socket at {}", path.display()))?;

        let task = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let request_tx = tx.clone();
                        let request_config = config_rx.clone();
                        tokio::spawn(async move {
                            if let Err(err) =
                                handle_client(stream, request_tx, request_config).await
                            {
                                tracing::warn!("Control client error: {err:#}");
                            }
                        });
                    }
                    Err(err) => {
                        tracing::error!("Control socket accept failed: {err}");
                        break;
                    }
                }
            }
        });

        Ok(Some(Self { path, task }))
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.task.abort();
        if let Err(err) = fs::remove_file(&self.path) {
            if err.kind() != ErrorKind::NotFound {
                tracing::warn!(
                    "Failed to remove control socket {}: {}",
                    self.path.display(),
                    err
                );
            }
        }
    }
}

pub async fn send_record_command(command: RecordCommand) -> Result<RecorderState> {
    parse_response(&exchange_request(command.as_str().as_bytes()).await?)
}

/// Apply the client's loaded limit before connecting. The daemon independently
/// admits the request using its current config snapshot, which is authoritative.
pub async fn send_copy_text(text: &str, max_copy_text_bytes: usize) -> Result<()> {
    validate_copy_text_size(text.len(), max_copy_text_bytes)?;
    let mut request = Vec::with_capacity(COPY_PREFIX.len() + text.len());
    request.extend_from_slice(COPY_PREFIX);
    request.extend_from_slice(text.as_bytes());
    let response = exchange_request(&request).await.context(
        "Copy acknowledgement failed; publication may have happened; do not retry automatically",
    )?;
    parse_copy_response(&response)
}

async fn exchange_request(request: &[u8]) -> Result<String> {
    let path = control_socket_path()?;
    let mut stream = UnixStream::connect(&path).await.with_context(|| {
        format!(
            "Failed to connect to hyprwhspr-rs control socket at {}. Start hyprwhspr-rs first.",
            path.display()
        )
    })?;

    stream
        .write_all(request)
        .await
        .context("Failed to send control command")?;
    stream
        .shutdown()
        .await
        .context("Failed to finish control command write")?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .context("Failed to read control response")?;

    String::from_utf8(response).context("Control response must contain valid UTF-8")
}

async fn handle_client(
    mut stream: UnixStream,
    tx: mpsc::Sender<ControlRequest>,
    config_rx: watch::Receiver<Config>,
) -> Result<()> {
    // Take one coherent admission/framing policy for this request. New requests
    // see watcher updates even while the recording session defers other config.
    let limit = config_rx.borrow().max_copy_text_bytes;
    let mut request = Vec::new();
    (&mut stream)
        .take(request_read_limit(limit)?)
        .read_to_end(&mut request)
        .await
        .context("Failed to read control request")?;

    let response = match parse_request(&request, limit) {
        Ok(WireRequest::Record(command)) => {
            let (reply_tx, reply_rx) = oneshot::channel();
            tx.send(ControlRequest::Record { command, reply_tx })
                .await
                .context("Control channel closed")?;
            match reply_rx.await {
                Ok(Ok(state)) => format!("ok {state}\n"),
                Ok(Err(message)) => format!("err {message}\n"),
                Err(_) => "err control handler dropped response\n".to_string(),
            }
        }
        Ok(WireRequest::CopyText(text)) => {
            let (reply_tx, reply_rx) = oneshot::channel();
            tx.send(ControlRequest::CopyText { text, reply_tx })
                .await
                .context("Control channel closed")?;
            match reply_rx.await {
                Ok(Ok(())) => "ok\n".to_owned(),
                Ok(Err(message)) => format!("err {message}\n"),
                Err(_) => "err copy handler dropped response; clipboard publication may have happened; do not retry\n".to_owned(),
            }
        }
        Err(error) => format!("err {error:#}\n"),
    };

    stream
        .write_all(response.as_bytes())
        .await
        .context("Failed to write control response")?;
    stream
        .shutdown()
        .await
        .context("Failed to finish control response write")?;

    Ok(())
}

fn control_socket_path() -> Result<PathBuf> {
    let base = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    let dir = base.join("hyprwhspr-rs");
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create control directory {}", dir.display()))?;
    Ok(dir.join("control.sock"))
}

fn prepare_socket_path(path: &PathBuf) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => bail!(
            "Another hyprwhspr-rs instance is already serving control commands at {}",
            path.display()
        ),
        Err(_) => {
            fs::remove_file(path).with_context(|| {
                format!("Failed to remove stale control socket {}", path.display())
            })?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured_limit(limit: usize) -> watch::Receiver<Config> {
        let mut config = Config::default();
        config.max_copy_text_bytes = limit;
        watch::channel(config).1
    }

    #[tokio::test]
    async fn copy_frame_waits_for_eof_and_preserves_text_until_acknowledged() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let (tx, mut rx) = mpsc::channel(1);
        let handler = tokio::spawn(handle_client(server, tx, configured_limit(128)));
        let text = "\n世界\t$(touch /tmp/not-a-command) `literal`\n";
        client.write_all(COPY_PREFIX).await.unwrap();
        client.write_all(text.as_bytes()).await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), rx.recv())
                .await
                .is_err()
        );
        client.shutdown().await.unwrap();
        match rx.recv().await.unwrap() {
            ControlRequest::CopyText {
                text: received,
                reply_tx,
            } => {
                assert_eq!(received, text);
                reply_tx.send(Ok(())).unwrap();
            }
            _ => panic!("copy must not become a recorder command"),
        }
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        assert_eq!(response, "ok\n");
        handler.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn rejected_frame_never_reaches_daemon_effects() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let (tx, mut rx) = mpsc::channel(1);
        let handler = tokio::spawn(handle_client(server, tx, configured_limit(128)));
        client.write_all(b"copy\n\xff").await.unwrap();
        client.shutdown().await.unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("err "));
        assert!(response.contains("valid UTF-8"));
        handler.await.unwrap().unwrap();
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn configured_large_payload_reaches_daemon_without_old_cap() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let limit = 2 * 1024 * 1024;
        let text = "x".repeat(1024 * 1024 + 1);
        let (tx, mut rx) = mpsc::channel(1);
        let handler = tokio::spawn(handle_client(server, tx, configured_limit(limit)));
        client.write_all(COPY_PREFIX).await.unwrap();
        client.write_all(text.as_bytes()).await.unwrap();
        client.shutdown().await.unwrap();
        match rx.recv().await.unwrap() {
            ControlRequest::CopyText {
                text: received,
                reply_tx,
            } => {
                assert_eq!(received, text);
                reply_tx.send(Ok(())).unwrap();
            }
            _ => panic!("expected clipboard-only request"),
        }
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        assert_eq!(response, "ok\n");
        handler.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn daemon_rejects_small_limit_before_forwarding_effects() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let (tx, mut rx) = mpsc::channel(1);
        let handler = tokio::spawn(handle_client(server, tx, configured_limit(3)));
        client.write_all("copy\n🙂".as_bytes()).await.unwrap();
        client.shutdown().await.unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        assert!(response.contains("max_copy_text_bytes limit of 3 bytes"));
        handler.await.unwrap().unwrap();
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn request_start_uses_latest_watched_limit_and_records_ignore_tiny_copy_limit() {
        let mut config = Config::default();
        config.max_copy_text_bytes = 1;
        let (config_tx, config_rx) = watch::channel(config.clone());
        config.max_copy_text_bytes = 4;
        config_tx.send(config.clone()).unwrap();
        let (mut client, server) = UnixStream::pair().unwrap();
        let (tx, mut rx) = mpsc::channel(1);
        let handler = tokio::spawn(handle_client(server, tx, config_rx.clone()));
        client.write_all("copy\n🙂".as_bytes()).await.unwrap();
        client.shutdown().await.unwrap();
        match rx.recv().await.unwrap() {
            ControlRequest::CopyText { reply_tx, .. } => {
                reply_tx.send(Ok(())).unwrap();
            }
            _ => panic!("updated limit must admit four-byte payload"),
        }
        handler.await.unwrap().unwrap();

        config.max_copy_text_bytes = 1;
        config_tx.send(config).unwrap();
        let (mut client, server) = UnixStream::pair().unwrap();
        let (tx, mut rx) = mpsc::channel(1);
        let handler = tokio::spawn(handle_client(server, tx, config_rx));
        client.write_all(b"status\n").await.unwrap();
        client.shutdown().await.unwrap();
        match rx.recv().await.unwrap() {
            ControlRequest::Record { command, reply_tx } => {
                assert_eq!(command, RecordCommand::Status);
                reply_tx.send(Ok(RecorderState::Inactive)).unwrap();
            }
            _ => panic!("tiny copy limit must not reject record commands"),
        }
        handler.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn client_rejects_invalid_or_oversized_limits_before_connecting() {
        assert!(send_copy_text("🙂", 3)
            .await
            .unwrap_err()
            .to_string()
            .contains("limit of 3 bytes"));
        assert!(send_copy_text("", 0)
            .await
            .unwrap_err()
            .to_string()
            .contains("max_copy_text_bytes must be"));
        assert!(send_copy_text("", usize::MAX)
            .await
            .unwrap_err()
            .to_string()
            .contains("max_copy_text_bytes must be"));
    }
}
