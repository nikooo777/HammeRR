use super::*;
use std::os::unix::process::ExitStatusExt;

#[test]
fn observation_errors_latch_failed_state_without_claiming_an_exit() {
    let mut observation = PlaybackObservation::default();
    observation.observe(Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "injected wait failure",
    )));
    assert_eq!(observation.state(), "failed");
    assert!(!observation.pending());
    assert_eq!(observation.finished, None);
    assert_eq!(
        observation.error.as_deref(),
        Some("Cannot observe sound preview process: injected wait failure")
    );

    observation.observe(Ok(None));
    observation.observe(Ok(Some(ExitStatus::from_raw(0))));
    assert_eq!(observation.state(), "failed");
    assert_eq!(observation.finished, None);
    assert_eq!(
        observation.error.as_deref(),
        Some("Cannot observe sound preview process: injected wait failure")
    );
}

#[test]
fn observation_distinguishes_pending_success_and_failed_exit() {
    let mut observation = PlaybackObservation::default();
    observation.observe(Ok(None));
    assert!(observation.pending());
    assert_eq!(observation.state(), "playing");
    assert_eq!(observation.finished, None);
    assert_eq!(observation.error, None);

    observation.observe(Ok(Some(ExitStatus::from_raw(0))));
    assert!(!observation.pending());
    assert_eq!(observation.state(), "finished");
    assert_eq!(observation.finished, Some(true));
    assert_eq!(observation.error, None);
    observation.observe(Err(io::Error::other("late observation")));
    assert_eq!(observation.state(), "finished");
    assert_eq!(observation.error, None);

    let mut observation = PlaybackObservation::default();
    observation.observe(Ok(Some(ExitStatus::from_raw(1 << 8))));
    assert!(!observation.pending());
    assert_eq!(observation.state(), "failed");
    assert_eq!(observation.finished, Some(false));
    assert_eq!(observation.error, None);
}
