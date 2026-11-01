//! Shared dictation lifecycle for shortcut, control-socket, and interactive callers.
//!
//! Hosts translate activation into commands and select `next_completion` beside
//! their other events. A stop starts inference and returns immediately. Only
//! `finish_completion` may save or deliver text; cancellation invalidates even a
//! result already returned by `next_completion`. Native device/output details
//! live in `native`, while the same interface accepts controlled adapters in tests.

pub mod native;
mod pipeline;

use anyhow::{Context, Result};
use futures_util::future::{pending, LocalBoxFuture};
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::audio::CapturedAudio;
use crate::benchmark::BenchmarkRecorder;
use pipeline::Processed;
pub use pipeline::{Pipeline, Transcriber};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Inactive,
    Recording,
    Processing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Hold,
    Press,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Start(Trigger),
    Stop,
    Toggle(Trigger),
    ReleaseHold,
    /// A disappearing input device stops any active recording for transcription.
    /// This preserves device-cancel behavior; it is distinct from `cancel`.
    DeviceCancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    Start,
    Stop,
}

/// Dictation uses the configured output mode; explicit copy publishes text
/// without inserting it or changing the active recording/processing operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryPurpose {
    Dictation,
    Copy,
}

pub trait CaptureSource {
    fn start(&mut self) -> Result<Box<dyn ActiveRecording>>;
}

/// Dropping a recording must release its capture resources without transcribing.
pub trait ActiveRecording {
    fn stop(self: Box<Self>) -> Result<CapturedAudio>;
}

/// Host effects stay on the caller's thread (native clipboard handles need not
/// be Send). History is best effort; delivery and status errors are reported.
pub trait SessionOutput {
    fn feedback(&mut self, cue: Cue) -> Result<()>;
    fn state_changed(&mut self, state: State) -> Result<()>;
    fn save_transcription(&mut self, text: &str) -> Result<()>;
    fn deliver<'a>(
        &'a mut self,
        text: &'a str,
        purpose: DeliveryPurpose,
    ) -> LocalBoxFuture<'a, Result<()>>;
    fn report_error(&mut self, error: &anyhow::Error);
    fn cleanup(&mut self) -> Result<()>;
}

enum Lifecycle {
    Idle,
    Recording {
        recording: Box<dyn ActiveRecording>,
        trigger: Trigger,
        benchmark: Box<BenchmarkRecorder>,
    },
    Processing {
        id: Arc<()>,
        task: Option<JoinHandle<Result<Processed>>>,
    },
}

/// Opaque inference result. It carries an identity, so old results cannot finish
/// a newer recording or cause history/clipboard/keyboard effects after cancel.
pub struct Completion {
    id: Arc<()>,
    result: Result<Processed>,
}

pub struct DictationSession {
    capture: Box<dyn CaptureSource>,
    pipeline: Arc<Pipeline>,
    output: Box<dyn SessionOutput>,
    lifecycle: Lifecycle,
    closed: bool,
}

impl DictationSession {
    pub fn new(
        capture: Box<dyn CaptureSource>,
        pipeline: Arc<Pipeline>,
        output: Box<dyn SessionOutput>,
    ) -> Self {
        Self {
            capture,
            pipeline,
            output,
            lifecycle: Lifecycle::Idle,
            closed: false,
        }
    }

    pub fn state(&self) -> State {
        match self.lifecycle {
            Lifecycle::Idle => State::Inactive,
            Lifecycle::Recording { .. } => State::Recording,
            Lifecycle::Processing { .. } => State::Processing,
        }
    }

    /// Route CLI/history copy through the same output owner as dictation.
    /// Copy is independent of capture/inference and never adds history entries.
    pub async fn copy_text(&mut self, text: &str) -> Result<()> {
        anyhow::ensure!(!self.closed, "Dictation session has shut down");
        self.output.deliver(text, DeliveryPurpose::Copy).await
    }

