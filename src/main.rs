use anyhow::{Context, Result};
use hyprwhspr_rs::{
    config::TranscriptionProvider, logging::TextPipelineFormatter, ConfigManager, HyprwhsprApp,
};
use std::env;
use std::path::PathBuf;
use std::process::Command;
use tokio::signal;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    // Handle install command before initializing logging (it has its own output)
    if args.len() > 1 && args[1] == "install" {
        return run_install(&args[2..]);
    }

    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hyprwhspr=info".into()),
        )
        .with(tracing_subscriber::fmt::layer().event_format(TextPipelineFormatter::new()))
        .init();

    // Check for test mode
    let test_mode = args.contains(&"--test".to_string());

    if test_mode {
        return run_test_mode().await;
    }

    info!("🚀 hyprwhspr-rs starting up!");
    info!("{}", "=".repeat(50));

    // Load configuration
    let config_manager = ConfigManager::load()?;
    config_manager.start_watching();
    let config = config_manager.get();
    info!("✅ Configuration loaded");
    info!(
        "   Transcription backend: {}",
        config.transcription.provider.label()
    );
    if matches!(
        config.transcription.provider,
        TranscriptionProvider::WhisperCpp
    ) {
        info!("   Model: {}", config.transcription.whisper_cpp.model);
    }
    if let Some(shortcut) = config.press_shortcut() {
        info!("   Press shortcut: {}", shortcut);
    } else {
        info!("   Press shortcut: disabled");
    }
    if let Some(shortcut) = config.hold_shortcut() {
        info!("   Hold shortcut: {}", shortcut);
    } else {
        info!("   Hold shortcut: disabled");
    }
    info!("   Audio feedback: {}", config.audio_feedback);

    // Initialize application
    let app = HyprwhsprApp::new(config_manager)?;

    // Set up signal handling
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

    #[cfg(unix)]
    {
        tokio::spawn(async move {
            let ctrl_c = signal::ctrl_c();
            let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("Failed to set up SIGTERM handler");

            tokio::select! {
                _ = ctrl_c => {
                    info!("Received SIGINT (Ctrl+C)");
                }
                _ = sigterm.recv() => {
                    info!("Received SIGTERM");
                }
            }

            let _ = shutdown_tx.send(());
        });
    }

    #[cfg(not(unix))]
    {
        tokio::spawn(async move {
            signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
            info!("Received SIGINT (Ctrl+C)");
            let _ = shutdown_tx.send(());
        });
    }

    // Run app until shutdown signal
    tokio::select! {
        result = app.run() => {
            if let Err(e) = result {
                info!("App error: {}", e);
            }
        }
        _ = shutdown_rx => {
            info!("Shutdown signal received");
        }
    }

    // Cleanup
    info!("🛑 Shutting down hyprwhspr-rs...");
    info!("✅ Shutdown complete");

    Ok(())
}

async fn run_test_mode() -> Result<()> {
    use hyprwhspr_rs::app_test::HyprwhsprAppTest;
    use tokio::io::{AsyncBufReadExt, BufReader};

    info!("🧪 Test Mode - Press Enter to toggle recording, Ctrl+C to quit");
    info!("{}", "=".repeat(50));

    // Load configuration
    let config_manager = ConfigManager::load()?;
    config_manager.start_watching();
    let mut config_rx = config_manager.subscribe();
    let config = config_manager.get();
    info!("✅ Configuration loaded");
    info!("   Model: {}", config.transcription.whisper_cpp.model);
    info!("   Audio feedback: {}", config.audio_feedback);

    // Initialize application
    let mut app = HyprwhsprAppTest::new(config_manager)?;

    info!("");
    info!("📝 Instructions:");
    info!("   1. Press Enter to START recording");
    info!("   2. Speak something");
    info!("   3. Press Enter to STOP recording");
    info!("   4. Text will be transcribed and injected");
    info!("");

    // Set up stdin reader
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();

    // Set up signal handling
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
        info!("Received SIGINT (Ctrl+C)");
        let _ = shutdown_tx.send(());
    });

    // Main loop
    loop {
        tokio::select! {
            line = reader.next_line() => {
                match line {
                    Ok(Some(_)) => {
                        // Toggle recording on Enter
                        if let Err(e) = app.toggle_recording().await {
                            info!("Error: {}", e);
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        info!("Error reading input: {}", e);
                        break;
                    }
                }
            }
            result = config_rx.changed() => {
                match result {
                    Ok(()) => {
                        let updated = config_rx.borrow().clone();
                        if let Err(err) = app.apply_config_update(updated) {
                            info!("Failed to apply config update: {}", err);
                        }
                    }
                    Err(_) => {
                        info!("Configuration watcher closed");
                        break;
                    }
                }
            }
            _ = &mut shutdown_rx => {
                info!("Shutdown signal received");
                break;
            }
        }
    }

    // Cleanup
    info!("🛑 Shutting down test mode...");
    app.cleanup().await?;
    info!("✅ Shutdown complete");

    Ok(())
}

fn run_install(args: &[String]) -> Result<()> {
    // Find the install script relative to the executable or in known locations
    let script_path = find_install_script()?;

    let mut cmd = Command::new("bash");
    cmd.arg(&script_path);

    // Pass through any arguments (e.g., --with-elephant)
    for arg in args {
        cmd.arg(arg);
    }

    // Set HYPRWHSPR_INSTALL_DIR so the script knows where to find config files
    if let Some(parent) = script_path.parent().and_then(|p| p.parent()) {
        cmd.env("HYPRWHSPR_INSTALL_DIR", parent);
    }

    let status = cmd.status().context("Failed to execute install script")?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("Install script exited with status: {}", status);
    }
}

fn find_install_script() -> Result<PathBuf> {
    // Try multiple locations in order of preference

    // 1. Relative to the executable (for installed binary)
    if let Ok(exe_path) = env::current_exe() {
        // Check ../share/hyprwhspr-rs/scripts/install-waybar.sh
        let share_path = exe_path
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("share/hyprwhspr-rs/scripts/install-waybar.sh"));
        if let Some(path) = share_path {
            if path.exists() {
                return Ok(path);
            }
        }

        // Check ../scripts/install-waybar.sh (dev layout)
        let dev_path = exe_path
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("scripts/install-waybar.sh"));
        if let Some(path) = dev_path {
            if path.exists() {
                return Ok(path);
            }
        }
    }

    // 2. Check in repo root (for cargo run)
    let cargo_manifest = env::var("CARGO_MANIFEST_DIR").ok();
    if let Some(manifest_dir) = cargo_manifest {
        let path = PathBuf::from(manifest_dir).join("scripts/install-waybar.sh");
        if path.exists() {
            return Ok(path);
        }
    }

    // 3. Check current working directory
    let cwd_path = PathBuf::from("scripts/install-waybar.sh");
    if cwd_path.exists() {
        return Ok(cwd_path);
    }

    // 4. Check XDG data dirs
    let data_home = env::var("XDG_DATA_HOME")
        .unwrap_or_else(|_| format!("{}/.local/share", env::var("HOME").unwrap_or_default()));
    let xdg_path = PathBuf::from(data_home).join("hyprwhspr-rs/scripts/install-waybar.sh");
    if xdg_path.exists() {
        return Ok(xdg_path);
    }

    anyhow::bail!(
        "Could not find install-waybar.sh script. \
        Make sure you're running from the hyprwhspr-rs directory or the script is installed."
    )
}
