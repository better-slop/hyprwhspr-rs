use hyprwhspr_rs::config::{SecretSource, SubscriptionAuthSource, ValueSource};

#[test]
fn value_source_prefers_non_empty_env_over_config_value() {
    std::env::set_var("HYPRWHSPR_TEST_BASE_URL_PREFERS_ENV", "http://remote:8080");

    let source = ValueSource {
        env: Some("HYPRWHSPR_TEST_BASE_URL_PREFERS_ENV".to_string()),
        value: Some("http://localhost:8080".to_string()),
    };

    assert_eq!(
        source.resolve("base_url").expect("resolved base_url"),
        "http://remote:8080"
    );

    std::env::remove_var("HYPRWHSPR_TEST_BASE_URL_PREFERS_ENV");
}

#[test]
fn value_source_uses_config_value_when_env_is_unset() {
    std::env::remove_var("HYPRWHSPR_TEST_BASE_URL_USES_VALUE");

    let source = ValueSource {
        env: Some("HYPRWHSPR_TEST_BASE_URL_USES_VALUE".to_string()),
        value: Some("http://localhost:8080".to_string()),
    };

    assert_eq!(
        source.resolve("base_url").expect("resolved base_url"),
        "http://localhost:8080"
    );
}

#[test]
fn secret_source_prefers_file_env_then_file_then_env() {
    let root =
        std::env::temp_dir().join(format!("hyprwhspr-rs-secret-source-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create temp dir");
    let file_env_path = root.join("file-env-secret");
    let file_path = root.join("file-secret");
    std::fs::write(&file_env_path, "from-file-env\n").expect("write file env secret");
    std::fs::write(&file_path, "from-file\n").expect("write file secret");

    std::env::set_var("HYPRWHSPR_TEST_API_KEY_FILE", &file_env_path);
    std::env::set_var("HYPRWHSPR_TEST_API_KEY", "from-env");

    let source = SecretSource {
        env: Some("HYPRWHSPR_TEST_API_KEY".to_string()),
        file: Some(file_path.to_string_lossy().into_owned()),
        file_env: Some("HYPRWHSPR_TEST_API_KEY_FILE".to_string()),
    };

    assert_eq!(
        source.resolve("api_key").expect("resolved api_key"),
        Some("from-file-env".to_string())
    );

    std::env::remove_var("HYPRWHSPR_TEST_API_KEY_FILE");
    assert_eq!(
        source.resolve("api_key").expect("resolved api_key"),
        Some("from-file".to_string())
    );

    let env_only_source = SecretSource {
        env: Some("HYPRWHSPR_TEST_API_KEY".to_string()),
        file: None,
        file_env: None,
    };
    assert_eq!(
        env_only_source
            .resolve("api_key")
            .expect("resolved api_key"),
        Some("from-env".to_string())
    );

    std::env::remove_var("HYPRWHSPR_TEST_API_KEY");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn subscription_auth_source_reads_codex_auth_json_access_token() {
    let root = std::env::temp_dir().join(format!(
        "hyprwhspr-rs-subscription-source-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("create temp dir");
    let auth_path = root.join("auth.json");
    std::fs::write(
        &auth_path,
        r#"{
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": "from-codex-auth",
                "refresh_token": "refresh-token"
            }
        }"#,
    )
    .expect("write auth json");

    let source = SubscriptionAuthSource {
        file: Some(auth_path.to_string_lossy().into_owned()),
        ..Default::default()
    };

    assert_eq!(
        source
            .resolve("subscription")
            .expect("resolved subscription token"),
        Some("from-codex-auth".to_string())
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn subscription_auth_source_uses_custom_json_pointer() {
    let root = std::env::temp_dir().join(format!(
        "hyprwhspr-rs-subscription-source-pointer-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("create temp dir");
    let auth_path = root.join("auth.json");
    std::fs::write(&auth_path, r#"{"nested":{"token":"from-pointer"}}"#).expect("write auth json");

    let source = SubscriptionAuthSource {
        file: Some(auth_path.to_string_lossy().into_owned()),
        json_pointer: Some("/nested/token".to_string()),
        ..Default::default()
    };

    assert_eq!(
        source
            .resolve("subscription")
            .expect("resolved subscription token"),
        Some("from-pointer".to_string())
    );

    let _ = std::fs::remove_dir_all(&root);
}
