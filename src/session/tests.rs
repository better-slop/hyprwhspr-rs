use super::*;
use crate::text::NormalizeTextService;
use crate::transcription::{BackendMetrics, TranscriptionResult};
use futures_util::future::{BoxFuture, LocalBoxFuture};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

#[path = "cancellation_tests.rs"]
mod cancellation;

#[path = "copy_tests.rs"]
mod copy;

#[derive(Default)]
struct Observed {
    starts: usize,
    stops: usize,
    dropped: usize,
    states: Vec<State>,
    history: Vec<String>,
    delivered: Vec<String>,
    delivery_purposes: Vec<DeliveryPurpose>,
    backend_audio: Vec<f32>,
    backend_calls: usize,
    audio: Option<CapturedAudio>,
    errors: usize,
    cleanups: usize,
    fail_start: bool,
    fail_stop: bool,
    fail_delivery: bool,
    fail_history: bool,
    fail_feedback: Option<Cue>,
    fail_status: Option<State>,
}

struct Capture(Arc<Mutex<Observed>>);
struct Recording(Arc<Mutex<Observed>>);

impl CaptureSource for Capture {
    fn start(&mut self) -> Result<Box<dyn ActiveRecording>> {
        let mut observed = self.0.lock().unwrap();
        observed.starts += 1;
        if observed.fail_start {
            anyhow::bail!("capture start failed");
        }
        Ok(Box::new(Recording(Arc::clone(&self.0))))
    }
}

impl ActiveRecording for Recording {
    fn stop(self: Box<Self>) -> Result<CapturedAudio> {
        let mut observed = self.0.lock().unwrap();
        observed.stops += 1;
        if observed.fail_stop {
            anyhow::bail!("capture stop failed");
        }
        Ok(observed.audio.clone().unwrap_or_else(|| CapturedAudio {
            samples: vec![0.25; 480],
            sample_rate: 48_000,
        }))
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.0.lock().unwrap().dropped += 1;
    }
}

struct Backend {
    observed: Arc<Mutex<Observed>>,
    response: Mutex<Option<oneshot::Receiver<Result<String>>>>,
}

impl Transcriber for Backend {
    fn label(&self) -> String {
        "controlled".into()
    }

    fn transcribe(&self, audio: Vec<f32>) -> BoxFuture<'_, Result<TranscriptionResult>> {
        {
            let mut observed = self.observed.lock().unwrap();
            observed.backend_audio = audio;
            observed.backend_calls += 1;
        }
        let response = self.response.lock().unwrap().take().unwrap();
        Box::pin(async move {
            Ok(TranscriptionResult {
                text: response.await??,
                metrics: BackendMetrics::default(),
            })
        })
    }
}

struct Output(Arc<Mutex<Observed>>);

impl SessionOutput for Output {
    fn feedback(&mut self, cue: Cue) -> Result<()> {
        if self.0.lock().unwrap().fail_feedback == Some(cue) {
            anyhow::bail!("feedback failed");
        }
        Ok(())
    }

    fn state_changed(&mut self, state: State) -> Result<()> {
        let mut observed = self.0.lock().unwrap();
        observed.states.push(state);
        if observed.fail_status == Some(state) {
            anyhow::bail!("status failed");
        }
        Ok(())
    }

    fn save_transcription(&mut self, text: &str) -> Result<()> {
        if self.0.lock().unwrap().fail_history {
            anyhow::bail!("history failed");
        }
        self.0.lock().unwrap().history.push(text.into());
        Ok(())
    }

    fn deliver<'a>(
        &'a mut self,
        text: &'a str,
        purpose: DeliveryPurpose,
    ) -> LocalBoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut observed = self.0.lock().unwrap();
            if observed.fail_delivery {
                anyhow::bail!("delivery failed");
            }
            observed.delivered.push(text.into());
            observed.delivery_purposes.push(purpose);
            Ok(())
        })
    }

    fn report_error(&mut self, _error: &anyhow::Error) {
        self.0.lock().unwrap().errors += 1;
    }

    fn cleanup(&mut self) -> Result<()> {
        self.0.lock().unwrap().cleanups += 1;
        Ok(())
    }
}

fn fixture() -> (
    DictationSession,
    Arc<Mutex<Observed>>,
    oneshot::Sender<Result<String>>,
) {
    fixture_with_vad(None)
}

fn fixture_with_vad(
    vad: Option<crate::audio::FastVad>,
) -> (
    DictationSession,
    Arc<Mutex<Observed>>,
    oneshot::Sender<Result<String>>,
) {
    let observed = Arc::new(Mutex::new(Observed::default()));
    let (response_tx, response_rx) = oneshot::channel();
    let pipeline = Pipeline::new(
        Arc::new(Backend {
            observed: Arc::clone(&observed),
            response: Mutex::new(Some(response_rx)),
        }),
        vad,
        NormalizeTextService::new(HashMap::from([("glorp".into(), "DICTATED".into())])),
    );
    let session = DictationSession::new(
        Box::new(Capture(Arc::clone(&observed))),
        Arc::new(pipeline),
        Box::new(Output(Arc::clone(&observed))),
    );
    (session, observed, response_tx)
}

