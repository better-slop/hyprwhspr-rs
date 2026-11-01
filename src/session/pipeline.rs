use anyhow::{Context, Result};
use futures_util::future::BoxFuture;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::{debug, info};

use crate::audio::{CapturedAudio, FastVad};
use crate::benchmark::BenchmarkRecorder;
use crate::text::NormalizeTextService;
use crate::transcription::TranscriptionResult;

/// Existing ASR adapters and controlled test backends use this inference seam.
/// Only audio and text cross it; history, status, and delivery belong to session.
pub trait Transcriber: Send + Sync {
    fn label(&self) -> String;
    fn transcribe(&self, audio: Vec<f32>) -> BoxFuture<'_, Result<TranscriptionResult>>;
}

pub struct Pipeline {
    transcriber: Arc<dyn Transcriber>,
    fast_vad: Arc<Mutex<Option<FastVad>>>,
    normalizer: NormalizeTextService,
}

pub(super) struct Processed {
    pub text: Option<String>,
    pub benchmark: BenchmarkRecorder,
}

impl Pipeline {
    pub fn new(
        transcriber: Arc<dyn Transcriber>,
        fast_vad: Option<FastVad>,
        normalizer: NormalizeTextService,
    ) -> Self {
        Self {
            transcriber,
            fast_vad: Arc::new(Mutex::new(fast_vad)),
            normalizer,
        }
    }

    pub(super) fn label(&self) -> String {
        self.transcriber.label()
    }

    pub(super) async fn process(
        &self,
        audio: CapturedAudio,
        mut benchmark: BenchmarkRecorder,
    ) -> Result<Processed> {
        benchmark.mark_processing_start(Instant::now());
        let fast_vad = Arc::clone(&self.fast_vad);
        // VAD/resampling are CPU work. The activation/control loop must remain
        // responsive even on a single-worker Tokio runtime.
        let (samples, mut benchmark) = tokio::task::spawn_blocking(move || {
            let start = Instant::now();
            let mut vad = fast_vad
                .lock()
                .map_err(|_| anyhow::anyhow!("Fast VAD lock poisoned"))?;
            let samples = preprocess(audio, vad.as_mut(), &mut benchmark)?;
            benchmark.record_preprocess_duration(start.elapsed());
            Ok::<_, anyhow::Error>((samples, benchmark))
        })
        .await
        .context("Audio preprocessing task failed")??;

        let text = if samples.is_empty() {
            None
        } else {
            benchmark.record_audio_sent(samples.len(), 16_000);
            let result = self.transcriber.transcribe(samples).await?;
            benchmark.record_backend_metrics(result.metrics);
            if result.text.trim().is_empty() {
                None
            } else {
                info!("📝 Transcription: {:?}", result.text);
                let text = self.normalizer.normalize(&result.text);
                (!text.is_empty()).then_some(text)
            }
        };
        Ok(Processed { text, benchmark })
    }
}

fn preprocess(
    mut audio: CapturedAudio,
    vad: Option<&mut FastVad>,
    benchmark: &mut BenchmarkRecorder,
) -> Result<Vec<f32>> {
    let mut dropped = None;
    if let Some(vad) = vad {
        if !FastVad::supports_sample_rate(audio.sample_rate) {
            audio.samples = resample_audio(&audio.samples, audio.sample_rate, 16_000);
            audio.sample_rate = 16_000;
        }
        if vad.sample_rate_hz() != audio.sample_rate {
            vad.set_sample_rate(audio.sample_rate)
                .context("Failed to configure fast VAD sample rate")?;
        }
        let outcome = vad
            .trim(&audio.samples)
            .context("Fast VAD trimming failed")?;
        debug!(
            "Earshot fast VAD kept {}/{} samples across {} segments (profile={}, switches={}, dropped={})",
            outcome.trimmed_audio.len(), audio.samples.len(), outcome.segments,
            outcome.final_profile, outcome.profile_switches, outcome.dropped_samples
        );
        dropped = Some(outcome.dropped_samples);
        audio.samples = outcome.trimmed_audio;
    }
    benchmark.record_trimmed_audio(audio.len(), audio.sample_rate, dropped);
    Ok(resample_audio(&audio.samples, audio.sample_rate, 16_000))
}

fn resample_audio(samples: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if samples.is_empty() || src_rate == 0 || dst_rate == 0 {
        return Vec::new();
    }
    if src_rate == dst_rate {
        return samples.to_vec();
    }
    let output_len =
        ((samples.len() as u64 * dst_rate as u64) + (src_rate as u64 / 2)) / src_rate as u64;
    let last = samples.len() - 1;
    (0..output_len as usize)
        .map(|n| {
            let position = n as f64 * src_rate as f64 / dst_rate as f64;
            let index = position.floor() as usize;
            let fraction = (position - index as f64) as f32;
            let left = samples[index.min(last)];
            let right = samples[(index + 1).min(last)];
            left + (right - left) * fraction
        })
        .collect()
}
