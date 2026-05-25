use hyprwhspr_rs::config::{
    Config, CustomProviderConfig, CustomProviderKind, SecretSource, TranscriptionProvider,
    ValueSource,
};
use hyprwhspr_rs::transcription::CustomOpenAiTranscriber;
use std::time::Duration;

#[test]
fn custom_provider_config_deserializes_requested_shape() {
    let json = r#"{
        "transcription": {
            "provider": "custom.remote_whisper",
            "custom": {
                "remote_whisper": {
                    "kind": "openai_audio_transcriptions",
                    "label": "Remote whisper.cpp",
                    "base_url": {
                        "env": "HYPRWHSPR_REMOTE_WHISPER_BASE_URL",
                        "value": "http://localhost:8080"
                    },
                    "endpoint": "/v1/audio/transcriptions",
                    "model": "whisper-large-v3",
                    "api_key": {
                        "env": "HYPRWHSPR_REMOTE_WHISPER_API_KEY",
                        "file": "/run/secrets/hyprwhspr-remote-key",
                        "file_env": "HYPRWHSPR_REMOTE_WHISPER_API_KEY_FILE"
                    },
                    "headers": {
                        "x-provider": "whisper.cpp"
                    },
                    "body": {
                        "temperature": "0",
                        "response_format": "json"
                    },
                    "prompt": "Transcribe technical notes."
                }
            }
        }
    }"#;

    let config: Config = serde_json::from_str(json).expect("deserialize config");

    assert_eq!(
        config.transcription.provider,
        TranscriptionProvider::Custom("remote_whisper".to_string())
    );

    let custom = config
        .transcription
        .custom
        .get("remote_whisper")
        .expect("custom provider");

    assert_eq!(custom.kind, CustomProviderKind::OpenAiAudioTranscriptions);
    assert_eq!(custom.label.as_deref(), Some("Remote whisper.cpp"));
    assert_eq!(
        custom.base_url,
        ValueSource {
            env: Some("HYPRWHSPR_REMOTE_WHISPER_BASE_URL".to_string()),
            value: Some("http://localhost:8080".to_string())
        }
    );
    assert_eq!(custom.endpoint, "/v1/audio/transcriptions");
    assert_eq!(custom.model, "whisper-large-v3");
    assert_eq!(
        custom.api_key,
        SecretSource {
            env: Some("HYPRWHSPR_REMOTE_WHISPER_API_KEY".to_string()),
            file: Some("/run/secrets/hyprwhspr-remote-key".to_string()),
            file_env: Some("HYPRWHSPR_REMOTE_WHISPER_API_KEY_FILE".to_string())
        }
    );
    assert_eq!(
        custom.headers.get("x-provider").map(String::as_str),
        Some("whisper.cpp")
    );
    assert_eq!(
        custom.body.get("temperature").map(String::as_str),
        Some("0")
    );
    assert_eq!(custom.prompt, "Transcribe technical notes.");
}

#[test]
fn custom_provider_round_trips() {
    let mut config = Config::default();
    config.transcription.provider = TranscriptionProvider::Custom("remote_whisper".to_string());
    config.transcription.custom.insert(
        "remote_whisper".to_string(),
        hyprwhspr_rs::config::CustomProviderConfig {
            kind: CustomProviderKind::OpenAiAudioTranscriptions,
            label: Some("Remote whisper.cpp".to_string()),
            base_url: ValueSource {
                env: Some("HYPRWHSPR_REMOTE_WHISPER_BASE_URL".to_string()),
                value: Some("http://localhost:8080".to_string()),
            },
            endpoint: "/v1/audio/transcriptions".to_string(),
            model: "whisper-large-v3".to_string(),
            audio_format: "wav".to_string(),
            api_key: SecretSource {
                env: Some("HYPRWHSPR_REMOTE_WHISPER_API_KEY".to_string()),
                file: None,
                file_env: Some("HYPRWHSPR_REMOTE_WHISPER_API_KEY_FILE".to_string()),
            },
            headers: [("x-provider".to_string(), "whisper.cpp".to_string())].into(),
            body: [("response_format".to_string(), "json".to_string())].into(),
            prompt: "Transcribe technical notes.".to_string(),
        },
    );

    let json = serde_json::to_string_pretty(&config).expect("serialize config");
    let decoded: Config = serde_json::from_str(&json).expect("deserialize config");

    assert_eq!(decoded, config);
}

#[test]
fn custom_provider_allows_absolute_endpoint_without_base_url() {
    let config = CustomProviderConfig {
        kind: CustomProviderKind::OpenAiAudioTranscriptions,
        label: Some("Fixed URL".to_string()),
        base_url: ValueSource::default(),
        endpoint: "http://127.0.0.1:18080/v1/audio/transcriptions".to_string(),
        model: "whisper-large-v3".to_string(),
        audio_format: "wav".to_string(),
        api_key: SecretSource::default(),
        headers: Default::default(),
        body: Default::default(),
        prompt: String::new(),
    };

    CustomOpenAiTranscriber::new("fixed_url", &config, Duration::from_secs(5), 0, String::new())
        .expect("absolute endpoint should not require base_url");
}