fn command(session: &mut DictationSession, command: Command) -> Result<State> {
    session.command(command, Instant::now())
}

async fn finish(session: &mut DictationSession) -> Result<()> {
    let completion =
        tokio::time::timeout(std::time::Duration::from_secs(5), session.next_completion())
            .await
            .expect("session completion timed out");
    session.finish_completion(completion).await
}

#[tokio::test]
async fn callers_share_capture_processing_normalization_history_and_delivery() {
    let (mut session, observed, response) = fixture();
    assert_eq!(
        command(&mut session, Command::Start(Trigger::External)).unwrap(),
        State::Recording
    );
    assert_eq!(
        command(&mut session, Command::Stop).unwrap(),
        State::Processing
    );
    // Stopping returns before the controlled backend answers; repeated input stays busy.
    assert_eq!(
        command(&mut session, Command::Toggle(Trigger::Press)).unwrap(),
        State::Processing
    );
    assert_eq!(
        command(&mut session, Command::Start(Trigger::Hold)).unwrap(),
        State::Processing
    );
    response.send(Ok("glorp".into())).unwrap();
    finish(&mut session).await.unwrap();
    assert_eq!(session.state(), State::Inactive);
    let observed = observed.lock().unwrap();
    assert_eq!(observed.starts, 1);
    assert_eq!(observed.stops, 1);
    assert_eq!(observed.backend_audio, vec![0.25; 160]);
    assert_eq!(observed.history, ["DICTATED"]);
    assert_eq!(observed.delivered, observed.history);
    assert_eq!(
        observed.states,
        [State::Recording, State::Processing, State::Inactive]
    );
}

#[tokio::test]
async fn hold_release_owns_only_hold_recordings_and_device_cancel_stops_any_recording() {
    let (mut session, observed, response) = fixture();
    command(&mut session, Command::Start(Trigger::Press)).unwrap();
    assert_eq!(
        command(&mut session, Command::ReleaseHold).unwrap(),
        State::Recording
    );
    command(&mut session, Command::DeviceCancel).unwrap();
    response.send(Ok(String::new())).unwrap();
    finish(&mut session).await.unwrap();
    command(&mut session, Command::Start(Trigger::Hold)).unwrap();
    assert_eq!(
        command(&mut session, Command::ReleaseHold).unwrap(),
        State::Processing
    );
    session.cancel().unwrap();
    assert_eq!(observed.lock().unwrap().stops, 2);
}

#[tokio::test]
async fn capture_failures_release_state_and_resources() {
    for stop_failure in [false, true] {
        let (mut session, observed, _response) = fixture();
        if stop_failure {
            command(&mut session, Command::Start(Trigger::External)).unwrap();
            observed.lock().unwrap().fail_stop = true;
            assert!(command(&mut session, Command::Stop).is_err());
            assert_eq!(observed.lock().unwrap().dropped, 1);
        } else {
            observed.lock().unwrap().fail_start = true;
            assert!(command(&mut session, Command::Start(Trigger::External)).is_err());
        }
        assert_eq!(session.state(), State::Inactive);
        assert_eq!(observed.lock().unwrap().errors, 1);
    }
}

#[tokio::test]
async fn feedback_and_status_failures_cannot_leave_capture_or_busy_state() {
    for fail_status in [false, true] {
        let (mut session, observed, _response) = fixture();
        command(&mut session, Command::Start(Trigger::External)).unwrap();
        if fail_status {
            observed.lock().unwrap().fail_status = Some(State::Processing);
        } else {
            observed.lock().unwrap().fail_feedback = Some(Cue::Stop);
        }
        assert!(command(&mut session, Command::Stop).is_err());
        assert_eq!(session.state(), State::Inactive);
        assert_eq!(observed.lock().unwrap().dropped, 1);
    }
}

#[tokio::test]
async fn failed_start_feedback_or_status_leaves_session_reusable() {
    for fail_status in [false, true] {
        let (mut session, observed, _response) = fixture();
        if fail_status {
            observed.lock().unwrap().fail_status = Some(State::Recording);
        } else {
            observed.lock().unwrap().fail_feedback = Some(Cue::Start);
        }
        assert!(command(&mut session, Command::Start(Trigger::External)).is_err());
        assert_eq!(session.state(), State::Inactive);
        assert_eq!(observed.lock().unwrap().dropped, usize::from(fail_status));
        observed.lock().unwrap().fail_status = None;
        observed.lock().unwrap().fail_feedback = None;
        assert_eq!(
            command(&mut session, Command::Start(Trigger::Hold)).unwrap(),
            State::Recording
        );
    }
}

