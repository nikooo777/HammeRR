use super::*;
use crate::recovery::{Settings, Store};
use crate::runtime::DocumentHandle;
use crate::session::Session;
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

fn fixture() -> (tempfile::TempDir, SharedSession, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store {
        directory: directory.path().join("recovery"),
    };
    let mut state = Session::new(Err("no assets required".into()));
    state.configure_recovery(Ok(store.directory.clone()));
    edit(&mut state, 0.0);
    (directory, Arc::new(Mutex::new(state)), store)
}

fn edit(session: &mut Session, offset: f64) {
    session
        .create_block(
            Actor::User,
            glam::DVec3::splat(offset),
            glam::DVec3::splat(offset + 16.0),
            "test",
            false,
        )
        .unwrap();
}

fn written(store: &Store, entry: &Entry) -> source_formats::vmf::Vmf {
    let (_, text) = store.read(&entry.id).unwrap();
    source_formats::vmf::Vmf::parse(&text).unwrap()
}

#[test]
fn scheduled_recovery_keeps_admitted_content_and_retention_after_edits() {
    let (_directory, session, store) = fixture();
    lock(&session)
        .set_recovery_settings(
            Actor::User,
            Settings {
                keep_per_map: 3,
                ..Settings::default()
            },
            None,
        )
        .unwrap();
    let first = crate::recovery::capture(&session, Actor::User, Kind::Manual).unwrap();
    let second = crate::recovery::capture(&session, Actor::User, Kind::Manual).unwrap();
    let frozen = lock(&session).document().vmf().clone();
    let submission = RecoverySubmission::begin(&session, Actor::Agent).unwrap();
    let (release, wait) = mpsc::channel();
    let worker = submission
        .schedule_with(|submission| {
            Ok(std::thread::spawn(move || {
                wait.recv_timeout(TIMEOUT).unwrap();
                submission.run()
            }))
        })
        .unwrap();
    {
        let mut state = lock(&session);
        edit(&mut state, 32.0);
        state
            .set_recovery_settings(
                Actor::User,
                Settings {
                    keep_per_map: 1,
                    ..Settings::default()
                },
                None,
            )
            .unwrap();
    }
    let source = lock(&session).document().vmf().clone();
    let history = lock(&session).history_snapshot();
    let camera = lock(&session).camera();
    release.send(()).unwrap();
    let entry = worker.join().unwrap().unwrap();
    assert_eq!(written(&store, &entry), frozen);
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().any(|entry| entry.id == first.id));
    assert!(entries.iter().any(|entry| entry.id == second.id));
    let state = lock(&session);
    assert_eq!(state.document().vmf(), &source);
    assert_eq!(state.history_snapshot(), history);
    assert_eq!(state.camera(), camera);
    assert!(state.document().is_dirty());
    assert_eq!(state.recovery_status()["pending_changes"], true);
    assert!(!state.recovery_busy());
    assert_eq!(state.recent_log().next().unwrap().actor, Actor::Agent);
}

#[test]
fn dropped_queued_recovery_retires_under_session_guard_and_preserves_errors() {
    let (_directory, session, store) = fixture();
    let entry = crate::recovery::capture(&session, Actor::User, Kind::Manual).unwrap();
    let request = lock(&session)
        .begin_recovery_snapshot(Kind::Manual)
        .unwrap();
    lock(&session)
        .finish_recovery_snapshot(Actor::User, &request, Err("earlier disk failure".into()))
        .unwrap_err();
    lock(&session)
        .set_recovery_settings(
            Actor::User,
            Settings {
                interval_seconds: 0,
                ..Settings::default()
            },
            None,
        )
        .unwrap_err();
    let errors = lock(&session).recovery_status();
    let submission = RecoverySubmission::begin(&session, Actor::Agent).unwrap();
    let cancellation = submission.cancellation();
    let queued = move || submission.run();
    let (send, receive) = mpsc::channel();
    let retained = session.clone();
    let worker = std::thread::spawn(move || {
        let mut state = lock(&retained);
        let serial = state.recovery_serial();
        drop(queued);
        assert!(!state.recovery_busy());
        assert!(state.recovery_serial() > serial);
        assert_eq!(
            state.recovery_status()["snapshot_error"],
            errors["snapshot_error"]
        );
        assert_eq!(
            state.recovery_status()["settings_error"],
            errors["settings_error"]
        );
        assert_eq!(
            state.recovery_status()["last_snapshot"],
            errors["last_snapshot"]
        );
        let newer = state.begin_recovery_snapshot(Kind::Manual).unwrap();
        drop(cancellation);
        assert!(state.recovery_busy());
        drop(newer);
        assert!(!state.recovery_busy());
        send.send(()).unwrap();
    });
    receive
        .recv_timeout(TIMEOUT)
        .expect("queued recovery Drop locked Session");
    worker.join().unwrap();
    assert_eq!(store.list().unwrap().entries.len(), 1);
    assert_eq!(store.list().unwrap().entries[0].id, entry.id);
}

