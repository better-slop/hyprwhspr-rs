//! Daemon activation host. Dictation behavior belongs to `session`; this module
//! translates shortcuts/control requests and keeps the input registry informed.

use anyhow::Result;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

use crate::config::{Config, ConfigManager, ShortcutsConfig};
use crate::control::{ControlRequest, ControlServer, RecordCommand, RecorderState};
use crate::input::{InputManagerHandle, ShortcutEvent, ShortcutKind, ShortcutPhase};
use crate::session::{native::SessionFactory, Command, DictationSession, State, Trigger};

pub struct HyprwhsprApp {
    config_manager: ConfigManager,
    factory: SessionFactory,
    session: DictationSession,
    current_config: Config,
    pending_config: Option<Config>,
    shortcut_tx: mpsc::Sender<ShortcutEvent>,
    shortcut_rx: Option<mpsc::Receiver<ShortcutEvent>>,
    control_tx: mpsc::Sender<ControlRequest>,
    control_rx: Option<mpsc::Receiver<ControlRequest>>,
    input_manager: Option<InputManagerHandle>,
}

impl HyprwhsprApp {
    pub fn new(config_manager: ConfigManager) -> Result<Self> {
        let current_config = config_manager.get();
        let mut factory = SessionFactory::default();
        let session = factory.build(&config_manager, &current_config)?;
        let (shortcut_tx, shortcut_rx) = mpsc::channel(10);
        let (control_tx, control_rx) = mpsc::channel(10);
        Ok(Self {
            config_manager,
            factory,
            session,
            current_config,
            pending_config: None,
            shortcut_tx,
            shortcut_rx: Some(shortcut_rx),
            control_tx,
            control_rx: Some(control_rx),
            input_manager: None,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        info!("🚀 hyprwhspr running!");
        let mut shortcut_rx = self
            .shortcut_rx
            .take()
            .expect("shortcut receiver already consumed");
        let mut control_rx = self
            .control_rx
            .take()
            .expect("control receiver already consumed");
        let _control_server =
            ControlServer::spawn(self.control_tx.clone(), self.config_manager.subscribe())?;
        self.ensure_input_manager(self.current_config.shortcuts.clone())?;
        self.log_shortcuts(&self.current_config.shortcuts);
        let mut config_rx = self.config_manager.subscribe();

        loop {
            tokio::select! {
                event = shortcut_rx.recv() => {
                    let Some(event) = event else { break };
                    if let Err(error) = self.handle_shortcut(event) {
                        error!("Error handling shortcut: {error:#}");
                    }
                }
                request = control_rx.recv() => {
                    let Some(request) = request else { break };
                    self.handle_control_request(request).await;
                }
                completion = self.session.next_completion() => {
                    if let Err(error) = self.session.finish_completion(completion).await {
                        error!("Failed to process recording: {error:#}");
                    }
                }
                result = config_rx.changed() => {
                    if result.is_err() { break; }
                    let updated = config_rx.borrow().clone();
                    if let Err(error) = self.apply_config_update(updated) {
                        error!("Failed to apply config update: {error:#}");
                    }
                }
            }
            self.update_input_busy();
            if self.session.state() == State::Inactive {
                if let Some(config) = self.pending_config.take() {
                    if let Err(error) = self.apply_config_update(config) {
                        error!("Failed to apply deferred config update: {error:#}");
                    }
                }
            }
        }
        Ok(())
    }

    fn ensure_input_manager(&mut self, shortcuts: ShortcutsConfig) -> Result<()> {
        if let Some(manager) = &self.input_manager {
            manager.update_shortcuts(shortcuts)?;
        } else {
            self.input_manager = Some(InputManagerHandle::spawn(
                shortcuts,
                self.shortcut_tx.clone(),
            )?);
        }
        Ok(())
    }

    fn update_input_busy(&self) {
        if let Some(manager) = &self.input_manager {
            manager.set_app_busy(self.session.state() != State::Inactive);
        }
    }

    fn apply_config_update(&mut self, config: Config) -> Result<()> {
        if config == self.current_config {
            self.pending_config = None;
            return Ok(());
        }
        if self.session.state() != State::Inactive {
            self.pending_config = Some(config);
            debug!("Deferring config refresh until dictation finishes");
            return Ok(());
        }
        let session = self.factory.build(&self.config_manager, &config)?;
        if config.shortcuts != self.current_config.shortcuts || self.input_manager.is_none() {
            self.ensure_input_manager(config.shortcuts.clone())?;
            self.log_shortcuts(&config.shortcuts);
        }
        self.session = session;
        self.current_config = config;
        info!("Configuration updated");
        Ok(())
    }

    fn log_shortcuts(&self, shortcuts: &ShortcutsConfig) {
        info!(
            "Press shortcut: {}",
            shortcuts.press.as_deref().unwrap_or("disabled")
        );
        info!(
            "Hold shortcut: {}",
            shortcuts.hold.as_deref().unwrap_or("disabled")
        );
    }

    fn handle_shortcut(&mut self, event: ShortcutEvent) -> Result<()> {
        let command = match (event.kind, event.phase) {
            (ShortcutKind::Press, ShortcutPhase::Start) => Command::Toggle(Trigger::Press),
            (ShortcutKind::Hold, ShortcutPhase::Start) => Command::Start(Trigger::Hold),
            (ShortcutKind::Hold, ShortcutPhase::End) => Command::ReleaseHold,
            (ShortcutKind::Hold, ShortcutPhase::Cancel) => Command::DeviceCancel,
            _ => return Ok(()),
        };
        self.session.command(command, event.triggered_at)?;
        Ok(())
    }

    async fn handle_control_request(&mut self, request: ControlRequest) {
        match request {
            ControlRequest::Record { command, reply_tx } => {
                let result = self
                    .execute_record_command(command)
                    .map_err(|error| format!("{error:#}"));
                if reply_tx.send(result).is_err() {
                    debug!("Control client disconnected before receiving response");
                }
            }
            ControlRequest::CopyText { text, reply_tx } => {
                let result = self
                    .session
                    .copy_text(&text)
                    .await
                    .map_err(|error| format!("{error:#}"));
                if reply_tx.send(result).is_err() {
                    debug!("Copy client disconnected before receiving response");
                }
            }
        }
    }

    fn execute_record_command(&mut self, command: RecordCommand) -> Result<RecorderState> {
        let command = match command {
            RecordCommand::Start => Some(Command::Start(Trigger::External)),
            RecordCommand::Stop => Some(Command::Stop),
            RecordCommand::Toggle => Some(Command::Toggle(Trigger::External)),
            RecordCommand::Status => None,
        };
        if let Some(command) = command {
            self.session.command(command, Instant::now())?;
        }
        Ok(match self.session.state() {
            State::Inactive => RecorderState::Inactive,
            State::Recording => RecorderState::Recording,
            State::Processing => RecorderState::Processing,
        })
    }

    pub async fn cleanup(&mut self) -> Result<()> {
        let result = self.session.cleanup();
        // Before joining activation, close any receiver still owned by the app.
        // run() owns its receiver locally and drops it when returning/cancelled.
        self.shortcut_rx.take();
        if let Some(mut manager) = self.input_manager.take() {
            manager.stop();
        }
        result
    }
}
