use super::*;

fn fixture() -> (tempfile::TempDir, Authority, Session, Session) {
    let directory = tempfile::tempdir().unwrap();
    let authority = Authority::configured(Ok(directory.path().join("settings")));
    let mut first = Session::with_recovery_settings(Err("no assets".into()), authority.clone());
    let mut second = Session::with_recovery_settings(Err("no assets".into()), authority.clone());
    first.configure_recovery(Ok(directory.path().join("first")));
    second.configure_recovery(Ok(directory.path().join("second")));
    for session in [&mut first, &mut second] {
        session
            .create_block(
                Actor::User,
                glam::DVec3::ZERO,
                glam::DVec3::splat(64.0),
                "test",
                false,
            )
            .unwrap();
    }
    (directory, authority, first, second)
}

fn capture(session: &mut Session) -> Entry {
    let request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    session
        .finish_recovery_snapshot(Actor::User, &request, request.run())
        .unwrap()
}

#[test]
fn shared_settings_preserve_local_requests_clocks_errors_and_observers() {
    let (_directory, authority, mut first, mut second) = fixture();
    let source = second.document().vmf().clone();
    let history = second.history_snapshot();
    let camera = second.camera();
    let clock = second.recovery.last_attempt;
    let key = second.recovery.document_key.clone();
    let serial = second.recovery_serial();
    let request = first.begin_recovery_snapshot(Kind::Manual).unwrap();
    second
        .update_recovery_settings(
            Actor::Agent,
            SettingsUpdate {
                enabled: Some(false),
                keep_per_map: Some(1),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    assert_eq!(request.keep, 10);
    assert!(first.recovery_busy());
    assert!(!second.recovery_busy());
    assert_eq!(second.recovery.last_attempt, clock);
    assert!(second.recovery_serial() > serial);
    let shared_serial = second.recovery_serial();
    first
        .finish_recovery_snapshot(Actor::Agent, &request, Err("first snapshot failed".into()))
        .unwrap_err();
    assert_eq!(second.recovery_serial(), shared_serial);
    assert!(second.recovery_status()["snapshot_error"].is_null());
    assert_eq!(
        first.recovery_status()["snapshot_error"],
        "first snapshot failed"
    );
    let revision = authority.snapshot().revision;
    second
        .update_recovery_settings(
            Actor::User,
            SettingsUpdate {
                interval_seconds: Some(0),
                ..Default::default()
            },
            None,
        )
        .unwrap_err();
    assert_eq!(authority.snapshot().revision, revision);
    assert!(second.recovery_serial() > shared_serial);
    assert_eq!(
        first.recovery_status()["settings_error"],
        second.recovery_status()["settings_error"]
    );
    capture(&mut first);
    assert!(first.recovery_status()["snapshot_error"].is_null());
    assert!(first.recovery_status()["settings_error"].is_string());
    assert!(second.recovery.last_entry.is_none());
    second
        .update_recovery_settings(Actor::User, SettingsUpdate::default(), None)
        .unwrap();
    assert!(first.recovery_status()["settings_error"].is_null());
    assert_eq!(second.recovery.document_key, key);
    assert_eq!(second.recovery.last_attempt, clock);
    assert_eq!(second.document().vmf(), &source);
    assert_eq!(second.history_snapshot(), history);
    assert_eq!(second.camera(), camera);
    assert_eq!(first.recovery_settings(), second.recovery_settings());
}

#[test]
fn attached_configuration_does_not_rebind_settings_and_independent_sessions_stay_independent() {
    let (directory, authority, mut first, mut second) = fixture();
    let settings = Settings {
        enabled: false,
        interval_seconds: 90,
        keep_per_map: 3,
    };
    first
        .set_recovery_settings(Actor::User, settings.clone(), None)
        .unwrap();
    let snapshot = authority.snapshot();
    let other = directory.path().join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("settings.json"), b"broken").unwrap();
    second.configure_recovery(Ok(other.clone()));
    assert_eq!(authority.snapshot(), snapshot);
    assert_eq!(second.recovery.store.as_ref().unwrap().directory, other);
    let mut independent = Session::new(Err("no assets".into()));
    independent.configure_recovery(Ok(directory.path().join("settings")));
    assert_eq!(independent.recovery_settings(), settings);
    first
        .set_recovery_settings(Actor::Agent, Settings::default(), None)
        .unwrap();
    assert_eq!(independent.recovery_settings(), settings);
    assert_eq!(second.recovery_settings(), Settings::default());
    let settings = authority.snapshot();
    let failed = second.begin_recovery_snapshot(Kind::Manual).unwrap();
    second
        .finish_recovery_snapshot(Actor::Agent, &failed, Err("capture failed".into()))
        .unwrap_err();
    second.configure_recovery(Err("snapshot mount unavailable".into()));
    assert_eq!(authority.snapshot(), settings);
    assert!(
        second.recovery_status()["storage_error"]
            .as_str()
            .unwrap()
            .contains("snapshot mount unavailable")
    );
    assert_eq!(second.recovery_status()["snapshot_error"], "capture failed");
    assert!(first.recovery_status()["storage_error"].is_null());
    assert!(first.recovery_status()["snapshot_error"].is_null());
    second.configure_recovery(Ok(directory.path().join("repaired")));
    assert!(second.recovery_status()["storage_error"].is_null());
    assert_eq!(second.recovery_status()["snapshot_error"], "capture failed");
}

#[test]
fn autosave_exit_and_backup_capture_one_policy_before_shared_settings_changes() {
    let (directory, _authority, mut first, mut second) = fixture();
    first
        .update_recovery_settings(
            Actor::User,
            SettingsUpdate {
                keep_per_map: Some(3),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    capture(&mut first);
    capture(&mut first);
    first
        .create_block(
            Actor::User,
            glam::DVec3::splat(128.0),
            glam::DVec3::splat(192.0),
            "newer",
            false,
        )
        .unwrap();
    first.recovery.last_attempt = Instant::now() - Duration::from_secs(120);
    let due = first.begin_due_recovery_snapshot().unwrap();
    second
        .update_recovery_settings(
            Actor::Agent,
            SettingsUpdate {
                enabled: Some(false),
                keep_per_map: Some(1),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    assert_eq!(due.keep, 3);
    first
        .finish_recovery_snapshot(Actor::System, &due, due.run())
        .unwrap();
    assert_eq!(first.recovery_entries().unwrap().entries.len(), 3);
    assert!(second.recovery_entries().unwrap().entries.is_empty());
    assert!(second.begin_exit_recovery_snapshot().unwrap().is_none());
    assert!(second.begin_due_recovery_snapshot().is_none());
    second
        .update_recovery_settings(
            Actor::Agent,
            SettingsUpdate {
                enabled: Some(true),
                keep_per_map: Some(3),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let exit = second.begin_exit_recovery_snapshot().unwrap().unwrap();
    first
        .update_recovery_settings(
            Actor::User,
            SettingsUpdate {
                enabled: Some(false),
                keep_per_map: Some(1),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    assert_eq!(exit.keep, 3);
    second
        .finish_recovery_snapshot(Actor::System, &exit, exit.run())
        .unwrap();
    let map = directory.path().join("map.vmf");
    std::fs::write(&map, first.document().vmf().write(&Default::default())).unwrap();
    first
        .update_recovery_settings(
            Actor::User,
            SettingsUpdate {
                keep_per_map: Some(3),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let destination = source_formats::file_publication::BoundDestination::capture(
        &map,
        source_formats::file_publication::Policy::FollowSource,
    )
    .unwrap();
    for _ in 0..2 {
        let backup = first.backup_before_save(&map, &destination).unwrap();
        first.finish_save_backup(backup.as_ref(), true).unwrap();
    }
    let backup = first
        .backup_before_save(&map, &destination)
        .unwrap()
        .unwrap();
    second
        .update_recovery_settings(
            Actor::Agent,
            SettingsUpdate {
                keep_per_map: Some(1),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    first.finish_save_backup(Some(&backup), true).unwrap();
    assert_eq!(
        backup
            .store
            .list()
            .unwrap()
            .entries
            .iter()
            .filter(|entry| entry.document_key == backup.entry.document_key)
            .count(),
        3
    );
}

#[test]
fn draining_one_origin_preserves_the_other_worker_and_document_failure() {
    use crate::runtime::lock;
    use std::sync::{Arc, Mutex};

    let (_directory, authority, first, mut second) = fixture();
    let failed = second.begin_recovery_snapshot(Kind::Manual).unwrap();
    second
        .finish_recovery_snapshot(Actor::Agent, &failed, Err("second failed".into()))
        .unwrap_err();
    let first = Arc::new(Mutex::new(first));
    let second = Arc::new(Mutex::new(second));
    let mut first_worker = crate::recovery::Worker::start(&first).unwrap();
    let mut second_worker = crate::recovery::Worker::start(&second).unwrap();
    let settings = authority.snapshot();
    let entry = first_worker.drain().unwrap().unwrap();
    assert_eq!(first_worker.drain().unwrap().unwrap().id, entry.id);
    drop(first_worker);
    drop(first);
    assert_eq!(authority.snapshot(), settings);
    assert_eq!(
        lock(&second).recovery_status()["snapshot_error"],
        "second failed"
    );
    assert_eq!(lock(&second).recovery_status()["pending_changes"], true);
    assert!(second_worker.drain().unwrap().is_some());
    assert_eq!(lock(&second).recovery_status()["pending_changes"], false);
    assert!(lock(&second).recovery_status()["snapshot_error"].is_null());
}
