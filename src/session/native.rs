//! Native adapters assembled once for both daemon and stdin activation hosts.

use anyhow::{Context, Result};
use futures_util::future::{BoxFuture, LocalBoxFuture};
use std::sync::Arc;
use tracing::info;

use super::{
    ActiveRecording, CaptureSource, Cue, DeliveryPurpose, DictationSession, Pipeline,
    SessionOutput, State, Transcriber,
};
use crate::audio::{
    capture::RecordingSession, AudioCapture, AudioFeedback, CapturedAudio, FastVad,
};
use crate::config::{Config, ConfigManager, TranscriptionProvider};
use crate::input::{DeliveryConfig, DeliveryIntent, TextInjector};
use crate::status::{StatusWriter, WaybarState};
use crate::text::NormalizeTextService;
use crate::transcription::{TranscriptionBackend, TranscriptionResult};
use crate::whisper::WhisperVadOptions;

/// Rebuild idle capture/output settings without reloading an unchanged ASR
/// model. New resources replace the old session only after successful setup.
#[derive(Default)]
pub struct SessionFactory {
    backend: Option<(Config, Arc<dyn Transcriber>)>,
}

impl SessionFactory {
    pub fn build(
        &mut self,
        config_manager: &ConfigManager,
        config: &Config,
    ) -> Result<DictationSession> {
        let capture =
            AudioCapture::new(config.audio_device).context("Failed to initialize audio capture")?;
        let transcriber = match &self.backend {
            Some((previous, backend)) if !TranscriptionBackend::needs_refresh(previous, config) => {
                Arc::clone(backend)
            }
            _ => {
                let backend = TranscriptionBackend::build(
                    config_manager,
                    config,
                    vad_options(config_manager, config),
                )
                .context("Failed to configure transcription backend")?;
                backend
                    .initialize()
                    .context("Failed to initialize transcription backend")?;
                info!(
                    "🎯 Active transcription backend: {}",
                    backend.provider().label()
                );
                Arc::new(backend) as Arc<dyn Transcriber>
            }
        };
        let fast_vad = if fast_vad_allowed(config) {
            FastVad::maybe_new(&config.fast_vad, capture.sample_rate_hint())
                .context("Failed to initialize fast VAD pipeline")?
        } else {
            if config.fast_vad.enabled {
                info!("⚡ Earshot fast VAD disabled because whisper-cli VAD is active");
            }
            None
        };
        if let Some(vad) = &fast_vad {
            info!(
                "⚡ Earshot fast VAD enabled (profile: {}, silence timeout: {} ms)",
                vad.settings().base_profile,
                config.fast_vad.silence_timeout_ms
            );
        }
        let output = NativeOutput {
            feedback: AudioFeedback::new(
                config.audio_feedback,
                config_manager.get_assets_dir(),
                config.start_sound_path.clone(),
                config.stop_sound_path.clone(),
                config.start_sound_volume,
                config.stop_sound_volume,
            ),
            injector: TextInjector::new(DeliveryConfig::from(config)),
            dictation_intent: config.dictation_delivery_intent(),
            status: StatusWriter::new()?,
        };
        output.status.set_state(WaybarState::Inactive, "Ready")?;
        self.backend = Some((config.clone(), Arc::clone(&transcriber)));
        Ok(DictationSession::new(
            Box::new(capture),
            Arc::new(Pipeline::new(
                transcriber,
                fast_vad,
                NormalizeTextService::new(config.word_overrides.clone()),
            )),
            Box::new(output),
        ))
    }
}

impl CaptureSource for AudioCapture {
    fn start(&mut self) -> Result<Box<dyn ActiveRecording>> {
        Ok(Box::new(self.start_recording()?))
    }
}

impl ActiveRecording for RecordingSession {
    fn stop(self: Box<Self>) -> Result<CapturedAudio> {
        (*self).stop()
    }
}

impl Transcriber for TranscriptionBackend {
    fn label(&self) -> String {
        self.provider().label().to_string()
    }

    fn transcribe(&self, audio: Vec<f32>) -> BoxFuture<'_, Result<TranscriptionResult>> {
        Box::pin(self.transcribe(audio))
    }
}

struct NativeOutput {
    feedback: AudioFeedback,
    injector: TextInjector,
    dictation_intent: DeliveryIntent,
    status: StatusWriter,
}

impl SessionOutput for NativeOutput {
    fn feedback(&mut self, cue: Cue) -> Result<()> {
        match cue {
            Cue::Start => self.feedback.play_start_sound(),
            Cue::Stop => self.feedback.play_stop_sound(),
        }
    }

    fn state_changed(&mut self, state: State) -> Result<()> {
        let (state, tooltip) = match state {
            State::Inactive => (WaybarState::Inactive, "Ready"),
            State::Recording => (WaybarState::Active, "Recording..."),
            State::Processing => (WaybarState::Processing, "Transcribing..."),
        };
        self.status.set_state(state, tooltip)
    }

    fn save_transcription(&mut self, text: &str) -> Result<()> {
        self.status.save_transcription(text)
    }

    fn deliver<'a>(
        &'a mut self,
        text: &'a str,
        purpose: DeliveryPurpose,
    ) -> LocalBoxFuture<'a, Result<()>> {
        let intent = match purpose {
            DeliveryPurpose::Dictation => self.dictation_intent,
            DeliveryPurpose::Copy => DeliveryIntent::Copy,
        };
        Box::pin(self.injector.deliver(text, intent))
    }

    fn report_error(&mut self, error: &anyhow::Error) {
        if let Err(status_error) = self.status.set_error(&format!("{error:#}")) {
            tracing::warn!("Failed to publish error status: {status_error:#}");
        }
    }

    fn cleanup(&mut self) -> Result<()> {
        self.status.cleanup()
    }
}

fn vad_options(manager: &ConfigManager, config: &Config) -> WhisperVadOptions {
    let vad = &config.transcription.whisper_cpp.vad;
    WhisperVadOptions {
        enabled: vad.enabled,
        model_path: manager.get_vad_model_path(config),
        threshold: vad.threshold,
        min_speech_ms: vad.min_speech_ms,
        min_silence_ms: vad.min_silence_ms,
        max_speech_s: vad.max_speech_s,
        speech_pad_ms: vad.speech_pad_ms,
        samples_overlap: vad.samples_overlap,
    }
}

fn fast_vad_allowed(config: &Config) -> bool {
    config.fast_vad.enabled
        && !(config.transcription.provider == TranscriptionProvider::WhisperCpp
            && config.transcription.whisper_cpp.vad.enabled)
}
