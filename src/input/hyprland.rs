use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::{env, path::PathBuf, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const IPC_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

pub(super) struct HyprlandDispatcher {
    socket_path: PathBuf,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub(super) struct ActiveWindow {
    pub address: String,
    pub class: String,
}

impl HyprlandDispatcher {
    pub(super) fn new() -> Option<Self> {
        let runtime_dir = env::var_os("XDG_RUNTIME_DIR")?;
        let signature = env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
        let socket_path = PathBuf::from(runtime_dir)
            .join("hypr")
            .join(signature)
            .join(".socket.sock");
        socket_path.exists().then_some(Self { socket_path })
    }

    pub(super) async fn send_paste_shortcut(&self, use_shift: bool) -> Result<()> {
        let modifiers = if use_shift { "ctrl shift" } else { "ctrl" };
        self.send_shortcut(modifiers, "v").await
    }

    pub(super) async fn send_global_paste_shortcut(&self) -> Result<()> {
        self.send_shortcut("shift", "Insert").await
    }

    async fn send_shortcut(&self, modifiers: &str, key: &str) -> Result<()> {
        let command = format!("dispatch sendshortcut {modifiers}, {key}, active");
        let response = self.send_command(&command).await?;
        // An empty/unsupported response is not an acknowledgement. It may still
        // follow a dispatched shortcut, so the delivery layer never retries it.
        if !response.eq_ignore_ascii_case("ok") {
            bail!("Hyprland did not acknowledge sendshortcut: {response:?}");
        }
        Ok(())
    }

    pub(super) async fn active_window(&self) -> Result<Option<ActiveWindow>> {
        parse_active_window(&self.send_command("j/activewindow").await?)
    }

    async fn send_command(&self, command: &str) -> Result<String> {
        self.send_bounded(command, IPC_TIMEOUT, MAX_RESPONSE_BYTES)
            .await
    }

    async fn send_bounded(
        &self,
        command: &str,
        timeout: Duration,
        max_response: usize,
    ) -> Result<String> {
        tokio::time::timeout(timeout, self.exchange(command, max_response))
            .await
            .context("Hyprland IPC deadline exceeded; command outcome may be uncertain")?
    }

    async fn exchange(&self, command: &str, max_response: usize) -> Result<String> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .with_context(|| {
                format!(
                    "Cannot connect to Hyprland at {}",
                    self.socket_path.display()
                )
            })?;
        stream
            .write_all(command.as_bytes())
            .await
            .context("Cannot write Hyprland IPC command")?;
        stream
            .shutdown()
            .await
            .context("Cannot finish Hyprland IPC command")?;
        let mut response = Vec::new();
        stream
            .take((max_response + 1) as u64)
            .read_to_end(&mut response)
            .await
            .context("Cannot read Hyprland IPC response")?;
        if response.len() > max_response {
            bail!("Hyprland IPC response exceeds {max_response} bytes");
        }
        Ok(String::from_utf8(response)
            .context("Invalid UTF-8 Hyprland IPC response")?
            .trim()
            .to_owned())
    }
}

fn parse_active_window(response: &str) -> Result<Option<ActiveWindow>> {
    let value: serde_json::Value =
        serde_json::from_str(response).context("Invalid Hyprland activewindow JSON")?;
    if value.as_object().is_some_and(|object| object.is_empty()) || value.is_null() {
        return Ok(None);
    }
    let window: ActiveWindow =
        serde_json::from_value(value).context("Hyprland activewindow lacks address/class")?;
    if window.address.is_empty() {
        bail!("Hyprland activewindow has an empty address");
    }
    Ok(Some(window))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_window_keeps_identity_for_focus_validation() {
        assert_eq!(
            parse_active_window(r#"{"address":"0x123","class":"foot","title":"shell"}"#).unwrap(),
            Some(ActiveWindow {
                address: "0x123".into(),
                class: "foot".into()
            })
        );
    }

    #[test]
    fn no_active_window_is_distinct_from_invalid_ipc() {
        assert!(parse_active_window("{}").unwrap().is_none());
        assert!(parse_active_window("unknown request").is_err());
        assert!(parse_active_window(r#"{"class":"foot"}"#).is_err());
    }

    struct TestSocket(PathBuf);

    impl TestSocket {
        fn bind() -> (Self, tokio::net::UnixListener) {
            static NEXT_SOCKET: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let id = NEXT_SOCKET.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("hyprwhspr-ipc-{}-{id}.sock", std::process::id()));
            let listener = tokio::net::UnixListener::bind(&path).unwrap();
            (Self(path), listener)
        }
    }

    impl Drop for TestSocket {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[tokio::test]
    async fn stalled_dispatch_times_out_with_uncertain_outcome() {
        let (socket, listener) = TestSocket::bind();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = String::new();
            stream.read_to_string(&mut request).await.unwrap();
            assert!(request.contains("sendshortcut"));
            std::future::pending::<()>().await;
            drop(stream);
        });
        let dispatcher = HyprlandDispatcher {
            socket_path: socket.0.clone(),
        };
        let error = dispatcher
            .send_bounded(
                "dispatch sendshortcut ctrl, v, active",
                Duration::from_millis(20),
                64,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("deadline exceeded"));
        server.abort();
    }

    #[tokio::test]
    async fn oversized_response_is_bounded_before_eof() {
        let (socket, listener) = TestSocket::bind();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            stream.read_to_end(&mut request).await.unwrap();
            stream.write_all(&[b'x'; 65]).await.unwrap();
            std::future::pending::<()>().await;
            drop(stream);
        });
        let dispatcher = HyprlandDispatcher {
            socket_path: socket.0.clone(),
        };
        let error = dispatcher
            .send_bounded("j/activewindow", Duration::from_secs(1), 64)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("exceeds 64 bytes"));
        server.abort();
    }
}
