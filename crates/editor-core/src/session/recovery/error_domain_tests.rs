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

#[test]
fn disk_reload_preserves_live_requests_and_errors_and_only_adopts_valid_settings() {
    let (directory, mut session) = fixture();
    let settings = Settings {
        enabled: true,
        interval_seconds: 60,
        keep_per_map: 4,
    };
    session
        .set_recovery_settings(Actor::User, settings.clone(), None)
        .unwrap();
    let failed = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    session
        .finish_recovery_snapshot(
            Actor::Agent,
            &failed,
            Err("retained capture failure".into()),
        )
        .unwrap_err();
    let request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    let revision = session.recovery_settings_revision();
    let attempt = session.recovery.last_attempt;
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let path = directory.path().join("settings.json");
    for invalid in [
        b"broken".to_vec(),
        vec![b' '; crate::recovery::MAX_SETTINGS_BYTES as usize + 1],
    ] {
        std::fs::write(&path, &invalid).unwrap();
        assert!(
            session
                .reload_recovery_settings(Actor::Agent, Some(revision))
                .is_err()
        );
        assert_eq!(session.recovery_settings(), &settings);
        assert_eq!(session.recovery_settings_revision(), revision);
        assert_eq!(session.recovery.last_attempt, attempt);
        assert!(session.recovery_busy());
        assert_eq!(
            session.recovery_status()["snapshot_error"],
            "retained capture failure"
        );
        assert_eq!(std::fs::read(&path).unwrap(), invalid);
    }
    let replacement = Settings {
        enabled: false,
        interval_seconds: 90,
        keep_per_map: 2,
    };
    std::fs::write(&path, serde_json::to_vec(&replacement).unwrap()).unwrap();
    session
        .reload_recovery_settings(Actor::User, Some(revision))
        .unwrap();
    assert_eq!(session.recovery_settings(), &replacement);
    assert_eq!(request.keep, 4);
    assert_eq!(session.recovery.last_attempt, attempt);
    assert!(session.recovery_busy());
    assert!(session.recovery_status()["settings_error"].is_null());
    assert_eq!(
        session.recovery_status()["snapshot_error"],
        "retained capture failure"
    );
    assert!(
        session
            .reload_recovery_settings(Actor::Agent, Some(revision))
            .is_err()
    );
    assert!(
        session
            .set_recovery_settings(Actor::Agent, settings, Some(revision))
            .is_err()
    );
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    std::fs::remove_file(&path).unwrap();
    session.reload_recovery_settings(Actor::User, None).unwrap();
    assert_eq!(session.recovery_settings(), &Settings::default());
    let mut unavailable = Session::new(Err("no assets".into()));
    let before = unavailable.recovery_status();
    assert!(
        unavailable
            .reload_recovery_settings(Actor::Agent, None)
            .is_err()
    );
    let after = unavailable.recovery_status();
    assert_eq!(after["settings"], before["settings"]);
    assert_eq!(after["settings_revision"], before["settings_revision"]);
    assert!(after["settings_error"].is_string());
    assert!(after["snapshot_error"].is_null());
}
