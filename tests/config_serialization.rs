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
fn config_with_only_hold_shortcut_should_not_have_press_shortcut() {
    // Test that when a user only specifies a hold shortcut, no press shortcut is added
    let json = r#"{"shortcuts": {"hold": "SUPER+R"}}"#;
    let config: Config = serde_json::from_str(json).expect("deserialize config");
    
    // The hold shortcut should be set
    assert_eq!(config.hold_shortcut(), Some("SUPER+R"));
    
    // The press shortcut should NOT be set (should be None)
    assert_eq!(config.press_shortcut(), None);
}

#[test]
fn config_with_only_press_shortcut_should_not_have_hold_shortcut() {
    // Test that when a user only specifies a press shortcut, no hold shortcut is added
    let json = r#"{"shortcuts": {"press": "SUPER+ALT+R"}}"#;
    let config: Config = serde_json::from_str(json).expect("deserialize config");
    
    // The press shortcut should be set
    assert_eq!(config.press_shortcut(), Some("SUPER+ALT+R"));
    
    // The hold shortcut should NOT be set (should be None)
    assert_eq!(config.hold_shortcut(), None);
}

#[test]
fn config_with_no_shortcuts_should_not_have_default_press_shortcut() {
    // When loading a config from disk with no shortcuts, we should respect that
    // and not add any default shortcuts
    let json = r#"{}"#;
    let config: Config = serde_json::from_str(json).expect("deserialize config");
    
    // Should NOT have a press shortcut
    assert_eq!(config.press_shortcut(), None);
    
    // Should NOT have a hold shortcut
    assert_eq!(config.hold_shortcut(), None);
}

#[test]
fn default_config_should_have_default_press_shortcut() {
    // When creating a brand new config via Config::default() (e.g., when no config
    // file exists), it should have a default press shortcut
    let config = Config::default();
    
    // Default should have a press shortcut
    assert_eq!(config.press_shortcut(), Some("SUPER+ALT+R"));
    
    // Default should NOT have a hold shortcut
    assert_eq!(config.hold_shortcut(), None);
}