#[tokio::test]
async fn empty_capture_and_vad_silence_skip_backend_and_delivery() {
    for use_vad in [false, true] {
        let vad = if use_vad {
            crate::audio::FastVad::maybe_new(
                &crate::config::FastVadConfig {
                    enabled: true,
                    ..Default::default()
                },
                16_000,
            )
            .unwrap()
        } else {
            None
        };
        let (mut session, observed, _response) = fixture_with_vad(vad);
        observed.lock().unwrap().audio = Some(CapturedAudio {
            samples: if use_vad {
                vec![0.0; 44_100]
            } else {
                Vec::new()
            },
            // An unsupported VAD rate also exercises its pre-trim resampling.
            sample_rate: 44_100,
        });
        command(&mut session, Command::Start(Trigger::External)).unwrap();
        command(&mut session, Command::Stop).unwrap();
        if use_vad {
            finish(&mut session).await.unwrap();
        }
        assert_eq!(session.state(), State::Inactive);
        let observed = observed.lock().unwrap();
        assert_eq!(observed.backend_calls, 0);
        assert!(observed.history.is_empty());
        assert!(observed.delivered.is_empty());
    }
}

#[tokio::test]
async fn empty_transcript_skips_history_and_delivery() {
    let (mut session, observed, response) = fixture();
    command(&mut session, Command::Start(Trigger::External)).unwrap();
    command(&mut session, Command::Stop).unwrap();
    response.send(Ok(" \t\n ".into())).unwrap();
    finish(&mut session).await.unwrap();
    assert_eq!(session.state(), State::Inactive);
    assert!(observed.lock().unwrap().history.is_empty());
    assert!(observed.lock().unwrap().delivered.is_empty());
}

#[tokio::test]
async fn history_failure_preserves_successful_delivery() {
    let (mut session, observed, response) = fixture();
    observed.lock().unwrap().fail_history = true;
    command(&mut session, Command::Start(Trigger::External)).unwrap();
    command(&mut session, Command::Stop).unwrap();
    response.send(Ok("glorp".into())).unwrap();
    finish(&mut session).await.unwrap();
    assert_eq!(session.state(), State::Inactive);
    assert_eq!(observed.lock().unwrap().delivered, ["DICTATED"]);
}

#[tokio::test]
async fn transcription_and_delivery_failures_return_inactive() {
    for delivery_failure in [false, true] {
        let (mut session, observed, response) = fixture();
        command(&mut session, Command::Start(Trigger::External)).unwrap();
        command(&mut session, Command::Stop).unwrap();
        if delivery_failure {
            observed.lock().unwrap().fail_delivery = true;
            response.send(Ok("glorp".into())).unwrap();
        } else {
            response
                .send(Err(anyhow::anyhow!("backend failed")))
                .unwrap();
        }
        assert!(finish(&mut session).await.is_err());
        assert_eq!(session.state(), State::Inactive);
        let observed = observed.lock().unwrap();
        assert!(observed.delivered.is_empty());
        assert_eq!(observed.errors, 1);
        assert_eq!(observed.history.len(), usize::from(delivery_failure));
    }
}

#[tokio::test]
async fn cancelled_completion_cannot_deliver_or_replace_a_new_recording() {
    let (mut session, observed, response) = fixture();
    command(&mut session, Command::Start(Trigger::External)).unwrap();
    command(&mut session, Command::Stop).unwrap();
    response.send(Ok("glorp".into())).unwrap();
    // A completed inference can be queued when shutdown/cancellation arrives.
    let completion = session.next_completion().await;
    session.cancel().unwrap();
    command(&mut session, Command::Start(Trigger::Hold)).unwrap();
    session.finish_completion(completion).await.unwrap();
    assert_eq!(session.state(), State::Recording);
    let observed = observed.lock().unwrap();
    assert!(observed.history.is_empty());
    assert!(observed.delivered.is_empty());
}

#[tokio::test]
async fn cleanup_discards_capture_and_pending_inference_without_delivery() {
    for processing in [false, true] {
        let (mut session, observed, _response) = fixture();
        command(&mut session, Command::Start(Trigger::External)).unwrap();
        if processing {
            command(&mut session, Command::Stop).unwrap();
        }
        session.cleanup().unwrap();
        session.cleanup().unwrap();
        assert_eq!(session.state(), State::Inactive);
        let observed = observed.lock().unwrap();
        assert_eq!(observed.dropped, 1);
        assert_eq!(observed.cleanups, 1);
        assert!(observed.history.is_empty());
        assert!(observed.delivered.is_empty());
    }
}
