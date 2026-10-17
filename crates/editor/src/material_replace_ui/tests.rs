use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle, lock};
use glam::DVec3;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

fn session() -> Session {
    let mut session = Session::new(Err("no assets required".into()));
    session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/source",
            false,
        )
        .unwrap();
    session
}

fn fixture() -> (
    tempfile::TempDir,
    DocumentHandle,
    SharedSession,
    DocumentAccess,
    Controller,
) {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let mut session = session();
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    access
        .with_session(|session| {
            controller.initialize(session, "brick/source");
            Ok(())
        })
        .unwrap();
    controller
        .window
        .set_replacement("metal/replacement".into());
    (directory, document, shared, access, controller)
}

#[test]
fn managed_replacement_acts_live_then_retained_terminal_callbacks_preserve_forms_drafts_and_peer() {
    let (directory, document, shared, _access, controller) = fixture();
    let source = lock(&shared).document().vmf().clone();
    let initial_history = lock(&shared).history_snapshot();
    controller.window.invoke_find();
    controller.window.invoke_preview_edit();
    assert!(lock(&shared).has_face_preview());
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), initial_history);
    controller.window.show().unwrap();
    controller.window.invoke_apply();
    assert!(!controller.window.window().is_visible());
    assert!(!lock(&shared).has_face_preview());
    assert_ne!(lock(&shared).document().vmf(), &source);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    controller.window.invoke_find();
    controller.window.invoke_select_matches();
    assert_eq!(lock(&shared).selected_faces().len(), 6);
    controller.window.invoke_preview_edit();
    controller.window.show().unwrap();
    let target = controller.target.clone();
    let window = controller.window.clone_strong();
    drop(controller);
    let mut peer = session();
    peer.configure_recovery(Ok(directory.path().join("peer-recovery")));
    let peer_shared = Arc::new(Mutex::new(peer));
    let peer_document = crate::test_assets::document(peer_shared.clone());
    let peer_access = DocumentAccess::for_document(peer_document.clone());
    let peer_controller = Controller::new_with_access(&peer_access).unwrap();
    peer_access
        .with_session(|session| {
            peer_controller.initialize(session, "brick/source");
            Ok(())
        })
        .unwrap();
    peer_controller.window.set_replacement("wood/peer".into());
    peer_controller.window.invoke_find();
    peer_controller.window.invoke_preview_edit();
    let peer_source = lock(&peer_shared).document().vmf().clone();
    let peer_preview = lock(&peer_shared)
        .face_preview_document()
        .unwrap()
        .vmf()
        .clone();
    let peer_history = lock(&peer_shared).history_snapshot();
    let peer_report = lock(&peer_shared).material_search_json(0, 100);
    let local = (target.borrow().report, target.borrow().preview);
    let gate = document.admit().unwrap();
    let draining = document.clone();
    let (done, completed) = mpsc::sync_channel(1);
    let drain = std::thread::spawn(move || done.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while document.lifecycle() == DocumentLifecycle::Live
        || lock(&shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline, "document did not begin draining");
        std::thread::yield_now();
    }
    window.invoke_search_edited();
    window.invoke_replacement_edited();
    window.invoke_find();
    window.invoke_select_matches();
    window.invoke_preview_edit();
    window.invoke_apply();
    assert_eq!(window.get_message(), "the editor is closing");
    assert_eq!((target.borrow().report, target.borrow().preview), local);
    assert!(completed.try_recv().is_err());
    drop(gate);
    completed
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    drain.join().unwrap();
    let store = crate::recovery::Store {
        directory: directory.path().join("recovery"),
    };
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let recovered = store.read(&entries[0].id).unwrap().1;
    let history = lock(&shared).history_snapshot();
    let report = lock(&shared).material_search_json(0, 100);
    let preview = lock(&shared).face_preview_document().unwrap().vmf().clone();
    let preview_token = lock(&shared).face_preview_token();
    let selected = lock(&shared).selected_faces();
    let camera = lock(&shared).camera();
    let view = lock(&shared).view_serial();
    let summary = window.get_summary();
    let ready = window.get_ready();
    let can_apply = window.get_can_apply();
    window.invoke_search_edited();
    window.invoke_replacement_edited();
    window.invoke_find();
    window.invoke_select_matches();
    window.invoke_preview_edit();
    window.invoke_apply();
    assert_eq!(window.get_message(), "the document is closed");
    assert!(window.window().is_visible());
    assert_eq!((target.borrow().report, target.borrow().preview), local);
    assert_eq!(window.get_summary(), summary);
    assert_eq!(window.get_ready(), ready);
    assert_eq!(window.get_can_apply(), can_apply);
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    assert_eq!((target.borrow().report, target.borrow().preview), local);
    assert_eq!(lock(&shared).face_preview_token(), preview_token);
    assert_eq!(
        lock(&shared).face_preview_document().unwrap().vmf(),
        &preview
    );
    assert_eq!(lock(&shared).material_search_json(0, 100), report);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(lock(&shared).selected_faces(), selected);
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(lock(&shared).view_serial(), view);
    assert_eq!(store.list().unwrap().entries.len(), 1);
    assert_eq!(store.read(&entries[0].id).unwrap().1, recovered);
    assert_eq!(lock(&peer_shared).document().vmf(), &peer_source);
    assert_eq!(
        lock(&peer_shared).face_preview_document().unwrap().vmf(),
        &peer_preview
    );
    assert_eq!(lock(&peer_shared).history_snapshot(), peer_history);
    assert_eq!(lock(&peer_shared).material_search_json(0, 100), peer_report);
    peer_controller.window.invoke_apply();
    assert_ne!(lock(&peer_shared).document().vmf(), &peer_source);
    assert!(!lock(&peer_shared).has_face_preview());
    assert_eq!(peer_document.lifecycle(), DocumentLifecycle::Live);
}

