use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use serde_json::json;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Fixture {
    document: DocumentHandle,
    access: DocumentAccess,
    shared: SharedSession,
    controller: Controller,
    origin: map_core::ObjectId,
    directory: tempfile::TempDir,
}

fn software() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session
        .create_block(
            Actor::User,
            DVec3::new(-128.0, -128.0, -16.0),
            DVec3::new(128.0, 128.0, 0.0),
            "brick/test",
            false,
        )
        .unwrap();
    let origin = session
        .edit(Actor::User, "origin", |tx| {
            map_core::edit::create_entity(
                tx,
                "logic_relay",
                Some(DVec3::new(16.125, 32.25, 64.5)),
                &[],
                Vec::new(),
            )
        })
        .unwrap();
    session.set_camera(render::Camera::planar(
        render::Projection::Top,
        glam::Vec3::new(3.125, -5.25, 0.0),
        256.0,
    ));
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    controller.set_viewport((511, 511));
    controller.show(&lock(&shared)).unwrap();
    controller
        .window
        .set_measurement_name("Pending ruler".into());
    Fixture {
        directory,
        document,
        access,
        shared,
        controller,
        origin,
    }
}

fn snapshot(shared: &SharedSession) -> (source_formats::vmf::Vmf, Value) {
    let session = lock(shared);
    (
        session.document().vmf().clone(),
        json!({
            "measurements": session.measurement_state(),
            "history": session.history_snapshot(),
            "camera": format!("{:?}", session.camera()),
            "selection": session.selected_ids().into_iter().map(|id| id.to_string()).collect::<Vec<_>>(),
            "serials": [session.scene_serial(), session.view_serial(), session.log_serial(), session.camera_serial()],
        }),
    )
}

fn form_snapshot(controller: &Controller) -> Value {
    let form = controller.form.borrow();
    let window = &controller.window;
    json!({
        "guard": [form.generation,form.revision],
        "snapshot": form.snapshot,
        "selected": form.selected,
        "tokens": [form.owned_token,form.displayed_token],
        "dirty": form.dirty,
        "fields": [window.get_measurement_name().to_string(),window.get_start_x().to_string(),window.get_start_y().to_string(),window.get_start_z().to_string(),window.get_end_x().to_string(),window.get_end_y().to_string(),window.get_end_z().to_string(),window.get_start_origin_object().to_string(),window.get_end_origin_object().to_string()],
        "enabled": [window.get_editable(),window.get_ready(),window.get_can_review(),window.get_can_pick(),window.get_can_attach(),window.get_can_delete(),window.get_can_frame(),window.get_picking(),window.get_show_guides()],
        "result": window.get_result().to_string(),
    })
}

fn choose(window: &MeasurementsWindow) {
    window.invoke_choose("1".into());
}

fn close(window: &MeasurementsWindow) {
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
}

fn rejected_actions(controller: &Controller, error: &str) {
    let window = &controller.window;
    let before = form_snapshot(controller);
    let actions: &[fn(&MeasurementsWindow)] = &[
        MeasurementsWindow::invoke_attach_start_origin,
        MeasurementsWindow::invoke_attach_end_origin,
        MeasurementsWindow::invoke_detach_start_origin,
        MeasurementsWindow::invoke_detach_end_origin,
        MeasurementsWindow::invoke_world_start,
        MeasurementsWindow::invoke_world_end,
        MeasurementsWindow::invoke_pick_start,
        MeasurementsWindow::invoke_pick_end,
        MeasurementsWindow::invoke_cancel_pick,
        MeasurementsWindow::invoke_edited,
        choose,
        MeasurementsWindow::invoke_new_measurement,
        MeasurementsWindow::invoke_preview,
        MeasurementsWindow::invoke_review,
        MeasurementsWindow::invoke_apply,
        MeasurementsWindow::invoke_delete_measurement,
        MeasurementsWindow::invoke_frame,
        MeasurementsWindow::invoke_view_changed,
        MeasurementsWindow::invoke_cancel,
        close,
    ];
    for (index, action) in actions.iter().enumerate() {
        window.show().unwrap();
        action(window);
        assert_eq!(window.get_message(), error, "action {index}");
        assert_eq!(form_snapshot(controller), before, "action {index}");
        if index >= actions.len() - 2 {
            assert!(!window.window().is_visible());
        }
    }
}

