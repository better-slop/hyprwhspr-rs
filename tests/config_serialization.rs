use hyprwhspr_rs::input::DeliveryIntent;
use hyprwhspr_rs::Config;

#[test]
fn released_clipboard_option_accepts_both_boolean_values() {
    for value in [false, true] {
        let config: Config =
            serde_json::from_value(serde_json::json!({"auto_copy_clipboard": value}))
                .expect("released clipboard option must load without migration");
        assert_eq!(config.auto_copy_clipboard, value);
        assert_eq!(
            config.dictation_delivery_intent(),
            if value {
                DeliveryIntent::Paste
            } else {
                DeliveryIntent::Type
            }
        );
        let serialized = serde_json::to_value(config).unwrap();
        assert_eq!(serialized["auto_copy_clipboard"], value);
    }
    assert!(
        serde_json::from_value::<Config>(serde_json::json!({"auto_copy_clipboard": "false"}))
            .is_err()
    );
}

#[test]
fn default_config_preserves_released_clipboard_option() {
    for config in [
        Config::default(),
        serde_json::from_str::<Config>("{}").unwrap(),
    ] {
        assert_eq!(config.dictation_delivery_intent(), DeliveryIntent::Paste);
        let serialized = serde_json::to_value(config).unwrap();
        assert_eq!(serialized["auto_copy_clipboard"], true);
        assert!(serialized.get("delivery_mode").is_none());
    }
    assert!(serde_json::from_str::<Config>(
        r#"{"$schema":"https://example.test/schema.json","auto_copy_clipboard":false}"#
    )
    .is_ok());
}

#[test]
fn generated_schema_preserves_released_clipboard_option() {
    let schema: serde_json::Value =
        serde_json::from_str(&hyprwhspr_rs::config::generated_schema_json().unwrap()).unwrap();
    assert_eq!(
        schema.pointer("/properties/auto_copy_clipboard/type"),
        Some(&serde_json::json!("boolean"))
    );
    assert_eq!(
        schema.pointer("/properties/auto_copy_clipboard/default"),
        Some(&serde_json::json!(true))
    );
    assert!(schema.pointer("/properties/delivery_mode").is_none());
}

#[test]
fn checked_in_schema_is_current() {
    let generated = format!(
        "{}\n",
        hyprwhspr_rs::config::generated_schema_json().expect("generate schema")
    );
    let checked_in = include_str!("../config/schema.json");

    assert_eq!(checked_in, generated);
}

#[test]
fn schema_allows_null_vad_max_speech_s() {
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../config/schema.json")).expect("parse schema");
    let max_speech_s = schema
        .pointer("/$defs/VadConfig/properties/max_speech_s/type")
        .expect("max_speech_s type schema");

    assert_eq!(
        max_speech_s,
        &serde_json::json!(["number", "null"]),
        "schema should match runtime deserialization, where max_speech_s: null means unlimited"
    );
}

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
fn default_config_includes_models_dir() {
    let config = Config::default();

    assert_eq!(
        config.transcription.whisper_cpp.models_dirs,
        vec!["~/.local/share/hyprwhspr-rs/models".to_string()]
    );
}

#[test]
fn hold_shortcut_only_disables_press_shortcut() {
    let json = r#"{"shortcuts":{"hold":"SUPER+R"}}"#;
    let mut config: Config = serde_json::from_str(json).expect("deserialize config");
    config.normalize_shortcuts();

    assert_eq!(config.shortcuts.hold.as_deref(), Some("SUPER+R"));
    assert_eq!(config.shortcuts.press, None);
}

#[test]
fn legacy_primary_shortcut_populates_press_even_with_hold() {
    let json = r#"{"primary_shortcut":"SUPER+SHIFT+R","shortcuts":{"hold":"SUPER+R"}}"#;
    let mut config: Config = serde_json::from_str(json).expect("deserialize config");
    config.normalize_shortcuts();

    assert_eq!(config.shortcuts.hold.as_deref(), Some("SUPER+R"));
    assert_eq!(config.shortcuts.press.as_deref(), Some("SUPER+SHIFT+R"));
}

#[test]
fn paste_hints_shift_insert_deserializes() {
    let json = r#"{
        "paste_hints":{
            "shift":["zed"],
            "shift_insert":["dev.zed.Zed"]
        }
    }"#;
    let config: Config = serde_json::from_str(json).expect("deserialize config");
    assert_eq!(config.paste_hints.shift, vec!["zed".to_string()]);
    assert_eq!(
        config.paste_hints.shift_insert,
        vec!["dev.zed.Zed".to_string()]
    );
}