#[test]
fn queued_recovery_cancellation_skips_the_writer_without_a_disk_failure() {
    let (_directory, session, store) = fixture();
    let request = lock(&session)
        .begin_recovery_snapshot(Kind::Manual)
        .unwrap();
    lock(&session)
        .finish_recovery_snapshot(Actor::User, &request, Err("retained failure".into()))
        .unwrap_err();
    let submission = RecoverySubmission::begin(&session, Actor::Agent).unwrap();
    drop(submission.cancellation());
    assert!(lock(&session).recovery_busy());
    assert_eq!(
        submission
            .run_with(|| panic!("cancelled queued capture entered writer"))
            .unwrap_err(),
        "Recovery snapshot cancelled"
    );
    let state = lock(&session);
    assert!(!state.recovery_busy());
    assert_eq!(
        state.recovery_status()["snapshot_error"],
        "retained failure"
    );
    assert!(state.recovery_status()["last_snapshot"].is_null());
    assert!(store.list().unwrap().entries.is_empty());
}

#[test]
fn cancellation_after_write_claim_keeps_busy_until_original_finish_and_prune() {
    let (_directory, session, store) = fixture();
    lock(&session)
        .set_recovery_settings(
            Actor::User,
            Settings {
                keep_per_map: 1,
                ..Settings::default()
            },
            None,
        )
        .unwrap();
    let previous = crate::recovery::capture(&session, Actor::User, Kind::Manual).unwrap();
    let frozen = lock(&session).document().vmf().clone();
    let submission = RecoverySubmission::begin(&session, Actor::Agent).unwrap();
    let cancellation = submission.cancellation();
    let (started, entered) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        submission.run_with(|| {
            started.send(()).unwrap();
            wait.recv_timeout(TIMEOUT).unwrap();
        })
    });
    entered.recv_timeout(TIMEOUT).unwrap();
    drop(cancellation);
    assert!(lock(&session).recovery_busy());
    assert!(store.read(&previous.id).is_ok());
    edit(&mut lock(&session), 32.0);
    release.send(()).unwrap();
    let entry = worker.join().unwrap().unwrap();
    assert_eq!(written(&store, &entry), frozen);
    assert!(store.read(&previous.id).is_err());
    assert_eq!(store.list().unwrap().entries.len(), 1);
    let state = lock(&session);
    assert!(!state.recovery_busy());
    assert!(state.recovery_status()["snapshot_error"].is_null());
    assert_eq!(state.recovery_status()["last_snapshot"]["id"], entry.id);
    assert_eq!(state.recovery_status()["pending_changes"], true);
    assert_eq!(state.recent_log().next().unwrap().actor, Actor::Agent);
}

#[test]
fn cancellation_after_write_claim_records_the_real_storage_failure() {
    let (directory, session, store) = fixture();
    std::fs::create_dir_all(&store.directory).unwrap();
    let submission = RecoverySubmission::begin(&session, Actor::Agent).unwrap();
    let cancellation = submission.cancellation();
    let (started, entered) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        submission.run_with(|| {
            started.send(()).unwrap();
            wait.recv_timeout(TIMEOUT).unwrap();
        })
    });
    entered.recv_timeout(TIMEOUT).unwrap();
    drop(cancellation);
    assert!(lock(&session).recovery_busy());
    let moved = directory.path().join("moved-recovery");
    std::fs::rename(&store.directory, &moved).unwrap();
    std::fs::write(&store.directory, b"not a directory").unwrap();
    release.send(()).unwrap();
    let error = worker.join().unwrap().unwrap_err();
    assert!(error.contains(store.directory.to_str().unwrap()));
    let state = lock(&session);
    assert!(!state.recovery_busy());
    assert_eq!(state.recovery_status()["snapshot_error"], error);
    assert_eq!(state.recovery_status()["pending_changes"], true);
    assert!(state.recovery_status()["last_snapshot"].is_null());
    drop(state);
    std::fs::remove_file(&store.directory).unwrap();
    std::fs::rename(moved, &store.directory).unwrap();
    assert!(store.list().unwrap().entries.is_empty());
}

