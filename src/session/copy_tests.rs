use super::*;

#[tokio::test]
async fn copy_uses_session_output_without_changing_lifecycle_or_history() {
    let (mut session, observed, response) = fixture();
    session.copy_text("idle copy").await.unwrap();
    assert_eq!(session.state(), State::Inactive);
    command(&mut session, Command::Start(Trigger::Hold)).unwrap();
    session.copy_text("recording copy").await.unwrap();
    assert_eq!(session.state(), State::Recording);
    // Copy must not change which trigger owns the active capture.
    command(&mut session, Command::ReleaseHold).unwrap();
    session.copy_text("processing copy").await.unwrap();
    assert_eq!(session.state(), State::Processing);
    assert!(observed.lock().unwrap().history.is_empty());
    response.send(Ok("glorp".into())).unwrap();
    finish(&mut session).await.unwrap();
    let observed = observed.lock().unwrap();
    assert_eq!(observed.starts, 1);
    assert_eq!(observed.stops, 1);
    assert_eq!(
        observed.delivered,
        ["idle copy", "recording copy", "processing copy", "DICTATED"]
    );
    assert_eq!(
        observed.delivery_purposes,
        [
            DeliveryPurpose::Copy,
            DeliveryPurpose::Copy,
            DeliveryPurpose::Copy,
            DeliveryPurpose::Dictation
        ]
    );
    assert_eq!(observed.history, ["DICTATED"]);
}

#[tokio::test]
async fn failed_copy_preserves_recording_and_shutdown_rejects_copy() {
    let (mut session, observed, _response) = fixture();
    command(&mut session, Command::Start(Trigger::External)).unwrap();
    observed.lock().unwrap().fail_delivery = true;
    assert!(session.copy_text("copy").await.is_err());
    assert_eq!(session.state(), State::Recording);
    assert_eq!(observed.lock().unwrap().dropped, 0);
    session.cleanup().unwrap();
    observed.lock().unwrap().fail_delivery = false;
    assert!(session.copy_text("after shutdown").await.is_err());
    assert!(observed.lock().unwrap().delivered.is_empty());
}
