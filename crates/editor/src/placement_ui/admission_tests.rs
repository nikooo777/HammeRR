use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use glam::DVec3;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Fixture {
    document: DocumentHandle,
    access: DocumentAccess,
    shared: SharedSession,
    controller: Controller,
    id: ObjectId,
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
    let (mut session, id) = crate::test_assets::placement();
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    controller.initialize(&mut lock(&shared)).unwrap();
    controller.window.set_offset("4".into());
    controller.window.show().unwrap();
    Fixture {
        directory,
        document,
        access,
        shared,
        controller,
        id,
    }
}

fn snapshot(shared: &SharedSession) -> (source_formats::vmf::Vmf, source_formats::vmf::Vmf, Value) {
    let session = lock(shared);
    (
        session.document().vmf().clone(),
        session.render_document().vmf().clone(),
        json!({
            "placement": session.surface_placement(),
            "history": session.history_snapshot(),
            "camera": format!("{:?}", session.camera()),
            "selection": session.selected_ids().into_iter().map(|id| id.to_string()).collect::<Vec<_>>(),
            "serials": [session.scene_serial(), session.view_serial(), session.log_serial(), session.camera_serial()],
        }),
    )
}

fn form_snapshot(controller: &Controller) -> Value {
    let context = controller.context.borrow();
    let window = &controller.window;
    json!({
        "context": context.as_ref().map(|context| (context.generation, context.ids.iter().map(|id|id.to_string()).collect::<Vec<_>>(), format!("{:?}",context.source_corner))),
        "token": controller.token.get(),
        "dirty": controller.dirty.get(),
        "picking": controller.picking.get(),
        "offset": window.get_offset().to_string(),
        "method": window.get_method(),
        "align": window.get_align(),
        "texture_lock": window.get_texture_lock(),
        "ready": window.get_ready(),
        "window_picking": window.get_picking(),
        "summary": window.get_summary().to_string(),
        "source_corner": window.get_source_corner().to_string(),
        "selection": window.get_selection().to_string(),
    })
}

fn close(window: &PlacementWindow) {
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
}

