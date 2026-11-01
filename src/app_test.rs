//! Interactive stdin activation host for `--test`.
//!
//! This is a real dictation caller: capture, ASR, normalization, history,
//! delivery, benchmark, and status all run through `DictationSession`.

use anyhow::Result;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{error, info};

use crate::config::{Config, ConfigManager};
use crate::session::{native::SessionFactory, Command, State, Trigger};

pub async fn run(config_manager: ConfigManager) -> Result<()> {
    let mut config_rx = config_manager.subscribe();
    let mut config = config_manager.get();
    let mut pending_config: Option<Config> = None;
    let mut factory = SessionFactory::default();
    let mut session = factory.build(&config_manager, &config)?;
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    info!("🧪 Test mode: Enter starts/stops recording; Ctrl+C quits");

    // Keep cleanup outside the event-loop future so stdin/signal errors also
    // discard capture and fence unfinished inference before returning.
    let event_loop = async {
        loop {
            tokio::select! {
                line = input.next_line() => {
                    if line?.is_none() { return Ok(()); }
                    if let Err(error) = session.command(Command::Toggle(Trigger::External), Instant::now()) {
                        error!("Dictation command failed: {error:#}");
                    }
                }
                completion = session.next_completion() => {
                    if let Err(error) = session.finish_completion(completion).await {
                        error!("Failed to process recording: {error:#}");
                    }
                }
                changed = config_rx.changed() => {
                    if changed.is_err() { return Ok(()); }
                    pending_config = Some(config_rx.borrow().clone());
                }
            }
            if session.state() == State::Inactive {
                if let Some(updated) = pending_config.take() {
                    if updated != config {
                        match factory.build(&config_manager, &updated) {
                            Ok(updated_session) => {
                                session = updated_session;
                                config = updated;
                                info!("Configuration updated");
                            }
                            Err(error) => error!("Failed to apply config update: {error:#}"),
                        }
                    }
                }
            }
        }
    };
    // Select shutdown around the whole host, including an awaited delivery.
    // Like the daemon's main loop, Ctrl+C can reach cleanup at any await.
    let result: Result<()> = tokio::select! {
        result = event_loop => result,
        result = &mut shutdown => result.map_err(Into::into),
    };
    let cleanup = session.cleanup();
    result.and(cleanup)
}
