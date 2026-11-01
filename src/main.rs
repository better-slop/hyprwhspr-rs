use anyhow::Result;
use clap::Parser;
use hyprwhspr_rs::{
    cli::{Cli, Command, RecordAction},
    config::TranscriptionProvider,
    control::RecordCommand,
    install,
    logging::TextPipelineFormatter,
    send_record_command, ConfigManager, HyprwhsprApp,
};
use tokio::signal;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle install command before initializing logging (it has its own output)
    if let Some(command) = cli.command {
        match command {
            Command::Install(args) => return install::run_install(&args),
            Command::Record(args) => {
                let command = match args.action {
                    RecordAction::Start => RecordCommand::Start,
                    RecordAction::Stop => RecordCommand::Stop,
                    RecordAction::Toggle => RecordCommand::Toggle,
                    RecordAction::Status => RecordCommand::Status,
                };
                let state = send_record_command(command).await?;
                println!("{state}");
                return Ok(());
            }
            Command::Copy => {
                use hyprwhspr_rs::control::{read_copy_text, send_copy_text};
                // Loading config does not initialize audio, models, or input.
                let limit = ConfigManager::load()?.get().max_copy_text_bytes;
                let text = read_copy_text(tokio::io::stdin(), limit).await?;
                return send_copy_text(&text, limit).await;
            }
        }
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
    if cli.test {
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
    let mut app = HyprwhsprApp::new(config_manager)?;

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
    app.cleanup().await?;
    info!("✅ Shutdown complete");

    Ok(())
}

async fn run_test_mode() -> Result<()> {
    let config_manager = ConfigManager::load()?;
    config_manager.start_watching();
    hyprwhspr_rs::app_test::run(config_manager).await
}
