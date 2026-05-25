//! Gated smoke test for a real whisper.cpp server.
//!
//! This test is ignored by default because it builds/runs an external server.
//! To run it cheaply against an existing checkout/server binary:
//!
//! ```text
//! HYPRWHSPR_WHISPER_CPP_SERVER=/tmp/whisper.cpp/build/bin/whisper-server \
//! HYPRWHSPR_WHISPER_CPP_MODEL=/path/to/ggml-tiny.en.bin \
//! cargo test --test custom_whisper_cpp_server -- --ignored
//! ```

use hyprwhspr_rs::config::{
    Config, CustomProviderConfig, CustomProviderKind, TranscriptionProvider, ValueSource,
};
use hyprwhspr_rs::transcription::CustomOpenAiTranscriber;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[tokio::test]
#[ignore = "requires whisper.cpp server binary and model env vars"]
async fn custom_provider_transcribes_against_whisper_cpp_server() {
    let server = std::env::var("HYPRWHSPR_WHISPER_CPP_SERVER")
        .expect("HYPRWHSPR_WHISPER_CPP_SERVER must point to whisper-server");
    let model = std::env::var("HYPRWHSPR_WHISPER_CPP_MODEL")
        .expect("HYPRWHSPR_WHISPER_CPP_MODEL must point to a ggml model");
    let port = std::env::var("HYPRWHSPR_WHISPER_CPP_PORT").unwrap_or_else(|_| "18080".to_string());
    let base_url = format!("http://127.0.0.1:{port}");

    let mut child = spawn_server(&server, &model, &port);
    wait_for_server(&base_url).await;

    let mut config = Config::default();
    config.transcription.provider = TranscriptionProvider::Custom("remote_whisper".to_string());
    config.transcription.custom.insert(
        "remote_whisper".to_string(),
        CustomProviderConfig {
            kind: CustomProviderKind::OpenAiAudioTranscriptions,
            label: Some("Remote whisper.cpp".to_string()),
            base_url: ValueSource {
                env: None,
                value: Some(base_url),
            },
            endpoint: "/v1/audio/transcriptions".to_string(),
            model: "whisper-large-v3".to_string(),
            audio_format: "wav".to_string(),
            api_key: Default::default(),
            headers: Default::default(),
            body: Default::default(),
            prompt: String::new(),
        },
    );

    let custom = config
        .transcription
        .custom
        .get("remote_whisper")
        .expect("custom provider");
    let transcriber = CustomOpenAiTranscriber::new(
        "remote_whisper",
        custom,
        Duration::from_secs(30),
        0,
        String::new(),
    )
    .expect("custom transcriber");

    let mut audio = Vec::new();
    for i in 0..16_000 {
        let t = i as f32 / 16_000.0;
        audio.push((t * 440.0 * std::f32::consts::TAU).sin() * 0.1);
    }

    let result = transcriber.transcribe(audio).await.expect("transcribe");
    assert!(result.text.trim().is_empty() || !result.text.contains("error"));

    let _ = child.kill();
}

fn spawn_server(server: &str, model: &str, port: &str) -> Child {
    Command::new(server)
        .args([
            "--host",
            "127.0.0.1",
            "--port",
            port,
            "--inference-path",
            "/v1/audio/transcriptions",
            "-m",
            model,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn whisper.cpp server")
}

async fn wait_for_server(base_url: &str) {
    let client = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(20);

    while Instant::now() < deadline {
        if client.get(base_url).send().await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    panic!("whisper.cpp server did not become ready");
}