    pub fn command(&mut self, command: Command, at: Instant) -> Result<State> {
        anyhow::ensure!(!self.closed, "Dictation session has shut down");
        let result = match command {
            Command::Start(trigger) if self.state() == State::Inactive => self.start(trigger, at),
            Command::Toggle(trigger) if self.state() == State::Inactive => self.start(trigger, at),
            Command::Stop | Command::DeviceCancel | Command::Toggle(_) => self.stop(at),
            Command::ReleaseHold
                if matches!(
                    self.lifecycle,
                    Lifecycle::Recording {
                        trigger: Trigger::Hold,
                        ..
                    }
                ) =>
            {
                self.stop(at)
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            self.output.report_error(&error);
            if let Err(status_error) = self.cancel() {
                warn!("Failed to publish inactive status: {status_error:#}");
            }
            return Err(error);
        }
        Ok(self.state())
    }

    fn start(&mut self, trigger: Trigger, at: Instant) -> Result<()> {
        info!("🎤 Starting recording...");
        self.output.feedback(Cue::Start)?;
        let recording = self.capture.start().context("Failed to start recording")?;
        self.lifecycle = Lifecycle::Recording {
            recording,
            trigger,
            benchmark: Box::new(BenchmarkRecorder::new(
                self.pipeline.label(),
                at,
                Instant::now(),
            )),
        };
        self.output.state_changed(State::Recording)
    }

    fn stop(&mut self, at: Instant) -> Result<()> {
        if self.state() != State::Recording {
            return Ok(());
        }
        let Lifecycle::Recording {
            recording,
            mut benchmark,
            ..
        } = std::mem::replace(&mut self.lifecycle, Lifecycle::Idle)
        else {
            unreachable!()
        };
        info!("🛑 Stopping recording...");
        benchmark.mark_keybind_stop(at);
        self.output.feedback(Cue::Stop)?;
        let audio = recording.stop().context("Failed to stop recording")?;
        benchmark.mark_recording_stop(Instant::now());
        benchmark.record_original_audio(audio.len(), audio.sample_rate);
        if audio.is_empty() {
            benchmark.mark_injection_skipped(Instant::now());
            log_benchmark(*benchmark);
            return self.output.state_changed(State::Inactive);
        }
        let pipeline = Arc::clone(&self.pipeline);
        self.lifecycle = Lifecycle::Processing {
            // The token is unique even across replacement sessions. An old
            // completion keeps its allocation alive until it is discarded.
            id: Arc::new(()),
            task: Some(tokio::spawn(async move {
                pipeline.process(audio, *benchmark).await
            })),
        };
        self.output.state_changed(State::Processing)
    }

    /// Cancellation-safe wait: dropping this future leaves the task owned by
    /// the session. No history or delivery happens while waiting for inference.
    pub async fn next_completion(&mut self) -> Completion {
        if let Lifecycle::Processing {
            id,
            task: Some(task),
        } = &mut self.lifecycle
        {
            let id = Arc::clone(id);
            let result = task
                .await
                .context("Dictation processing task failed")
                .and_then(|r| r);
            if let Lifecycle::Processing { task, .. } = &mut self.lifecycle {
                *task = None;
            }
            Completion { id, result }
        } else {
            pending().await
        }
    }

    /// Commit a current result to history/delivery, then return to inactive.
    /// Hosts call this in their selected completion branch, rather than racing
    /// its delivery future against the activation events. Cleanup still fences
    /// results if the host itself is dropped during delivery.
    pub async fn finish_completion(&mut self, completion: Completion) -> Result<()> {
        if !matches!(&self.lifecycle, Lifecycle::Processing { id, .. } if Arc::ptr_eq(id, &completion.id))
        {
            return Ok(());
        }
        let result = self.finish_result(completion.result).await;
        self.lifecycle = Lifecycle::Idle;
        if let Err(error) = &result {
            self.output.report_error(error);
        }
        let status_result = self.output.state_changed(State::Inactive);
        result.and(status_result)
    }

    async fn finish_result(&mut self, result: Result<Processed>) -> Result<()> {
        let Processed {
            text,
            mut benchmark,
        } = result?;
        if let Some(text) = text {
            if let Err(error) = self.output.save_transcription(&text) {
                warn!("Failed to save transcription to history: {error:#}");
            }
            benchmark.mark_injection_start(Instant::now());
            self.output
                .deliver(&text, DeliveryPurpose::Dictation)
                .await?;
            benchmark.mark_injection_end(Instant::now());
        } else {
            benchmark.mark_injection_skipped(Instant::now());
        }
        log_benchmark(benchmark);
        Ok(())
    }

    /// Discard capture or in-flight inference, including a queued completion.
    /// Blocking native inference may continue on its worker; it owns no effects.
    pub fn cancel(&mut self) -> Result<()> {
        self.discard();
        self.output.state_changed(State::Inactive)
    }

    pub fn cleanup(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.discard();
        self.output.cleanup()
    }

    fn discard(&mut self) {
        if let Lifecycle::Processing {
            task: Some(task), ..
        } = std::mem::replace(&mut self.lifecycle, Lifecycle::Idle)
        {
            task.abort();
        }
    }
}

impl Drop for DictationSession {
    fn drop(&mut self) {
        self.discard();
    }
}

fn log_benchmark(benchmark: BenchmarkRecorder) {
    if let Some(summary) = benchmark.finalize() {
        info!(message = %format_args!("\n{}", summary));
    }
}

#[cfg(test)]
mod tests;
