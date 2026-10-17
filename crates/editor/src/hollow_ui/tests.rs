use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle, lock};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

fn fixture() -> (
    tempfile::TempDir,
    DocumentHandle,
    SharedSession,
    DocumentAccess,
    HollowWindow,
) {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets required".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(128.0),
            "brick/test",
            false,
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let window = HollowWindow::new().unwrap();
    bind_with_access(&window, &access);
    access
        .with_session(|session| {
            refresh(&window, session);
            Ok(())
        })
        .unwrap();
    (directory, document, shared, access, window)
}

fn fields(window: &HollowWindow) -> Value {
    json!({
        "faces":window.get_faces().iter().map(|row| json!({"key":row.key.to_string(),"label":row.label.to_string(),"open":row.open})).collect::<Vec<_>>(),
        "source":window.get_source().to_string(), "stamp":window.get_stamp().to_string(),
        "token":window.get_token().to_string(), "summary":window.get_summary().to_string(),
        "selection":window.get_selection_label().to_string(), "ready":window.get_ready(),
        "can_preview":window.get_can_preview(), "cavities":window.get_cavities(),
        "cavity":window.get_cavity(), "direction":window.get_direction(),
        "thickness":window.get_thickness().to_string()
    })
}

fn state(session: &Session) -> Value {
    json!({
        "map":session.document().vmf().write(&Default::default()),
        "draft":session.render_document().vmf().write(&Default::default()),
        "hollow":session.hollow_state(), "history":session.history_snapshot(),
        "selection":format!("{:?}",session.selected_ids()),
        "camera":format!("{:?}",session.camera()), "view":session.view_serial()
    })
}

fn reject_actions(window: &HollowWindow, expected: &Value, error: &str) {
    let key = window.get_faces().iter().next().unwrap().key;
    window.invoke_choose_opening(key);
    assert_eq!(fields(window), *expected);
    assert_eq!(window.get_message(), error);
    for action in [
        HollowWindow::invoke_close_openings,
        HollowWindow::invoke_edited,
        HollowWindow::invoke_preview_edit,
        HollowWindow::invoke_look_inside,
        HollowWindow::invoke_apply,
    ] {
        action(window);
        assert_eq!(fields(window), *expected);
        assert_eq!(window.get_message(), error);
    }
}

#[test]
fn managed_hollow_acts_live_then_rejects_retained_actions_during_drain_and_keeps_peer_live() {
    i_slint_backend_testing::init_no_event_loop();
    let (directory, document, shared, _access, window) = fixture();
    let source = lock(&shared).document().vmf().clone();
    let selected = lock(&shared).selected_ids();
    window.invoke_preview_edit();
    assert!(window.get_ready());
    let top = window
        .get_faces()
        .iter()
        .find(|face| face.label.starts_with("Top"))
        .unwrap();
    window.invoke_choose_opening(top.key);
    assert!(!window.get_ready());
    window.invoke_preview_edit();
    assert_eq!(
        lock(&shared).hollow_state()["preview"]["result"]["brushes"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    let before_camera = lock(&shared).camera();
    window.invoke_look_inside();
    assert_ne!(lock(&shared).camera(), before_camera);
    window.invoke_apply();
    assert_ne!(lock(&shared).document().vmf(), &source);
    lock(&shared).undo(Actor::User).unwrap();
    lock(&shared)
        .select(&selected, crate::session::SelectMode::Replace)
        .unwrap();
    refresh(&window, &lock(&shared));
    window.invoke_close_openings();
    window.invoke_preview_edit();
    assert!(window.get_ready());
    window.show().unwrap();
    let (_peer_directory, peer_document, peer_shared, _peer_access, peer_window) = fixture();
    peer_window.invoke_preview_edit();
    let peer_source = lock(&peer_shared).document().vmf().clone();
    let peer_state = state(&lock(&peer_shared));
    let peer_fields = fields(&peer_window);
    let expected_fields = fields(&window);
    let gate = document.admit().unwrap();
    let draining = document.clone();
    let (done, completed) = mpsc::sync_channel(1);
    let drain = std::thread::spawn(move || done.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + TIMEOUT;
    while document.lifecycle() == DocumentLifecycle::Live
        || lock(&shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline, "document did not begin draining");
        std::thread::yield_now();
    }
    let expected_state = state(&lock(&shared));
    reject_actions(&window, &expected_fields, "the editor is closing");
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    window.show().unwrap();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!window.window().is_visible());
    assert_eq!(fields(&window), expected_fields);
    assert_eq!(state(&lock(&shared)), expected_state);
    assert!(completed.try_recv().is_err());
    drop(gate);
    completed.recv_timeout(TIMEOUT).unwrap().unwrap();
    drain.join().unwrap();
    let store = crate::recovery::Store {
        directory: directory.path().join("recovery"),
    };
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let recovered = store.read(&entries[0].id).unwrap().1;
    assert_eq!(source_formats::vmf::Vmf::parse(&recovered).unwrap(), source);
    reject_actions(&window, &expected_fields, "the document is closed");
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    window.show().unwrap();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!window.window().is_visible());
    assert_eq!(fields(&window), expected_fields);
    assert_eq!(state(&lock(&shared)), expected_state);
    assert_eq!(store.list().unwrap().entries.len(), 1);
    assert_eq!(store.read(&entries[0].id).unwrap().1, recovered);
    assert_eq!(state(&lock(&peer_shared)), peer_state);
    assert_eq!(fields(&peer_window), peer_fields);
    peer_window.invoke_apply();
    assert_ne!(lock(&peer_shared).document().vmf(), &peer_source);
    assert_eq!(peer_document.lifecycle(), DocumentLifecycle::Live);
}

