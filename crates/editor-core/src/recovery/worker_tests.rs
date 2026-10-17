use super::*;
use crate::runtime::{SharedSession, lock};
use crate::session::{Actor, Session};
use std::sync::{Arc, Mutex};

fn fixture(dirty: bool) -> (tempfile::TempDir, SharedSession, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("recovery");
    let mut session = Session::new(Err("No content required".into()));
    session.configure_recovery(Ok(path.clone()));
    if dirty {
        session
            .create_block(
                Actor::User,
                glam::DVec3::ZERO,
                glam::DVec3::splat(16.0),
                "test",
                false,
            )
            .unwrap();
    }
    (directory, Arc::new(Mutex::new(session)), path)
}

#[test]
fn recovery_worker_drain_writes_dirty_origin_once_and_preserves_document_state() {
    let (_directory, session, path) = fixture(true);
    let retained = session.clone();
    let (source, history, camera) = {
        let state = lock(&session);
        (
            state.document().vmf().clone(),
            state.history_snapshot(),
            state.camera(),
        )
    };
    let mut worker = Worker::start(&session).unwrap();
    let start = std::time::Instant::now();
    let entry = worker.drain().unwrap().unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(3));
    assert_eq!(entry.kind, Kind::Autosave);
    assert_eq!(worker.drain().unwrap().unwrap().id, entry.id);
    drop(worker);
    let store = Store { directory: path };
    assert_eq!(store.list().unwrap().entries.len(), 1);
    let (_, text) = store.read(&entry.id).unwrap();
    assert_eq!(source_formats::vmf::Vmf::parse(&text).unwrap(), source);
    let state = lock(&retained);
    assert_eq!(state.document().vmf(), &source);
    assert_eq!(state.history_snapshot(), history);
    assert_eq!(state.camera(), camera);
    assert!(state.document().is_dirty());
    assert_eq!(state.recovery_status()["pending_changes"], false);
    assert_eq!(state.recovery_status()["busy"], false);
}

#[test]
fn recovery_worker_drain_clean_and_disabled_do_not_capture() {
    for dirty in [false, true] {
        let (_directory, session, path) = fixture(dirty);
        if dirty {
            let mut state = lock(&session);
            let mut settings = state.recovery_settings().clone();
            settings.enabled = false;
            state
                .set_recovery_settings(Actor::User, settings, None)
                .unwrap();
        }
        let mut worker = Worker::start(&session).unwrap();
        assert!(worker.drain().unwrap().is_none());
        assert!(worker.drain().unwrap().is_none());
        drop(worker);
        assert!(Store { directory: path }.list().unwrap().entries.is_empty());
    }
}

#[test]
fn recovery_worker_drain_failure_is_terminal_visible_and_does_not_retry_after_drop() {
    let (_directory, session, path) = fixture(true);
    std::fs::write(&path, b"not a directory").unwrap();
    let original = lock(&session).document().vmf().clone();
    let mut worker = Worker::start(&session).unwrap();
    let error = worker.drain().unwrap_err();
    assert!(error.contains("Exit recovery snapshot failed"), "{error}");
    let status = lock(&session).recovery_status();
    assert_eq!(status["error"], error);
    assert_eq!(status["pending_changes"], true);
    assert_eq!(status["busy"], false);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(worker.drain().unwrap_err(), error);
    drop(worker);
    assert!(Store { directory: path }.list().unwrap().entries.is_empty());
    assert_eq!(lock(&session).document().vmf(), &original);
}

#[test]
fn recovery_capture_panic_completes_busy_as_error_then_exit_can_recover() {
    let (_directory, session, path) = fixture(true);
    let request = lock(&session)
        .begin_recovery_snapshot(Kind::Manual)
        .unwrap();
    assert!(lock(&session).recovery_busy());
    let result = complete_capture_with(&session, Actor::User, &request, |_| {
        panic!("injected snapshot failure")
    });
    assert_eq!(result.unwrap_err(), "Recovery snapshot worker panicked");
    let status = lock(&session).recovery_status();
    assert_eq!(status["busy"], false);
    assert_eq!(status["pending_changes"], true);
    assert_eq!(status["error"], "Recovery snapshot worker panicked");
    lock(&session).shutdown();
    let mut worker = Worker::start(&session).unwrap();
    assert!(worker.drain().unwrap().is_some());
    assert_eq!(Store { directory: path }.list().unwrap().entries.len(), 1);
    assert_eq!(lock(&session).recovery_status()["pending_changes"], false);
}

#[test]
fn recovery_worker_join_panic_is_reported_even_when_final_snapshot_succeeds() {
    let (_directory, session, path) = fixture(true);
    let thread = std::thread::spawn(|| panic!("injected autosave loop panic"));
    let mut worker = Worker {
        stop: Arc::new((Mutex::new(false), std::sync::Condvar::new())),
        thread: Some(thread),
        session: Arc::downgrade(&session),
        drained: None,
    };
    let error = worker.drain().unwrap_err();
    assert_eq!(error, "Recovery autosave thread panicked");
    assert_eq!(lock(&session).recovery_status()["error"], error);
    assert_eq!(lock(&session).recovery_status()["pending_changes"], false);
    assert_eq!(worker.drain().unwrap_err(), error);
    drop(worker);
    assert_eq!(Store { directory: path }.list().unwrap().entries.len(), 1);
}

#[test]
fn recovery_worker_drain_uses_its_capture_outcome_despite_retained_settings_warning() {
    let (_directory, session, path) = fixture(true);
    std::fs::create_dir_all(path.join("settings.json")).unwrap();
    let warning = lock(&session)
        .set_recovery_settings(Actor::User, Settings::default(), None)
        .unwrap_err();
    assert_eq!(lock(&session).recovery_status()["settings_error"], warning);
    let mut worker = Worker::start(&session).unwrap();
    let entry = worker.drain().unwrap().unwrap();
    assert_eq!(worker.drain().unwrap().unwrap().id, entry.id);
    let status = lock(&session).recovery_status();
    assert_eq!(status["error"], warning);
    assert_eq!(status["settings_error"], warning);
    assert!(status["snapshot_error"].is_null());
    assert!(status["retention_error"].is_null());
    assert!(status["drain_error"].is_null());
    assert_eq!(status["pending_changes"], false);
    assert_eq!(Store { directory: path }.list().unwrap().entries.len(), 1);
}
