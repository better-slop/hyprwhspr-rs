use super::*;
use std::time::Duration;

#[tokio::test]
async fn cancelling_completion_wait_does_not_detach_or_lose_inference() {
    let (mut session, observed, response) = fixture();
    command(&mut session, Command::Start(Trigger::External)).unwrap();
    command(&mut session, Command::Stop).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(10), session.next_completion())
            .await
            .is_err()
    );
    assert_eq!(session.state(), State::Processing);
    response.send(Ok("glorp".into())).unwrap();
    finish(&mut session).await.unwrap();
    assert_eq!(observed.lock().unwrap().delivered, ["DICTATED"]);
}

#[tokio::test]
async fn completion_from_a_replaced_session_cannot_finish_its_replacement() {
    let (mut old, _, old_response) = fixture();
    command(&mut old, Command::Start(Trigger::External)).unwrap();
    command(&mut old, Command::Stop).unwrap();
    old_response.send(Ok("glorp".into())).unwrap();
    let old_completion = old.next_completion().await;
    let (mut replacement, observed, _response) = fixture();
    command(&mut replacement, Command::Start(Trigger::External)).unwrap();
    command(&mut replacement, Command::Stop).unwrap();
    replacement.finish_completion(old_completion).await.unwrap();
    assert_eq!(replacement.state(), State::Processing);
    assert!(observed.lock().unwrap().history.is_empty());
    assert!(observed.lock().unwrap().delivered.is_empty());
}

struct BlockingBackend {
    gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    started: Mutex<Option<oneshot::Sender<()>>>,
    finished: Mutex<Option<oneshot::Sender<()>>>,
}

impl Transcriber for BlockingBackend {
    fn label(&self) -> String {
        "non-abortable model".into()
    }

    fn transcribe(&self, _audio: Vec<f32>) -> BoxFuture<'_, Result<TranscriptionResult>> {
        let gate = self.gate.lock().unwrap().take().unwrap();
        let started = self.started.lock().unwrap().take().unwrap();
        let finished = self.finished.lock().unwrap().take().unwrap();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                started.send(()).unwrap();
                gate.recv().unwrap();
                finished.send(()).unwrap();
            })
            .await?;
            Ok(TranscriptionResult {
                text: "glorp".into(),
                metrics: BackendMetrics::default(),
            })
        })
    }
}

#[tokio::test]
async fn cleanup_fences_a_native_worker_that_finishes_after_cancellation() {
    let observed = Arc::new(Mutex::new(Observed::default()));
    let (release, gate) = std::sync::mpsc::channel();
    let (started_tx, started) = oneshot::channel();
    let (finished_tx, finished) = oneshot::channel();
    let pipeline = Pipeline::new(
        Arc::new(BlockingBackend {
            gate: Mutex::new(Some(gate)),
            started: Mutex::new(Some(started_tx)),
            finished: Mutex::new(Some(finished_tx)),
        }),
        None,
        NormalizeTextService::new(HashMap::new()),
    );
    let mut session = DictationSession::new(
        Box::new(Capture(Arc::clone(&observed))),
        Arc::new(pipeline),
        Box::new(Output(Arc::clone(&observed))),
    );
    command(&mut session, Command::Start(Trigger::External)).unwrap();
    command(&mut session, Command::Stop).unwrap();
    tokio::time::timeout(Duration::from_secs(5), started)
        .await
        .unwrap()
        .unwrap();
    session.cleanup().unwrap();
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), finished)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.state(), State::Inactive);
    let observed = observed.lock().unwrap();
    assert!(observed.history.is_empty());
    assert!(observed.delivered.is_empty());
}