#[test]
fn managed_hollow_preserves_exact_foreign_preview_and_stale_source_checks() {
    i_slint_backend_testing::init_no_event_loop();
    let (_directory, _document, shared, access, window) = fixture();
    window.invoke_preview_edit();
    let own = window.get_token();
    access
        .with_session(|session| {
            let ids = session.selected_ids();
            let stamp = session.hollow_stamp();
            session.preview_hollow(
                &ids,
                map_core::hollow::Spec {
                    thickness: 8.0,
                    outward: true,
                    open_faces: Default::default(),
                },
                &stamp,
            )
        })
        .unwrap();
    let foreign = state(&lock(&shared));
    assert_ne!(
        lock(&shared).hollow_state()["preview"]["token"],
        own.to_string()
    );
    window.invoke_look_inside();
    assert!(window.get_message().contains("closed or replaced"));
    window.show().unwrap();
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    assert_eq!(state(&lock(&shared)), foreign);
    window.show().unwrap();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!window.window().is_visible());
    assert_eq!(state(&lock(&shared)), foreign);
    window.invoke_apply();
    assert!(window.get_message().contains("closed or replaced"));
    assert_eq!(state(&lock(&shared)), foreign);
    lock(&shared)
        .create_block(
            Actor::User,
            glam::DVec3::splat(256.0),
            glam::DVec3::splat(320.0),
            "brick/test",
            false,
        )
        .unwrap();
    let changed = state(&lock(&shared));
    window.invoke_preview_edit();
    assert!(
        window
            .get_message()
            .contains("Map, selection or preview changed")
    );
    assert_eq!(state(&lock(&shared)), changed);
}

#[test]
fn hollow_action_holds_one_operation_through_nested_form_invalidation_during_drain() {
    i_slint_backend_testing::init_no_event_loop();
    let (directory, document, shared, access, window) = fixture();
    window.invoke_preview_edit();
    assert!(window.get_ready());
    let key = window.get_faces().iter().next().unwrap().key;
    let (done, completed) = mpsc::sync_channel(1);
    let drain = with_session(
        &window.as_weak(),
        &access.downgrade(),
        |window, _session| {
            let draining = document.clone();
            let drain = std::thread::spawn(move || done.send(draining.shutdown()).unwrap());
            let deadline = Instant::now() + TIMEOUT;
            while document.lifecycle() == DocumentLifecycle::Live {
                assert!(
                    Instant::now() < deadline,
                    "document did not close admission"
                );
                std::thread::yield_now();
            }
            choose_opening(window, &key);
            assert!(!window.get_ready());
            assert_eq!(openings(window).unwrap().len(), 1);
            assert!(window.get_message().starts_with("Settings changed"));
            assert!(completed.try_recv().is_err());
            assert!(!directory.path().join("recovery").exists());
            drain
        },
    )
    .unwrap();
    completed.recv_timeout(TIMEOUT).unwrap().unwrap();
    drain.join().unwrap();
    assert_eq!(document.lifecycle(), DocumentLifecycle::Finished);
    assert_eq!(lock(&shared).document().vmf().world.solids.len(), 1);
    assert_eq!(
        crate::recovery::Store {
            directory: directory.path().join("recovery")
        }
        .list()
        .unwrap()
        .entries
        .len(),
        1
    );
}

#[test]
fn retained_hollow_window_has_no_runtime_owner_and_missing_window_abandons_before_admission() {
    i_slint_backend_testing::init_no_event_loop();
    let (_directory, document, shared, access, window) = fixture();
    let gone = HollowWindow::new().unwrap();
    let weak_window = gone.as_weak();
    drop(gone);
    assert!(
        with_session(&weak_window, &access.downgrade(), |_, _| panic!(
            "vanished child accessed Session"
        ))
        .is_none()
    );
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    window.invoke_preview_edit();
    let expected = fields(&window);
    let weak_session = Arc::downgrade(&shared);
    drop(access);
    drop(shared);
    drop(document);
    assert!(weak_session.upgrade().is_none());
    reject_actions(&window, &expected, "the document is closed");
    window.show().unwrap();
    window.invoke_cancel();
    assert!(!window.window().is_visible());
}

#[test]
fn hollow_failed_drain_is_memoized_and_local_dismissal_preserves_preview() {
    i_slint_backend_testing::init_no_event_loop();
    let (directory, document, shared, _access, window) = fixture();
    window.invoke_preview_edit();
    let fields = fields(&window);
    let recovery = directory.path().join("recovery");
    std::fs::write(&recovery, b"not a directory").unwrap();
    let error = document.shutdown().unwrap_err();
    let expected_state = state(&lock(&shared));
    let expected_error = format!("document shutdown failed: {error}");
    reject_actions(&window, &fields, &expected_error);
    window.show().unwrap();
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    assert_eq!(window.get_message(), expected_error);
    assert_eq!(state(&lock(&shared)), expected_state);
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    assert_eq!(document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
}
