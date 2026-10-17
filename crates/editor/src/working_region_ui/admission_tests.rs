use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use serde_json::json;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);
const ACTIONS: [fn(&WorkingRegionWindow); 3] = [
    WorkingRegionWindow::invoke_apply,
    WorkingRegionWindow::invoke_use_bounds,
    WorkingRegionWindow::invoke_clear,
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
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    controller.show(&mut lock(&shared)).unwrap();
    (directory, document, access, shared, controller)
}

fn snapshot(shared: &SharedSession) -> (source_formats::vmf::Vmf, Value) {
    let session = lock(shared);
    (
        session.document().vmf().clone(),
        json!({
            "history": session.history_snapshot(),
            "camera": format!("{:?}", session.camera()),
            "selected": session.selected_ids().into_iter().map(|id| id.to_string()).collect::<Vec<_>>(),
            "region": session.working_region_summary(),
            "scene_serial": session.scene_serial(),
            "log_serial": session.log_serial(),
        }),
    )
}

fn form_snapshot(controller: &Controller) -> Value {
    json!({
        "snapshot": *controller.snapshot.borrow(),
        "fields": [controller.window.get_min_x().to_string(),controller.window.get_min_y().to_string(),controller.window.get_min_z().to_string(),controller.window.get_max_x().to_string(),controller.window.get_max_y().to_string(),controller.window.get_max_z().to_string()],
        "enabled": controller.window.get_region_enabled(),
        "summary": controller.window.get_summary().to_string(),
    })
}

#[test]
fn managed_region_callbacks_preserve_review_snapshots_and_live_peer() {
    i_slint_backend_testing::init_no_event_loop();
    let (_directory, document, _access, shared, controller) = fixture();
    let (_peer_directory, peer, _peer_access, peer_shared, _peer_controller) = fixture();
    let peer_before = snapshot(&peer_shared);
    controller.window.invoke_use_bounds();
    controller.window.invoke_apply();
    assert!(lock(&shared).working_region_enabled());
    {
        let mut session = lock(&shared);
        let stamp = session.working_region_stamp();
        session
            .set_working_region(
                Actor::Agent,
                Action::Set {
                    bounds: [DVec3::splat(100.0), DVec3::splat(200.0)],
                    enabled: true,
                },
                &stamp,
            )
            .unwrap();
    }
    controller.window.set_min_x("17".into());
    let before = snapshot(&shared);
    let form = form_snapshot(&controller);
    for invoke in ACTIONS {
        invoke(&controller.window);
        assert!(controller.window.get_message().contains("changed"));
        assert_eq!(snapshot(&shared), before);
        assert_eq!(form_snapshot(&controller), form);
    }
    controller.refresh(&mut lock(&shared));
    controller.window.invoke_use_bounds();
    controller.window.invoke_apply();
    controller.window.invoke_clear();
    assert!(!lock(&shared).working_region_enabled());
    assert_eq!(snapshot(&peer_shared), peer_before);
    assert_eq!(peer.lifecycle(), DocumentLifecycle::Live);
    document.shutdown().unwrap();
    peer.shutdown().unwrap();
}

#[test]
fn all_region_callbacks_preserve_pending_fields_and_region_during_real_drain_and_finish() {
    i_slint_backend_testing::init_no_event_loop();
    let (directory, document, _access, shared, controller) = fixture();
    let (_peer_directory, peer, _peer_access, peer_shared, peer_controller) = fixture();
    controller.window.invoke_use_bounds();
    controller.window.invoke_apply();
    controller.window.set_min_x("17".into());
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
    peer_controller.window.invoke_use_bounds();
    peer_controller.window.invoke_apply();
    assert!(lock(&peer_shared).working_region_enabled());
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
    controller.window.invoke_cancel();
    assert!(!controller.window.window().is_visible());
    assert_eq!(form_snapshot(&controller), form);
    peer.shutdown().unwrap();
}

#[test]
fn retained_region_callbacks_release_runtime_session_and_originating_window() {
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
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    let (_directory, document, access, _shared, controller) = fixture();
    let window = controller.window.as_weak();
    let target = access.downgrade();
    controller.window.hide().unwrap();
    drop(controller);
    assert!(window.upgrade().is_none());
    callback(&window, &target, |_, _| {
        panic!("vanished window executed region callback")
    });
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    document.shutdown().unwrap();
}
