use crate::config::ParakeetConfig;
use crate::transcription::postprocess::clean_transcription;
use crate::transcription::{BackendMetrics, TranscriptionResult};
use anyhow::{Context, Result};
use parakeet_rs::{ParakeetTDT, TimestampMode, Transcriber};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::{info, warn};

#[derive(Clone)]
pub struct ParakeetTranscriber {
    model: Arc<Mutex<ParakeetTDT>>,
    prompt: String,
    model_dir: PathBuf,
}

impl ParakeetTranscriber {
    pub fn new(_config: &ParakeetConfig, model_dir: PathBuf, prompt: String) -> Result<Self> {
        let model = ParakeetTDT::from_pretrained(&model_dir, None).with_context(|| {
            format!(
                "Failed to load Parakeet TDT model from {}",
                model_dir.display()
            )
        })?;

        Ok(Self {
            model: Arc::new(Mutex::new(model)),
            prompt,
            model_dir,
        })
    }

    pub fn initialize(&self) -> Result<()> {
        let has_encoder = self.model_dir.join("encoder-model.onnx").exists()
            || self.model_dir.join("encoder.onnx").exists();
        let has_decoder = self.model_dir.join("decoder_joint-model.onnx").exists()
            || self.model_dir.join("decoder_joint.onnx").exists();
        let has_vocab = self.model_dir.join("vocab.txt").exists();

        if !has_encoder {
            anyhow::bail!(
                "Parakeet TDT encoder model not found in {}. Run scripts/download-parakeet-tdt.sh",
                self.model_dir.display()
            );
        }

        if !has_decoder {
            anyhow::bail!(
                "Parakeet TDT decoder model not found in {}. Run scripts/download-parakeet-tdt.sh",
                self.model_dir.display()
            );
        }

        if !has_vocab {
            anyhow::bail!(
                "Parakeet TDT vocab.txt not found in {}. Run scripts/download-parakeet-tdt.sh",
                self.model_dir.display()
            );
        }

        info!(
            "✅ Parakeet TDT transcription ready (model dir: {})",
            self.model_dir.display()
        );
        Ok(())
    }

    pub fn provider_name(&self) -> &'static str {
        "Parakeet TDT (NVIDIA)"
    }

    pub async fn transcribe(&self, audio_data: Vec<f32>) -> Result<TranscriptionResult> {
        if audio_data.is_empty() {
            return Ok(TranscriptionResult {
                text: String::new(),
                metrics: BackendMetrics::default(),
            });
        }

        let duration_secs = audio_data.len() as f32 / 16000.0;
        info!(
            provider = self.provider_name(),
            "🧠 Transcribing {:.2}s of audio via Parakeet TDT", duration_secs
        );

        let transcribe_start = Instant::now();
        let model = self.model.clone();
        let prompt = self.prompt.clone();

        let raw_text = tokio::task::spawn_blocking(move || -> Result<String> {
            let mut guard = model.blocking_lock();
            let result = guard
                .transcribe_samples(audio_data, 16_000, 1, Some(TimestampMode::Sentences))
                .map_err(|e| anyhow::anyhow!("Parakeet transcription failed: {}", e))?;
            Ok(result.text)
        })
        .await
        .context("Parakeet TDT worker panicked")??;

        let transcription_duration = transcribe_start.elapsed();
        let cleaned = clean_transcription(&raw_text, &prompt);

        if cleaned.is_empty() {
            warn!("Parakeet TDT returned empty or non-speech transcription");
        } else {
            info!("✅ Transcription (Parakeet TDT): {}", cleaned);
        }

        let metrics = BackendMetrics {
            encode_duration: None,
            encoded_bytes: None,
            upload_duration: None,
            response_duration: None,
            transcription_duration,
        };

        Ok(TranscriptionResult {
            text: cleaned,
            metrics,
        })
    }
}
