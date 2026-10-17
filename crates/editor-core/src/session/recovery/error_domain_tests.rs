use super::*;

fn fixture() -> (tempfile::TempDir, Session) {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets".into()));
    session.configure_recovery(Ok(directory.path().to_owned()));
    (directory, session)
}

#[test]
fn settings_save_and_snapshot_failure_keep_their_own_domains_in_both_orders() {
    for finish_first in [false, true] {
        let (_directory, mut session) = fixture();
        let request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
        let before = session.document().vmf().clone();
        if finish_first {
            session
                .finish_recovery_snapshot(Actor::Agent, &request, Err("snapshot disk full".into()))
                .unwrap_err();
        }
        session
            .set_recovery_settings(Actor::User, Settings::default(), None)
            .unwrap();
        if !finish_first {
            session
                .finish_recovery_snapshot(Actor::Agent, &request, Err("snapshot disk full".into()))
                .unwrap_err();
        }
        let status = session.recovery_status();
        assert_eq!(status["error"], "snapshot disk full");
        assert_eq!(status["snapshot_error"], "snapshot disk full");
        assert!(status["settings_error"].is_null());
        assert!(!session.recovery_busy());
        assert_eq!(session.document().vmf(), &before);
    }
}

#[test]
fn successful_snapshot_preserves_unrepaired_settings_error_and_disabled_policy() {
    let (directory, mut session) = fixture();
    std::fs::write(directory.path().join("settings.json"), b"broken").unwrap();
    session.configure_recovery(Ok(directory.path().to_owned()));
    let failure = session.recovery_status()["error"].clone();
    assert!(failure.is_string());
    assert!(!session.recovery_settings().enabled);
    let request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    let entry = session
        .finish_recovery_snapshot(Actor::Agent, &request, request.run())
        .unwrap();
    assert!(
        session
            .recovery_entries()
            .unwrap()
            .entries
            .iter()
            .any(|saved| saved.id == entry.id)
    );
    let status = session.recovery_status();
    assert_eq!(status["settings_error"], failure);
    assert_eq!(status["error"], failure);
    assert!(status["snapshot_error"].is_null());
    assert!(status["retention_error"].is_null());
    assert!(!session.recovery_settings().enabled);
    session
        .set_recovery_settings(Actor::User, Settings::default(), None)
        .unwrap();
    assert!(session.recovery_status()["error"].is_null());
}

#[test]
fn owned_cleanup_completion_survives_later_settings_and_snapshot_success() {
    let (_directory, mut session) = fixture();
    let mut request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    request.keep = 0;
    let completed = session
        .finish_recovery_snapshot_completed(Actor::Agent, &request, request.run())
        .unwrap();
    assert_eq!(
        completed.retention_error.as_deref(),
        Some("Keep at least one recovery snapshot")
    );
    assert_eq!(
        session.recovery_status()["retention_error"],
        "Keep at least one recovery snapshot"
    );
    session
        .set_recovery_settings(Actor::User, Settings::default(), None)
        .unwrap();
    assert_eq!(
        session.recovery_status()["error"],
        "Keep at least one recovery snapshot"
    );
    let next = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    session
        .finish_recovery_snapshot(Actor::Agent, &next, next.run())
        .unwrap();
    assert!(session.recovery_status()["retention_error"].is_null());
    assert_eq!(
        completed.retention_error.as_deref(),
        Some("Keep at least one recovery snapshot")
    );
    assert_eq!(completed.entry.document_key, request.document_key);
}

#[test]
fn terminal_drain_error_survives_settings_and_capture_success_without_duplicate_wrapping() {
    let (_directory, mut session) = fixture();
    let request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    session
        .finish_recovery_snapshot(Actor::Agent, &request, Err("disk full".into()))
        .unwrap_err();
    session.report_recovery_worker_error("Exit recovery snapshot failed: disk full".into());
    assert_eq!(
        session.recovery_status()["error"],
        "Exit recovery snapshot failed: disk full"
    );
    session
        .set_recovery_settings(Actor::User, Settings::default(), None)
        .unwrap();
    let next = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    session
        .finish_recovery_snapshot(Actor::Agent, &next, next.run())
        .unwrap();
    assert_eq!(
        session.recovery_status()["drain_error"],
        "Exit recovery snapshot failed: disk full"
    );
}

#[test]
fn aggregate_does_not_hide_an_unrelated_substring_error() {
    let (_directory, mut session) = fixture();
    session.recovery.snapshot_error = Some("snapshot disk failure".into());
    session.recovery.settings_error = Some("disk".into());
    assert_eq!(
        session.recovery_status()["error"],
        "snapshot disk failure; disk"
    );
}
