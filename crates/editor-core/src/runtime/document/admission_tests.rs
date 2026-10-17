use super::*;
use crate::session::{Actor, Session};
use std::sync::mpsc;
use std::time::Instant;

const TIMEOUT: Duration = Duration::from_secs(5);

fn fixture() -> (
    tempfile::TempDir,
    DocumentHandle,
    SharedSession,
    recovery::Store,
) {
    let directory = tempfile::tempdir().unwrap();
    let store = recovery::Store {
        directory: directory.path().join("recovery"),
    };
    let mut session = Session::new(Err("no assets required".into()));
    session.configure_recovery(Ok(store.directory.clone()));
    edit(&mut session, 0.0);
    let shared = Arc::new(Mutex::new(session));
    let document = DocumentHandle::start(shared.clone()).unwrap();
    (directory, document, shared, store)
}

fn edit(session: &mut Session, offset: f64) {
    session
        .create_block(
            Actor::Agent,
            glam::DVec3::splat(offset),
            glam::DVec3::splat(offset + 16.0),
            "test",
            false,
        )
        .unwrap();
}

fn wait_for_drain(document: &DocumentHandle) {
    let deadline = Instant::now() + TIMEOUT;
    while document.lifecycle() == DocumentLifecycle::Live {
        assert!(
            Instant::now() < deadline,
            "shutdown did not close admission"
        );
        std::thread::yield_now();
    }
}

#[test]
fn access_admitted_before_shutdown_finishes_fixed_target_before_exit_capture() {
    let (_directory, document, shared, store) = fixture();
    let access = DocumentAccess::for_document(document.clone());
    let (entered, admission) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let operation = std::thread::spawn(move || {
        access.with_shared(|session| {
            entered.send(()).unwrap();
            wait.recv_timeout(TIMEOUT).unwrap();
            edit(&mut lock(session), 96.0);
            Ok(())
        })
    });
    admission.recv_timeout(TIMEOUT).unwrap();
    let draining = document.clone();
    let (finished, result) = mpsc::channel();
    let shutdown = std::thread::spawn(move || finished.send(draining.shutdown()).unwrap());
    wait_for_drain(&document);
    assert!(result.try_recv().is_err());
    assert!(store.list().unwrap().entries.is_empty());
    release.send(()).unwrap();
    operation.join().unwrap().unwrap();
    result.recv_timeout(TIMEOUT).unwrap().unwrap();
    shutdown.join().unwrap();
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let (_, text) = store.read(&entries[0].id).unwrap();
    assert_eq!(
        source_formats::vmf::Vmf::parse(&text).unwrap(),
        *lock(&shared).document().vmf()
    );
    assert_eq!(document.lifecycle(), DocumentLifecycle::Finished);
}

#[test]
fn admitted_operation_delays_final_capture_and_retires_under_session_guard() {
    let (_directory, document, shared, store) = fixture();
    let operation = document.admit().unwrap();
    let retained = operation.clone();
    assert_eq!(document.active_operations(), 1);
    let (send, receive) = mpsc::channel();
    let draining = document.clone();
    let worker = std::thread::spawn(move || send.send(draining.shutdown()).unwrap());
    wait_for_drain(&document);
    assert_eq!(document.admit().err().unwrap(), "the editor is closing");
    assert!(receive.try_recv().is_err());
    assert!(store.list().unwrap().entries.is_empty());
    let expected = {
        let mut session = lock(&shared);
        edit(&mut session, 64.0);
        drop(retained);
        assert_eq!(document.active_operations(), 1);
        drop(operation);
        assert_eq!(document.active_operations(), 0);
        session.document().vmf().clone()
    };
    let mut peer = Session::new(Err("peer assets unavailable".into()));
    edit(&mut peer, 128.0);
    receive.recv_timeout(TIMEOUT).unwrap().unwrap();
    worker.join().unwrap();
    assert_eq!(document.lifecycle(), DocumentLifecycle::Finished);
    assert_eq!(document.admit().err().unwrap(), "the document is closed");
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let (_, text) = store.read(&entries[0].id).unwrap();
    assert_eq!(source_formats::vmf::Vmf::parse(&text).unwrap(), expected);
    document.shutdown().unwrap();
    assert_eq!(store.list().unwrap().entries.len(), 1);
}

#[test]
fn failed_drain_keeps_identity_error_and_terminal_admission_after_storage_repair() {
    let (_directory, document, _shared, store) = fixture();
    let id = document.id();
    std::fs::write(&store.directory, "not a directory").unwrap();
    let error = document.shutdown().unwrap_err();
    assert_eq!(
        document.lifecycle(),
        DocumentLifecycle::Failed(error.clone())
    );
    assert_eq!(
        document.admit().err().unwrap(),
        format!("document shutdown failed: {error}")
    );
    std::fs::remove_file(&store.directory).unwrap();
    std::fs::create_dir(&store.directory).unwrap();
    assert_eq!(document.shutdown().unwrap_err(), error);
    assert_eq!(document.id(), id);
    assert!(store.list().unwrap().entries.is_empty());
}

#[test]
fn identity_survives_buffer_replacement_and_weak_continuations_do_not_keep_runtime_alive() {
    let (directory, document, shared, _store) = fixture();
    let id = document.id();
    let access = DocumentAccess::for_document(document.clone());
    let continuation = access.downgrade();
    let path = directory.path().join("source.vmf");
    access
        .with_session(|session| session.save(Actor::User, Some(&path)))
        .unwrap();
    access
        .with_session(|session| session.new_map(Actor::User, true))
        .unwrap();
    access
        .with_session(|session| session.open(Actor::User, &path, true))
        .unwrap();
    let other_path = directory.path().join("renamed.vmf");
    access
        .with_session(|session| session.save(Actor::User, Some(&other_path)))
        .unwrap();
    assert_eq!(document.id(), id);
    let (_peer_directory, peer, _peer_shared, _peer_store) = fixture();
    assert_ne!(peer.id(), id);
    document.shutdown().unwrap();
    let before = lock(&shared).history_snapshot();
    assert_eq!(
        continuation
            .upgrade()
            .unwrap()
            .with_session(|session| session.new_map(Actor::User, true))
            .unwrap_err(),
        "the document is closed"
    );
    assert_eq!(lock(&shared).history_snapshot(), before);
    drop(access);
    drop(document);
    assert!(continuation.upgrade().is_err());
    let (_next_directory, next, _next_shared, _next_store) = fixture();
    assert_ne!(next.id(), id);
    next.shutdown().unwrap();
    assert_eq!(peer.lifecycle(), DocumentLifecycle::Live);
    peer.shutdown().unwrap();
}