#[test]
fn managed_apply_cancel_and_native_close_preserve_the_exact_foreign_preview() {
    let (_directory, document, shared, _access, controller) = fixture();
    let window = &controller.window;
    window.invoke_find();
    window.invoke_preview_edit();
    let own = controller.target.borrow().preview;
    let report = controller.target.borrow().report.unwrap();
    lock(&shared)
        .replace_materials(Actor::Agent, report, "wood/foreign", true)
        .unwrap();
    let foreign = lock(&shared).face_preview_token();
    assert_ne!(own, foreign);
    let preview = lock(&shared).face_preview_document().unwrap().vmf().clone();
    let source = lock(&shared).document().vmf().clone();
    window.invoke_apply();
    assert!(window.get_message().contains("Another face preview"));
    assert_eq!(lock(&shared).face_preview_token(), foreign);
    window.show().unwrap();
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    assert_eq!(controller.target.borrow().preview, None);
    assert_eq!(lock(&shared).face_preview_token(), foreign);
    window.show().unwrap();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!window.window().is_visible());
    assert_eq!(lock(&shared).face_preview_token(), foreign);
    document.shutdown().unwrap();
    window.show().unwrap();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!window.window().is_visible());
    assert_eq!(window.get_message(), "the document is closed");
    assert_eq!(lock(&shared).face_preview_token(), foreign);
    assert_eq!(
        lock(&shared).face_preview_document().unwrap().vmf(),
        &preview
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
}

#[test]
fn retained_replacement_window_keeps_no_session_or_runtime_owner() {
    let (_directory, document, shared, access, controller) = fixture();
    let weak_session = Arc::downgrade(&shared);
    let window = controller.window.clone_strong();
    drop(controller);
    drop(access);
    drop(shared);
    drop(document);
    assert!(weak_session.upgrade().is_none());
    window.invoke_find();
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_cancel();
    assert!(!window.window().is_visible());
}

#[test]
fn missing_replacement_window_abandons_action_while_document_remains_live() {
    let (_directory, document, shared, access, controller) = fixture();
    let weak_window = controller.window.as_weak();
    let continuation = access.downgrade();
    drop(controller);
    assert!(
        with_session(&weak_window, &continuation, |_, _| panic!(
            "vanished window executed Session action"
        ))
        .is_none()
    );
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    assert!(!lock(&shared).has_face_preview());
}

#[test]
fn replacement_preserves_memoized_failed_shutdown_without_cancelling_preview_or_retrying() {
    let (directory, document, shared, _access, controller) = fixture();
    controller.window.invoke_find();
    controller.window.invoke_preview_edit();
    let preview = lock(&shared).face_preview_token();
    let local = (
        controller.target.borrow().report,
        controller.target.borrow().preview,
    );
    let recovery = directory.path().join("recovery");
    std::fs::write(&recovery, b"not a directory").unwrap();
    let error = document.shutdown().unwrap_err();
    let expected = format!("document shutdown failed: {error}");
    controller.window.invoke_find();
    assert_eq!(controller.window.get_message(), expected);
    controller.window.invoke_apply();
    assert_eq!(controller.window.get_message(), expected);
    controller.window.show().unwrap();
    controller.window.invoke_cancel();
    assert!(!controller.window.window().is_visible());
    assert_eq!(controller.window.get_message(), expected);
    assert_eq!(lock(&shared).face_preview_token(), preview);
    assert_eq!(
        (
            controller.target.borrow().report,
            controller.target.borrow().preview
        ),
        local
    );
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    assert_eq!(document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
}
