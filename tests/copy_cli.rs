//! Exercise real CLI stdin/config loading against a controlled control socket.
//! Native clipboard effects are covered by the delivery module's adapter tests.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::process::Command;

struct Fixture(PathBuf);

impl Fixture {
    fn new(limit: usize) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "hyprwhspr-copy-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let config_dir = root.join("config/hyprwhspr-rs");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(root.join("runtime/hyprwhspr-rs")).unwrap();
        std::fs::write(
            config_dir.join("config.jsonc"),
            serde_json::to_vec(&serde_json::json!({
                "max_copy_text_bytes": limit,
                // Copy must not initialize a model or audio capture.
                "transcription": {"whisper_cpp": {"model": "missing-test-model"}}
            }))
            .unwrap(),
        )
        .unwrap();
        Self(root)
    }

    fn listener(&self) -> UnixListener {
        UnixListener::bind(self.0.join("runtime/hyprwhspr-rs/control.sock")).unwrap()
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hyprwhspr-rs"));
        command
            .arg("copy")
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("XDG_RUNTIME_DIR", self.0.join("runtime"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // This directory contains only this test's config and socket fixture.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn accepted_copy(limit: usize, text: &str) {
    let fixture = Fixture::new(limit);
    let listener = fixture.listener();
    let expected = format!("copy\n{text}").into_bytes();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut frame = Vec::new();
        (&mut stream)
            .take((limit + b"copy\n".len() + 1) as u64)
            .read_to_end(&mut frame)
            .await
            .unwrap();
        assert_eq!(
            frame, expected,
            "CLI must send literal UTF-8 stdin unchanged"
        );
        stream.write_all(b"ok\n").await.unwrap();
        stream.shutdown().await.unwrap();
    });
    let mut child = fixture.command().spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let text = text.as_bytes().to_vec();
    let writer = tokio::spawn(async move {
        stdin.write_all(&text).await.unwrap();
        stdin.shutdown().await.unwrap();
    });
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    writer.await.unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    server.await.unwrap();
}

#[tokio::test]
async fn cli_accepts_configured_payload_above_old_one_mib_cap() {
    accepted_copy(2 * 1024 * 1024, &"x".repeat(1024 * 1024 + 1)).await;
}

#[tokio::test]
async fn cli_preserves_unicode_byte_boundary_and_empty_copy() {
    accepted_copy(4, "🙂").await;
    accepted_copy(1, "").await;
    let literal = "\n世界\t$(literal) `text`\n";
    accepted_copy(literal.len(), literal).await;
}

#[tokio::test]
async fn cli_rejects_small_limit_before_connecting_to_daemon() {
    let fixture = Fixture::new(3);
    let listener = fixture.listener();
    let mut child = fixture.command().spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all("🙂".as_bytes()).await.unwrap();
    stdin.shutdown().await.unwrap();
    drop(stdin);
    let output = child.wait_with_output().await.unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("max_copy_text_bytes limit of 3 bytes")
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(10), listener.accept())
            .await
            .is_err()
    );
}
