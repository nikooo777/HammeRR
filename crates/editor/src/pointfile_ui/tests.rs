use super::*;
use crate::runtime::DocumentHandle;
use glam::{DVec3, Vec3};
use std::sync::{Arc, Mutex};

fn fixture() -> (
    tempfile::TempDir,
    DocumentHandle,
    SharedSession,
    DocumentAccess,
    PointfileWindow,
) {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets required".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(16.0), "test", false)
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let window = PointfileWindow::new().unwrap();
    bind_with_access(&window, &access);
    refresh(&window, &lock(&shared));
    (directory, document, shared, access, window)
}

fn report(shared: &SharedSession) -> serde_json::Value {
    serde_json::to_value(lock(shared).pointfile_report(0, 64).unwrap()).unwrap()
}

#[test]
fn pointfile_actions_work_live_then_reject_retained_calls_without_changing_path_or_peer() {
    let (directory, document, shared, _access, window) = fixture();
    let path = directory.path().join("private.lin");
    let bytes = "0 0 64\n128 0 64\n128 128 64\n256 128 128\n";
    std::fs::write(&path, bytes).unwrap();
    window.set_path(path.display().to_string().into());
    window.set_aspect(1.5);
    window.invoke_load();
    assert_eq!(window.get_total(), 4);
    assert!(window.get_current());
    window.invoke_navigate(1);
    assert_eq!(lock(&shared).camera().position, Vec3::new(128.0, 0.0, 64.0));
    window.invoke_frame();
    assert!(window.get_message().is_empty());
    window.invoke_clear();
    assert!(!window.get_has_path());
    window.invoke_load();
    assert!(window.get_current());
    let mut peer = Session::new(Err("peer assets unavailable".into()));
    peer.create_block(Actor::User, DVec3::ZERO, DVec3::splat(16.0), "test", false)
        .unwrap();
    peer.load_pointfile(Actor::User, &path).unwrap();
    let peer_before = serde_json::to_value(peer.pointfile_report(0, 64).unwrap()).unwrap();
    let peer_camera = peer.camera();
    document.shutdown().unwrap();
    let before = report(&shared);
    let history = lock(&shared).history_snapshot();
    let camera = lock(&shared).camera();
    let token = window.get_token();
    window.invoke_navigate(2);
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_frame();
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_clear();
    assert_eq!(window.get_message(), "the document is closed");
    window.set_path(
        directory
            .path()
            .join("not-read.lin")
            .display()
            .to_string()
            .into(),
    );
    window.invoke_load();
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_browse();
    assert_eq!(window.get_message(), "the document is closed");
    assert_eq!(window.get_token(), token);
    assert_eq!(report(&shared), before);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
    assert_eq!(
        serde_json::to_value(peer.pointfile_report(0, 64).unwrap()).unwrap(),
        peer_before
    );
    assert_eq!(peer.camera(), peer_camera);
    window.show().unwrap();
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    assert_eq!(report(&shared), before);
}

#[test]
fn pointfile_picker_builder_and_late_completion_hold_no_waiting_lease_and_do_not_load_files() {
    let (directory, document, shared, access, window) = fixture();
    let target = access.downgrade();
    let path = directory.path().join("chosen.lin");
    let before = report(&shared);
    accept_browse(&window.as_weak(), &target, &path);
    assert_eq!(window.get_path(), path.display().to_string());
    assert_eq!(report(&shared), before);
    let dialog = browse_dialog(&window.as_weak(), &target).unwrap();
    document.shutdown().unwrap();
    let other = directory.path().join("late.lin");
    accept_browse(&window.as_weak(), &target, &other);
    assert_eq!(window.get_path(), path.display().to_string());
    assert_eq!(window.get_message(), "the document is closed");
    assert_eq!(report(&shared), before);
    assert!(!path.exists());
    assert!(!other.exists());
    drop(dialog);
}

#[test]
fn retained_pointfile_window_keeps_no_session_or_runtime_owner() {
    let (_directory, document, shared, access, window) = fixture();
    let weak = Arc::downgrade(&shared);
    drop(access);
    drop(shared);
    drop(document);
    assert!(weak.upgrade().is_none());
    window.invoke_load();
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_clear();
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_cancel();
}
