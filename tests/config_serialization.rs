use hyprwhspr_rs::Config;

#[test]
fn default_config_omits_infinite_max_speech_s() {
    let config = Config::default();
    let json = serde_json::to_string_pretty(&config).expect("serialize config");
    assert!(!json.contains("\"max_speech_s\""));
}

#[test]
fn null_max_speech_s_deserializes_to_default() {
    let json = r#"{"transcription":{"whisper_cpp":{"vad":{"max_speech_s":null}}}}"#;
    let config: Config = serde_json::from_str(json).expect("deserialize config");
    assert!(config
        .transcription
        .whisper_cpp
        .vad
        .max_speech_s
        .is_infinite());
}

#[test]
fn default_config_round_trips() {
    let config = Config::default();
    let json = serde_json::to_string_pretty(&config).expect("serialize config");
    let decoded: Config = serde_json::from_str(&json).expect("deserialize config");
    assert_eq!(decoded, config);
}

#[test]
fn default_models_dir_uses_xdg_data() {
    let config = Config::default();
    let expected = directories::ProjectDirs::from("", "", "hyprwhspr-rs")
        .map(|dirs| dirs.data_dir().join("models").to_string_lossy().to_string())
        .unwrap_or_else(|| "models".to_string());
    assert_eq!(
        config
            .transcription
            .whisper_cpp
            .models_dirs
            .first()
            .expect("models dir"),
        &expected
    );
}
