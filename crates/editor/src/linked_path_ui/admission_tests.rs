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
    session.set_definitions(
        Actor::User,
        source_formats::fgd::Catalog::parse(
            "@PointClass = path_corner [ targetname(target_source) : \"Name\" target(target_destination) : \"Next\" ]",
        ).unwrap(),
    );
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
    Fixture {
        directory,
        document,
        access,
        shared,
        controller,
    }
}

fn snapshot(shared: &SharedSession) -> (source_formats::vmf::Vmf, Value) {
    let session = lock(shared);
    (
        session.document().vmf().clone(),
        json!({
            "path": session.linked_path(),
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
        "generation": form.generation,
        "token": form.owned_token,
        "dirty": form.dirty,
        "existing_snapshot": form.existing_snapshot,
        "seed": form.existing_seed,
        "prefix": window.get_prefix().to_string(),
        "class": window.get_class_index(),
        "closed": window.get_closed(),
        "existing": window.get_existing(),
        "rows": window.get_points().iter().map(|row| (row.id.to_string(),row.name.to_string(),row.label.to_string(),row.x.to_string(),row.y.to_string(),row.z.to_string(),row.link.to_string())).collect::<Vec<_>>(),
        "removed": window.get_removed_nodes().iter().map(|row| row.to_string()).collect::<Vec<_>>(),
        "enabled": [window.get_editable(),window.get_ready(),window.get_can_review(),window.get_can_load(),window.get_can_preview(),window.get_picking()],
        "status": window.get_preview_status().to_string(),
    })
}

fn pick(window: &LinkedPathWindow) {
    window.invoke_pick_position(1);
}

fn remove(window: &LinkedPathWindow) {
    window.invoke_remove_point(1);
}

fn reorder(window: &LinkedPathWindow) {
    window.invoke_move_point(1, -1);
}

fn close(window: &LinkedPathWindow) {
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
}

fn rejected_actions(controller: &Controller, error: &str) {
    let window = &controller.window;
    let before = form_snapshot(controller);
    let actions: &[fn(&LinkedPathWindow)] = &[
        pick,
        LinkedPathWindow::invoke_cancel_pick,
        LinkedPathWindow::invoke_load_selected,
        LinkedPathWindow::invoke_new_path,
        LinkedPathWindow::invoke_edited,
        LinkedPathWindow::invoke_add_point,
        remove,
        reorder,
        LinkedPathWindow::invoke_preview,
        LinkedPathWindow::invoke_review,
        LinkedPathWindow::invoke_apply,
        LinkedPathWindow::invoke_cancel,
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
fn managed_linked_paths_share_exact_rows_picks_existing_sources_and_review_ownership() {
    software();
    let target = fixture();
    let peer = fixture();
    let peer_before = snapshot(&peer.shared);
    let window = &target.controller.window;
    window.invoke_add_point();
    let mut rows = window.get_points().iter().collect::<Vec<_>>();
    rows[2].x = "384.125".into();
    set_rows(window, rows);
    window.invoke_edited();
    window.invoke_move_point(2, -1);
    window.invoke_remove_point(2);
    assert_eq!(request(window).unwrap().points[1].x, 384.125);
    window.set_prefix("route".into());
    window.invoke_edited();
    window.invoke_preview();
    assert!(window.get_ready(), "{}", window.get_message());
    window.invoke_pick_position(1);
    assert!(!window.window().is_visible());
    let state = lock(&target.shared).linked_path();
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
    assert_eq!(lock(&target.shared).linked_path(), state);
    window.invoke_cancel_pick();
    assert!(!window.get_picking());
    window.invoke_pick_position(1);
    target
        .controller
        .pick_at_pixel(
            &mut lock(&target.shared),
            glam::DVec2::splat(255.2),
            (511, 511),
        )
        .unwrap();
    assert_eq!(
        request(window).unwrap().points[1],
        DVec3::new(3.125, -5.25, 0.0)
    );
    let token = lock(&target.shared).linked_path()["preview"]["token"]
        .as_u64()
        .unwrap();
    let foreign = lock(&target.shared)
        .review_linked_path(Actor::Agent, token)
        .unwrap();
    window.invoke_pick_position(1);
    window.invoke_apply();
    window.invoke_cancel();
    assert_eq!(lock(&target.shared).linked_path(), foreign);
    target.controller.show(&lock(&target.shared)).unwrap();
    window.invoke_review();
    assert!(window.get_ready(), "{}", window.get_message());
    let state = lock(&target.shared).linked_path();
    let seed =
        crate::identity::parse_id(state["preview"]["nodes"][0]["id"].as_str().unwrap()).unwrap();
    let history = lock(&target.shared).document().undo_labels().count();
    window.invoke_apply();
    assert_eq!(
        lock(&target.shared).document().undo_labels().count(),
        history + 1
    );
    lock(&target.shared)
        .select(&[seed], crate::session::SelectMode::Replace)
        .unwrap();
    target.controller.show(&lock(&target.shared)).unwrap();
    window.invoke_load_selected();
    assert!(window.get_existing(), "{}", window.get_message());
    let source_ids = window
        .get_points()
        .iter()
        .map(|row| row.id.to_string())
        .collect::<Vec<_>>();
    window.invoke_preview();
    assert!(window.get_ready(), "{}", window.get_message());
    assert_eq!(
        window
            .get_points()
            .iter()
            .map(|row| row.id.to_string())
            .collect::<Vec<_>>(),
        source_ids
    );
    window.invoke_cancel();
    target.controller.show(&lock(&target.shared)).unwrap();
    window.invoke_new_path();
    assert!(!window.get_existing());
    assert!(window.get_points().iter().all(|row| row.id.is_empty()));
    lock(&target.shared).new_map(Actor::Agent, true).unwrap();
    let before = snapshot(&target.shared);
    window.invoke_preview();
    window.invoke_load_selected();
    window.invoke_apply();
    assert_eq!(snapshot(&target.shared), before);
    assert_eq!(snapshot(&peer.shared), peer_before);
    target.document.shutdown().unwrap();
    peer.document.shutdown().unwrap();
}

#[test]
fn linked_paths_reject_every_callback_during_drain_after_finish_and_after_failure() {
    software();
    let target = fixture();
    let peer = fixture();
    target.controller.window.invoke_preview();
    target.controller.window.set_prefix("pending_route".into());
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
    assert_eq!(lock(&peer.shared).document().vmf().entities.len(), 2);
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
fn retained_linked_paths_release_session_and_abandon_a_vanished_window() {
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
        panic!("vanished linked path window admitted work")
    });
    assert_eq!(fixture.document.lifecycle(), DocumentLifecycle::Live);
    fixture.document.shutdown().unwrap();
}
