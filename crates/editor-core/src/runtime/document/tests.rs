use super::*;
use crate::session::{Actor, Session};

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
    let mut state = Session::new(Err("no assets needed".into()));
    state.configure_recovery(Ok(store.directory.clone()));
    state
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(64.0),
            "test",
            false,
        )
        .unwrap();
    let session = Arc::new(Mutex::new(state));
    let document = DocumentHandle::start(session.clone()).unwrap();
    (directory, document, session, store)
}

#[test]
fn owning_handles_retain_fixed_runtime_until_the_last_owner_drains() {
    let (_directory, document, session, store) = fixture();
    let runtime = Arc::downgrade(&document.0);
    let original = lock(&session).document().vmf().clone();
    let retained = document.clone();
    let second = retained.clone();
    assert!(Arc::ptr_eq(document.session(), &session));
    drop(document);
    assert!(runtime.upgrade().is_some());
    assert!(store.list().unwrap().entries.is_empty());
    drop(retained);
    assert!(runtime.upgrade().is_some());
    assert!(store.list().unwrap().entries.is_empty());
    drop(second);
    assert!(runtime.upgrade().is_none());
    assert_eq!(store.list().unwrap().entries.len(), 1);
    let state = lock(&session);
    assert_eq!(state.document().vmf(), &original);
    assert_eq!(state.recovery_status()["pending_changes"], false);
    assert_eq!(state.recovery_status()["busy"], false);
    assert!(state.recovery_status()["error"].is_null());
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_clones_retain_the_same_document_runtime() {
    let (_directory, document, _session, store) = fixture();
    let runtime = Arc::downgrade(&document.0);
    let server = crate::mcp::EditorServer::for_document(document.clone(), None);
    let retained = server.clone();
    drop(document);
    drop(server);
    assert!(runtime.upgrade().is_some());
    assert!(store.list().unwrap().entries.is_empty());
    drop(retained);
    assert!(runtime.upgrade().is_none());
    assert_eq!(store.list().unwrap().entries.len(), 1);
}

#[test]
fn finished_handles_retain_exact_failure_without_retrying_after_storage_repair() {
    let (_directory, document, session, store) = fixture();
    let retained = document.clone();
    std::fs::write(&store.directory, b"not a directory").unwrap();
    let failure = document.shutdown().unwrap_err();
    assert!(failure.contains("Exit recovery snapshot failed"));
    std::fs::remove_file(&store.directory).unwrap();
    std::fs::create_dir(&store.directory).unwrap();
    lock(&session)
        .create_block(
            Actor::Agent,
            glam::DVec3::splat(128.0),
            glam::DVec3::splat(192.0),
            "late",
            false,
        )
        .unwrap();
    assert_eq!(retained.shutdown().unwrap_err(), failure);
    drop(document);
    assert_eq!(retained.shutdown().unwrap_err(), failure);
    drop(retained);
    assert!(store.list().unwrap().entries.is_empty());
    let status = lock(&session).recovery_status();
    assert_eq!(status["pending_changes"], true);
    assert_eq!(status["busy"], false);
    assert_eq!(status["drain_error"], failure);
}