#[test]
fn managed_measurements_share_exact_attachments_picks_reviews_and_history() {
    software();
    let target = fixture();
    let peer = fixture();
    let peer_before = snapshot(&peer.shared);
    let window = &target.controller.window;
    window.set_measurement_name("Exact ruler".into());
    window.invoke_edited();
    window.invoke_preview();
    assert!(window.get_ready(), "{}", window.get_message());
    window.set_start_origin_object(target.origin.to_string().into());
    window.invoke_attach_start_origin();
    assert!(window.get_start_attached());
    assert_eq!(window.get_start_x(), "16.125");
    window.invoke_detach_start_origin();
    assert!(!window.get_start_attached());
    window.set_end_origin_object(target.origin.to_string().into());
    window.invoke_attach_end_origin();
    assert!(window.get_end_attached());
    window.invoke_world_end();
    assert!(!window.get_end_attached());
    window.invoke_pick_start();
    assert!(!window.window().is_visible());
    let state = lock(&target.shared).measurement_state();
    assert!(
        target
            .controller
            .pick_at_pixel(
                &mut lock(&target.shared),
                glam::DVec2::splat(255.2),
                (512, 511)
            )
            .is_err()
    );
    assert_eq!(lock(&target.shared).measurement_state(), state);
    target
        .controller
        .pick_at_pixel(
            &mut lock(&target.shared),
            glam::DVec2::splat(255.2),
            (511, 511),
        )
        .unwrap();
    assert_eq!(window.get_start_x(), "3.125");
    window.invoke_pick_end();
    window.invoke_cancel_pick();
    window.invoke_frame();
    let history = lock(&target.shared).document().undo_labels().count();
    window.invoke_apply();
    assert_eq!(
        lock(&target.shared).document().undo_labels().count(),
        history + 1
    );
    window.invoke_choose("1".into());
    window.set_show_guides(false);
    window.invoke_view_changed();
    assert_eq!(
        lock(&target.shared).measurement_state()["show_guides"],
        false
    );
    window.invoke_delete_measurement();
    assert!(
        lock(&target.shared)
            .measurements()
            .unwrap()
            .measurements
            .is_empty()
    );
    window.invoke_new_measurement();
    window.set_measurement_name("Reviewed ruler".into());
    window.invoke_preview();
    let token = lock(&target.shared).measurement_state()["draft"]["token"]
        .as_u64()
        .unwrap();
    let foreign = lock(&target.shared)
        .review_measurement(Actor::Agent, token)
        .unwrap();
    window.invoke_pick_end();
    window.invoke_apply();
    assert_eq!(lock(&target.shared).measurement_state(), foreign);
    target.controller.refresh(&lock(&target.shared));
    window.invoke_review();
    assert_eq!(window.get_measurement_name(), "Reviewed ruler");
    window.invoke_cancel();
    assert!(lock(&target.shared).measurement_state()["draft"].is_null());
    lock(&target.shared).new_map(Actor::Agent, true).unwrap();
    let before = snapshot(&target.shared);
    window.invoke_preview();
    window.invoke_delete_measurement();
    assert_eq!(snapshot(&target.shared), before);
    assert_eq!(snapshot(&peer.shared), peer_before);
    target.document.shutdown().unwrap();
    peer.document.shutdown().unwrap();
}

#[test]
fn measurements_reject_every_callback_during_drain_after_finish_and_after_failure() {
    software();
    let target = fixture();
    let peer = fixture();
    target.controller.window.invoke_preview();
    target.controller.window.set_start_x("17.125".into());
    let gate = target.document.admit().unwrap();
    let draining = target.document.clone();
    let (send, receive) = mpsc::channel();
    let thread = std::thread::spawn(move || send.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + TIMEOUT;
    while target.document.lifecycle() == DocumentLifecycle::Live
        || lock(&target.shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(receive.try_recv().is_err());
    let before = snapshot(&target.shared);
    let peer_before = snapshot(&peer.shared);
    rejected_actions(&target.controller, "the editor is closing");
    assert_eq!(snapshot(&target.shared), before);
    assert_eq!(snapshot(&peer.shared), peer_before);
    peer.controller.window.invoke_preview();
    peer.controller.window.invoke_apply();
    assert_eq!(
        lock(&peer.shared)
            .measurements()
            .unwrap()
            .measurements
            .len(),
        1
    );
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    drop(gate);
    receive.recv_timeout(TIMEOUT).unwrap().unwrap();
    thread.join().unwrap();
    assert_eq!(target.document.lifecycle(), DocumentLifecycle::Finished);
    let before = snapshot(&target.shared);
    let entries = lock(&target.shared).recovery_entries().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let store = crate::recovery::Store {
        directory: target.directory.path().join("recovery"),
    };
    let recovered = store.read(&entries[0].id).unwrap().1;
    assert_eq!(
        source_formats::vmf::Vmf::parse(&recovered).unwrap(),
        before.0
    );
    rejected_actions(&target.controller, "the document is closed");
    assert_eq!(snapshot(&target.shared), before);
    assert_eq!(store.read(&entries[0].id).unwrap().1, recovered);
    peer.document.shutdown().unwrap();

    let failed = fixture();
    failed.controller.window.invoke_preview();
    let recovery = failed.directory.path().join("recovery");
    if recovery.exists() {
        std::fs::remove_dir_all(&recovery).unwrap();
    }
    std::fs::write(&recovery, b"blocked").unwrap();
    let error = failed.document.shutdown().unwrap_err();
    assert_eq!(
        failed.document.lifecycle(),
        DocumentLifecycle::Failed(error.clone())
    );
    let before = snapshot(&failed.shared);
    rejected_actions(
        &failed.controller,
        &format!("document shutdown failed: {error}"),
    );
    assert_eq!(snapshot(&failed.shared), before);
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    assert_eq!(failed.document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(recovery).unwrap().count(), 0);
}

#[test]
fn retained_measurements_release_session_and_abandon_a_vanished_window() {
    software();
    let fixture = fixture();
    let window = fixture.controller.window.clone_strong();
    window.hide().unwrap();
    let weak_session = Arc::downgrade(&fixture.shared);
    let weak_access = fixture.access.downgrade();
    drop(fixture);
    assert!(weak_session.upgrade().is_none());
    assert!(weak_access.upgrade().is_err());
    window.invoke_preview();
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_cancel();
    assert!(!window.window().is_visible());

    let fixture = self::fixture();
    let weak_window = fixture.controller.window.as_weak();
    let target = fixture.access.downgrade();
    fixture.controller.window.hide().unwrap();
    drop(fixture.controller);
    assert!(weak_window.upgrade().is_none());
    with_session(&weak_window, &target, |_, _| {
        panic!("vanished measurement window admitted work")
    });
    assert_eq!(fixture.document.lifecycle(), DocumentLifecycle::Live);
    fixture.document.shutdown().unwrap();
}
