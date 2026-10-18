use super::*;
use crate::session::{PropertyChange, PropertyTarget, Session};
use std::sync::mpsc;

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
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(store.directory.clone()));
    let shared = Arc::new(Mutex::new(session));
    let document = DocumentHandle::start(shared.clone()).unwrap();
    (directory, document, shared, store)
}

fn edit(session: &mut Session) {
    session
        .create_block(
            Actor::Agent,
            glam::DVec3::ZERO,
            glam::DVec3::splat(16.0),
            "test",
            false,
        )
        .unwrap();
}

#[test]
fn admitted_edit_makes_close_busy_then_stale_without_cleanup() {
    let (_directory, document, shared, store) = fixture();
    let access = DocumentAccess::for_document(document.clone());
    let (entered, admitted) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        access.with_shared(|session| {
            entered.send(()).unwrap();
            wait.recv_timeout(TIMEOUT).unwrap();
            edit(&mut lock(session));
            Ok(())
        })
    });
    admitted.recv_timeout(TIMEOUT).unwrap();
    let review = document.document_close_review().unwrap();
    let before = lock(&shared).history_snapshot();
    let error = document
        .try_commit_reviewed_close(&review, true)
        .unwrap_err();
    assert!(error.contains("busy"));
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    assert_eq!(lock(&shared).history_snapshot(), before);
    assert!(store.list().unwrap().entries.is_empty());
    release.send(()).unwrap();
    worker.join().unwrap().unwrap();
    assert!(
        document
            .try_commit_reviewed_close(&review, true)
            .unwrap_err()
            .contains("changed")
    );
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    let current = document.document_close_review().unwrap();
    document.try_commit_reviewed_close(&current, true).unwrap();
    assert_eq!(document.lifecycle(), DocumentLifecycle::Draining);
    assert!(document.admit().is_err());
    assert!(
        DocumentAccess::for_document(document.clone())
            .with_session(|session| {
                edit(session);
                Ok(())
            })
            .is_err()
    );
    assert!(store.list().unwrap().entries.is_empty());
    document.shutdown().unwrap();
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let (_, saved) = store.read(&entries[0].id).unwrap();
    assert_eq!(
        source_formats::vmf::Vmf::parse(&saved).unwrap(),
        *lock(&shared).document().vmf()
    );
}

#[test]
fn clean_map_property_draft_requires_exact_reviewed_discard() {
    let (_directory, document, shared, _store) = fixture();
    {
        let mut session = lock(&shared);
        session.begin_property_edit(PropertyTarget::World).unwrap();
    }
    let review = document.document_close_review().unwrap();
    assert!(review.requires_discard());
    assert!(
        document
            .try_commit_reviewed_close(&review, false)
            .unwrap_err()
            .contains("discard")
    );
    let token = lock(&shared).property_draft().unwrap().token;
    lock(&shared)
        .update_property_edit(
            token,
            vec![PropertyChange::Set {
                key: "message".into(),
                value: "pending".into(),
            }],
        )
        .unwrap();
    assert!(
        document
            .try_commit_reviewed_close(&review, true)
            .unwrap_err()
            .contains("changed")
    );
    assert_eq!(lock(&shared).property_draft().unwrap().token, token);
    let current = document.document_close_review().unwrap();
    document.try_commit_reviewed_close(&current, true).unwrap();
    assert!(lock(&shared).property_draft().is_some());
    document.shutdown().unwrap();
    assert_eq!(
        document
            .try_commit_reviewed_close(&current, true)
            .unwrap_err(),
        "the document is closed"
    );
}

#[test]
fn close_review_is_bound_to_runtime_even_for_the_same_session() {
    let (_directory, document, shared, _store) = fixture();
    let peer = DocumentHandle::start(shared).unwrap();
    let review = document.document_close_review().unwrap();
    assert!(
        peer.try_commit_reviewed_close(&review, true)
            .unwrap_err()
            .contains("another document")
    );
    assert_eq!(peer.lifecycle(), DocumentLifecycle::Live);
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    document.shutdown().unwrap();
    peer.shutdown().unwrap();
}

#[test]
fn accepted_close_marks_session_terminal_before_cleanup_and_preserves_failed_drain() {
    let (directory, document, shared, store) = fixture();
    edit(&mut lock(&shared));
    let review = document.document_close_review().unwrap();
    document.try_commit_reviewed_close(&review, true).unwrap();
    let output = directory.path().join("must-not-save.vmf");
    assert!(lock(&shared).save(Actor::User, Some(&output)).is_err());
    assert!(!output.exists());
    std::fs::write(&store.directory, "not a directory").unwrap();
    let error = document.shutdown().unwrap_err();
    assert_eq!(
        document.lifecycle(),
        DocumentLifecycle::Failed(error.clone())
    );
    assert_eq!(
        document
            .try_commit_reviewed_close(&review, true)
            .unwrap_err(),
        format!("document shutdown failed: {error}")
    );
    std::fs::remove_file(&store.directory).unwrap();
    std::fs::create_dir(&store.directory).unwrap();
    assert_eq!(document.shutdown(), Err(error));
    assert!(store.list().unwrap().entries.is_empty());
}

#[cfg(feature = "mcp")]
#[test]
fn token_close_validates_before_reserving_and_reservation_failure_preserves_live() {
    let (_directory, document, shared, store) = fixture();
    let token = document.document_close_review().unwrap().token();
    let reserved = std::cell::Cell::new(0);
    let reserve = || {
        reserved.set(reserved.get() + 1);
        Ok(())
    };
    assert!(
        document
            .commit_close_token("foreign", true, reserve)
            .is_err()
    );
    assert_eq!(reserved.get(), 0);
    let operation = document.admit().unwrap();
    assert!(
        document
            .commit_close_token(&token, true, reserve)
            .unwrap_err()
            .contains("busy")
    );
    assert_eq!(reserved.get(), 0);
    drop(operation);
    edit(&mut lock(&shared));
    assert!(
        document
            .commit_close_token(&token, true, reserve)
            .unwrap_err()
            .contains("changed")
    );
    let current = document.document_close_review().unwrap().token();
    assert!(
        document
            .commit_close_token(&current, false, reserve)
            .unwrap_err()
            .contains("discard")
    );
    assert_eq!(reserved.get(), 0);
    let before = lock(&shared).history_snapshot();
    assert_eq!(
        document.commit_close_token(&current, true, || Err::<(), _>("output unavailable".into())),
        Err("output unavailable".into())
    );
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    assert_eq!(lock(&shared).history_snapshot(), before);
    assert!(store.list().unwrap().entries.is_empty());
    document
        .commit_close_token(&current, true, reserve)
        .unwrap();
    assert_eq!(reserved.get(), 1);
    assert_eq!(document.lifecycle(), DocumentLifecycle::Draining);
    assert!(document.admit().is_err());
    document.shutdown().unwrap();
}