fn rejected_actions(controller: &Controller, error: &str) {
    let window = &controller.window;
    let before = form_snapshot(controller);
    let actions: &[fn(&PlacementWindow)] = &[
        PlacementWindow::invoke_invalidate,
        PlacementWindow::invoke_preview,
        PlacementWindow::invoke_apply,
        PlacementWindow::invoke_frame,
        PlacementWindow::invoke_cancel,
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
fn managed_placement_preserves_selection_exact_picks_and_foreign_transform_ownership() {
    software();
    let target = fixture();
    let peer = fixture();
    let peer_before = snapshot(&peer.shared);
    let window = &target.controller.window;
    let original = lock(&target.shared).document().vmf().clone();
    window.set_method(2);
    window.set_align(true);
    window.invoke_invalidate();
    window.invoke_preview();
    assert!(target.controller.is_picking(), "{}", window.get_message());
    assert!(!window.window().is_visible());
    assert!(
        target
            .controller
            .pick_at_ray(
                &mut lock(&target.shared),
                Ray {
                    origin: DVec3::splat(1000.0),
                    direction: DVec3::Z
                }
            )
            .is_err()
    );
    assert!(target.controller.is_picking());
    target
        .controller
        .pick_at_ray(
            &mut lock(&target.shared),
            Ray {
                origin: DVec3::new(0.0, 0.0, 64.0),
                direction: DVec3::Y,
            },
        )
        .unwrap();
    assert!(!target.controller.is_picking());
    assert!(window.get_ready());
    window.invoke_frame();
    window.set_offset("8".into());
    window.invoke_invalidate();
    window.invoke_apply();
    assert_eq!(lock(&target.shared).document().vmf(), &original);
    window.set_method(0);
    window.invoke_preview();
    let history = lock(&target.shared).document().undo_labels().count();
    window.invoke_apply();
    assert_eq!(
        lock(&target.shared).document().undo_labels().count(),
        history + 1
    );
    lock(&target.shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&target.shared).document().vmf(), &original);
    target
        .controller
        .initialize(&mut lock(&target.shared))
        .unwrap();
    window.invoke_preview();
    {
        let mut session = lock(&target.shared);
        let transform = session.transform_preview().unwrap().transform;
        session
            .preview_transform(Actor::Agent, &[target.id], transform, Some(transform.pivot))
            .unwrap();
    }
    let foreign = snapshot(&target.shared);
    window.invoke_apply();
    window.invoke_frame();
    assert_eq!(snapshot(&target.shared), foreign);
    let foreign_transform = lock(&target.shared).transform_preview().unwrap().transform;
    window.invoke_cancel();
    let after_cancel = snapshot(&target.shared);
    assert_eq!(after_cancel.0, foreign.0);
    assert_eq!(after_cancel.1, foreign.1);
    assert_eq!(after_cancel.2["history"], foreign.2["history"]);
    assert_eq!(
        lock(&target.shared).transform_preview().unwrap().transform,
        foreign_transform
    );
    lock(&target.shared).cancel_transform_preview();
    target
        .controller
        .initialize(&mut lock(&target.shared))
        .unwrap();
    window.set_method(2);
    window.invoke_preview();
    let old_token = target.controller.token.get();
    {
        let mut session = lock(&target.shared);
        session
            .begin_surface_placement_pick(
                &[target.id],
                Options {
                    source_corner: None,
                    align: false,
                    offset: 0.0,
                    texture_lock: true,
                },
            )
            .unwrap();
    }
    assert_ne!(
        lock(&target.shared).surface_placement()["token"].as_u64(),
        old_token
    );
    let replacement = snapshot(&target.shared);
    assert!(
        target
            .controller
            .pick_at_ray(
                &mut lock(&target.shared),
                Ray {
                    origin: DVec3::new(0.0, 0.0, 64.0),
                    direction: DVec3::Y
                }
            )
            .is_err()
    );
    window.show().unwrap();
    close(window);
    assert!(!window.window().is_visible());
    assert_eq!(snapshot(&target.shared), replacement);
    lock(&target.shared).new_map(Actor::Agent, true).unwrap();
    let before = snapshot(&target.shared);
    window.invoke_preview();
    window.invoke_apply();
    window.invoke_frame();
    assert_eq!(snapshot(&target.shared), before);
    assert_eq!(snapshot(&peer.shared), peer_before);
    target.document.shutdown().unwrap();
    peer.document.shutdown().unwrap();
}

#[test]
fn placement_rejects_every_callback_during_drain_after_finish_and_after_failure() {
    software();
    let target = fixture();
    let peer = fixture();
    target.controller.window.invoke_preview();
    target.controller.window.set_offset("17.125".into());
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
    let peer_history = lock(&peer.shared).document().undo_labels().count();
    peer.controller.window.invoke_preview();
    peer.controller.window.invoke_apply();
    assert!(!peer.controller.window.get_ready() || !peer.controller.window.window().is_visible());
    assert_eq!(
        lock(&peer.shared).document().undo_labels().count(),
        peer_history + 1
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
fn retained_placement_releases_session_and_abandon_a_vanished_window() {
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
        panic!("vanished placement window admitted work")
    });
    assert_eq!(fixture.document.lifecycle(), DocumentLifecycle::Live);
    fixture.document.shutdown().unwrap();
}

#[test]
fn managed_corner_placement_retains_the_exact_component_and_rejects_changed_selection() {
    software();
    let target = fixture();
    let ObjectId::Solid(solid) = target.id else {
        unreachable!()
    };
    let component = {
        let mut session = lock(&target.shared);
        let shape = session.component_topology(&[solid]).unwrap().remove(0);
        let component = Component {
            solid,
            sides: shape.vertices[0].sides.clone(),
        };
        let stamp = session.component_stamp();
        session
            .select_components(
                &[component.clone()],
                crate::session::SelectMode::Replace,
                &stamp,
            )
            .unwrap();
        component
    };
    target
        .controller
        .initialize(&mut lock(&target.shared))
        .unwrap();
    assert_eq!(
        target
            .controller
            .context
            .borrow()
            .as_ref()
            .unwrap()
            .source_corner,
        Some(component)
    );
    target.controller.window.set_method(3);
    target.controller.window.set_offset("0".into());
    target.controller.window.invoke_preview();
    assert!(target.controller.is_picking());
    target
        .controller
        .pick_at_ray(
            &mut lock(&target.shared),
            Ray {
                origin: DVec3::new(-200.0, -200.0, 100.0),
                direction: DVec3::NEG_Z,
            },
        )
        .unwrap();
    assert!(
        target
            .controller
            .window
            .get_summary()
            .contains("Target corner")
    );
    lock(&target.shared)
        .select(&[], crate::session::SelectMode::Replace)
        .unwrap();
    let before = snapshot(&target.shared);
    target.controller.window.invoke_apply();
    target.controller.window.invoke_preview();
    target.controller.window.invoke_frame();
    assert_eq!(snapshot(&target.shared), before);
    target.controller.window.invoke_cancel();
    target.document.shutdown().unwrap();
}
