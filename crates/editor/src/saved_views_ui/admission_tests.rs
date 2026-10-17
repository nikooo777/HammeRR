use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use glam::DVec3;
use serde_json::{Value, json};
use slint::Model;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);
const ACTIONS: [fn(&SavedViewsWindow); 6] = [
    |window| window.invoke_choose("1".into()),
    SavedViewsWindow::invoke_save_new,
    SavedViewsWindow::invoke_replace_view,
    SavedViewsWindow::invoke_rename_view,
    SavedViewsWindow::invoke_restore_view,
    SavedViewsWindow::invoke_delete_view,
];

fn fixture() -> (
    tempfile::TempDir,
    DocumentHandle,
    DocumentAccess,
    SharedSession,
    Controller,
) {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(64.0), "test", false)
        .unwrap();
    session
        .save_camera_view(Actor::User, "Initial", None, None)
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    controller.show(&lock(&shared)).unwrap();
    controller.window.invoke_choose("1".into());
    (directory, document, access, shared, controller)
}

fn snapshot(shared: &SharedSession) -> (source_formats::vmf::Vmf, Value) {
    let session = lock(shared);
    (
        session.document().vmf().clone(),
        json!({
            "history": session.history_snapshot(),
            "camera": format!("{:?}", session.camera()),
            "camera_serial": session.camera_serial(),
            "scene_serial": session.scene_serial(),
            "log_serial": session.log_serial(),
            "selected": session.selected_ids(),
        }),
    )
}

fn form_snapshot(controller: &Controller) -> Value {
    let form = controller.form.borrow();
    json!({
        "guard": form.guard,
        "selected": form.selected,
        "id": controller.window.get_selected_id().to_string(),
        "name": controller.window.get_view_name().to_string(),
        "current": controller.window.get_current().to_string(),
        "editable": controller.window.get_editable(),
        "rows": controller.window.get_views().iter().map(|row| (row.id.to_string(),row.name.to_string(),row.projection.to_string())).collect::<Vec<_>>(),
    })
}

#[test]
fn managed_saved_view_actions_keep_exact_review_guards_and_live_peer_isolation() {
    i_slint_backend_testing::init_no_event_loop();
    let (_directory, document, _access, shared, controller) = fixture();
    let (_peer_directory, peer, _peer_access, peer_shared, _peer_controller) = fixture();
    let peer_before = snapshot(&peer_shared);
    controller.window.set_view_name("Second".into());
    controller.window.invoke_save_new();
    assert_eq!(lock(&shared).camera_views().unwrap().views.len(), 2);
    let id = controller.window.get_selected_id();
    lock(&shared).set_projection(Actor::User, render::Projection::Top);
    controller.window.set_view_name("Replacement".into());
    controller.window.invoke_replace_view();
    let saved_camera = lock(&shared).camera();
    controller.window.set_view_name("Renamed".into());
    controller.window.invoke_rename_view();
    assert_eq!(
        lock(&shared).camera_views().unwrap().views[1].name,
        "Renamed"
    );
    lock(&shared).set_projection(Actor::User, render::Projection::Front);
    controller.window.invoke_restore_view();
    assert_eq!(lock(&shared).camera(), saved_camera);
    lock(&shared)
        .create_block(
            Actor::Agent,
            DVec3::splat(96.0),
            DVec3::splat(112.0),
            "test",
            false,
        )
        .unwrap();
    let before = snapshot(&shared);
    let form = form_snapshot(&controller);
    for invoke in &ACTIONS[1..] {
        invoke(&controller.window);
        assert!(controller.window.get_message().contains("Map changed"));
        assert_eq!(snapshot(&shared), before);
        assert_eq!(form_snapshot(&controller), form);
    }
    controller.window.invoke_choose(id);
    controller.window.invoke_delete_view();
    assert_eq!(lock(&shared).camera_views().unwrap().views.len(), 1);
    assert_eq!(snapshot(&peer_shared), peer_before);
    assert_eq!(peer.lifecycle(), DocumentLifecycle::Live);
    document.shutdown().unwrap();
    peer.shutdown().unwrap();
}

#[test]
fn all_saved_view_callbacks_preserve_pending_form_during_real_drain_and_after_finish() {
    i_slint_backend_testing::init_no_event_loop();
    let (directory, document, _access, shared, controller) = fixture();
    let (_peer_directory, peer, _peer_access, peer_shared, peer_controller) = fixture();
    controller.window.set_view_name("Pending rename".into());
    let gate = document.admit().unwrap();
    let draining = document.clone();
    let (send, receive) = mpsc::channel();
    let thread = std::thread::spawn(move || send.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + TIMEOUT;
    while document.lifecycle() == DocumentLifecycle::Live
        || lock(&shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(receive.try_recv().is_err());
    let before = snapshot(&shared);
    let form = form_snapshot(&controller);
    let peer_before = snapshot(&peer_shared);
    for invoke in ACTIONS {
        invoke(&controller.window);
        assert_eq!(controller.window.get_message(), "the editor is closing");
        assert_eq!(snapshot(&shared), before);
        assert_eq!(form_snapshot(&controller), form);
        assert_eq!(snapshot(&peer_shared), peer_before);
    }
    peer_controller.window.set_view_name("Peer rename".into());
    peer_controller.window.invoke_rename_view();
    assert_eq!(
        lock(&peer_shared).camera_views().unwrap().views[0].name,
        "Peer rename"
    );
    assert_eq!(peer.lifecycle(), DocumentLifecycle::Live);
    drop(gate);
    receive.recv_timeout(TIMEOUT).unwrap().unwrap();
    thread.join().unwrap();
    let before = snapshot(&shared);
    let entries = lock(&shared).recovery_entries().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let store = crate::recovery::Store {
        directory: directory.path().join("recovery"),
    };
    let (_, recovered) = store.read(&entries[0].id).unwrap();
    for invoke in ACTIONS {
        invoke(&controller.window);
        assert_eq!(controller.window.get_message(), "the document is closed");
        assert_eq!(snapshot(&shared), before);
        assert_eq!(form_snapshot(&controller), form);
    }
    assert_eq!(store.read(&entries[0].id).unwrap().1, recovered);
    controller.window.invoke_close_dialog();
    assert!(!controller.window.window().is_visible());
    assert_eq!(form_snapshot(&controller), form);
    peer.shutdown().unwrap();
}

#[test]
fn retained_saved_views_callbacks_release_runtime_session_and_originating_window() {
    i_slint_backend_testing::init_no_event_loop();
    let (_directory, document, access, shared, controller) = fixture();
    let window = controller.window.clone_strong();
    let weak = Arc::downgrade(&shared);
    drop(controller);
    drop(access);
    drop(document);
    drop(shared);
    assert!(weak.upgrade().is_none());
    for invoke in ACTIONS {
        invoke(&window);
        assert_eq!(window.get_message(), "the document is closed");
    }
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
    let (_directory, document, access, _shared, controller) = fixture();
    let window = controller.window.as_weak();
    let target = access.downgrade();
    drop(controller);
    assert!(window.upgrade().is_none());
    callback(&window, &target, |_, _| {
        panic!("vanished window executed saved-view callback")
    });
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    document.shutdown().unwrap();
}