#[test]
fn failed_recovery_schedule_keeps_exact_permit_after_closure_destruction() {
    for panic_scheduler in [false, true] {
        let (_directory, session, store) = fixture();
        let submission = RecoverySubmission::begin(&session, Actor::User).unwrap();
        let request = Arc::downgrade(&submission.request);
        let result: Result<(), String> = submission.schedule_with(|submission| {
            let queued = move || submission.run();
            let state = lock(&session);
            drop(queued);
            assert!(state.recovery_busy());
            drop(state);
            assert!(store.list().unwrap().entries.is_empty());
            assert!(request.upgrade().is_some());
            if panic_scheduler {
                panic!("scheduler rejected the closure");
            }
            Err("thread spawn failed".into())
        });
        let failure = result.unwrap_err();
        assert!(request.upgrade().is_none());
        let mut state = lock(&session);
        assert!(!state.recovery_busy());
        assert_eq!(state.recovery_status()["snapshot_error"], failure);
        assert_eq!(state.recent_log().next().unwrap().actor, Actor::User);
        let newer = state.begin_recovery_snapshot(Kind::Manual).unwrap();
        assert!(state.recovery_busy());
        drop(newer);
        assert!(!state.recovery_busy());
        assert!(store.list().unwrap().entries.is_empty());
    }
}

#[test]
fn document_shutdown_waits_for_queued_or_writing_recovery_then_captures_latest() {
    for writing in [false, true] {
        let (_directory, session, store) = fixture();
        let document = DocumentHandle::start(session.clone()).unwrap();
        let submission = RecoverySubmission::begin(&session, Actor::Agent).unwrap();
        let cancellation = submission.cancellation();
        let (started, entered) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            if writing {
                submission.run_with(|| {
                    started.send(()).unwrap();
                    wait.recv_timeout(TIMEOUT).unwrap();
                })
            } else {
                started.send(()).unwrap();
                wait.recv_timeout(TIMEOUT).unwrap();
                submission.run()
            }
        });
        entered.recv_timeout(TIMEOUT).unwrap();
        edit(&mut lock(&session), 32.0);
        let latest = lock(&session).document().vmf().clone();
        let (finished, result) = mpsc::channel();
        let draining = document.clone();
        let shutdown = std::thread::spawn(move || finished.send(draining.shutdown()).unwrap());
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let error = lock(&session)
                .begin_recovery_snapshot(Kind::Manual)
                .err()
                .unwrap();
            if error == "The editor is closing" {
                break;
            }
            assert!(Instant::now() < deadline, "document shutdown did not start");
            std::thread::yield_now();
        }
        drop(cancellation);
        assert!(lock(&session).recovery_busy());
        assert!(result.try_recv().is_err());
        assert!(store.list().unwrap().entries.is_empty());
        release.send(()).unwrap();
        assert_eq!(worker.join().unwrap().is_ok(), writing);
        result.recv_timeout(TIMEOUT).unwrap().unwrap();
        shutdown.join().unwrap();
        document.shutdown().unwrap();
        let entries = store.list().unwrap().entries;
        assert_eq!(entries.len(), if writing { 2 } else { 1 });
        let final_entry = entries
            .iter()
            .find(|entry| entry.kind == Kind::Autosave)
            .unwrap();
        assert_eq!(written(&store, final_entry), latest);
        let state = lock(&session);
        assert!(!state.recovery_busy());
        assert_eq!(state.recovery_status()["pending_changes"], false);
        assert!(state.recovery_status()["error"].is_null());
    }
}
