use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle, lock};
use crate::session::SelectMode;
use glam::DVec3;
use map_core::{ObjectId, query::Ray, surface::FaceId};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Fixture {
    document: DocumentHandle,
    access: DocumentAccess,
    shared: SharedSession,
    controller: Controller,
    face: FaceId,
    directory: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let ObjectId::Solid(solid) = session
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(64.0), "test", false)
        .unwrap()
    else {
        unreachable!()
    };
    let side = session
        .document()
        .solid(solid)
        .unwrap()
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let face = FaceId { solid, side };
    session.create_terrain(Actor::User, &[face], 2, 0).unwrap();
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    access
        .with_session(|session| controller.initialize(session))
        .unwrap();
    Fixture {
        directory,
        document,
        access,
        shared,
        controller,
        face,
    }
}

fn fields(controller: &Controller) -> Value {
    let window = &controller.window;
    let target = controller.target.borrow();
    let target = target.as_ref().unwrap();
    json!({
        "selection":window.get_selection_label().to_string(),
        "stale":window.get_stale(), "ready":window.get_ready(),
        "mask_available":window.get_mask_available(), "mask":window.get_mask_label().to_string(),
        "use_mask":window.get_use_mask(), "grid":window.get_grid_index(),
        "operation":window.get_operation_index(), "direction":window.get_direction_index(),
        "physics":window.get_physics_index(), "player":window.get_player_index(),
        "ray":window.get_ray_index(), "sew":window.get_sew_blend(),
        "smoothing":window.get_smoothing_index(), "angle":window.get_smoothing_angle().to_string(),
        "center":[window.get_center_x().to_string(),window.get_center_y().to_string(),window.get_center_z().to_string()],
        "radius":window.get_radius().to_string(), "amount":window.get_amount().to_string(),
        "strength":window.get_strength().to_string(), "blend":window.get_blend_value().to_string(),
        "seed":window.get_seed().to_string(),
        "vertex":[window.get_vertex_x().to_string(),window.get_vertex_y().to_string(),window.get_vertex_z().to_string()],
        "sliders":[window.get_radius_slider(),window.get_amount_slider(),window.get_strength_slider(),window.get_blend_slider(),window.get_smoothing_slider()],
        "preview":target.preview,"mask_token":target.mask_token,
        "stroke":target.stroke,"handles":target.handles,
        "picking":controller.is_picking(), "stroking":controller.is_stroking(),
        "dragging":controller.dragging.get(),
    })
}

fn state(session: &Session) -> Value {
    json!({
        "map":session.document().vmf().write(&Default::default()),
        "render":session.render_document().vmf().write(&Default::default()),
        "preview":session.face_preview_token(),
        "highlight":session.terrain_triangle_highlight_json(),
        "stroke":session.terrain_stroke_json(),"handles":session.terrain_move_handles_json(),
        "history":session.history_snapshot(),
        "selection":session.selected_ids().iter().map(ToString::to_string).collect::<Vec<_>>(),
        "faces":format!("{:?}",session.selected_faces()),
        "camera":format!("{:?}",session.camera()),"view":session.view_serial(),
    })
}

fn rejected_actions(controller: &Controller, error: &str) {
    let window = &controller.window;
    let expected = fields(controller);
    for action in [
        TerrainWindow::invoke_move_handles,
        TerrainWindow::invoke_draw_stroke,
        TerrainWindow::invoke_pick_center,
        TerrainWindow::invoke_edited,
        TerrainWindow::invoke_preview_edit,
        TerrainWindow::invoke_apply,
    ] {
        action(window);
        assert_eq!(window.get_message(), error);
        assert_eq!(fields(controller), expected);
    }
    window.show().unwrap();
    window.invoke_cancel();
    assert_eq!(window.get_message(), error);
    assert!(!window.window().is_visible());
    assert_eq!(fields(controller), expected);
    window.show().unwrap();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert_eq!(window.get_message(), error);
    assert!(!window.window().is_visible());
    assert_eq!(fields(controller), expected);
}

#[test]
fn managed_terrain_actions_keep_reviews_and_live_peer_across_drain_and_owner_release() {
    i_slint_backend_testing::init_no_event_loop();
    let target = fixture();
    let peer = fixture();
    let controller = &target.controller;
    let window = &controller.window;
    let original = lock(&target.shared).document().vmf().clone();
    window.invoke_pick_center();
    assert!(controller.is_picking());
    let ray = Ray {
        origin: DVec3::new(32.0, 32.0, 128.0),
        direction: DVec3::NEG_Z,
    };
    target
        .access
        .with_session(|session| controller.select_center(session, &ray))
        .unwrap();
    assert!(!controller.is_picking());
    window.set_operation_index(1);
    window.set_radius("32".into());
    window.set_amount("4".into());
    window.invoke_edited();
    window.invoke_draw_stroke();
    assert!(controller.is_stroking(), "{}", window.get_message());
    target
        .access
        .with_session(|session| {
            controller.update_stroke(session, &ray)?;
            controller.finish_stroke(session)
        })
        .unwrap();
    assert!(window.get_ready());
    window.invoke_apply();
    assert_ne!(lock(&target.shared).document().vmf(), &original);
    target
        .access
        .with_session(|session| {
            session
                .undo(Actor::User)
                .expect("applied terrain must be undoable");
            session.select_faces(&[target.face], SelectMode::Replace)?;
            session.highlight_terrain_vertices(Actor::User, &[(target.face, vec![12])])?;
            controller.initialize(session)
        })
        .unwrap();
    window.set_operation_index(11);
    window.set_vertex_z("8".into());
    window.invoke_preview_edit();
    assert!(window.get_ready(), "{}", window.get_message());
    assert_eq!(
        lock(&target.shared)
            .terrain_grids(&[target.face], true)
            .unwrap()[0]
            .mesh
            .positions[12]
            .z,
        72.0
    );
    window.invoke_cancel();
    assert!(!lock(&target.shared).has_face_preview());
    window.invoke_move_handles();
    assert!(controller.is_handling(), "{}", window.get_message());
    window.invoke_cancel();
    assert!(!controller.is_handling());
    assert_eq!(
        lock(&target.shared).terrain_move_handles_json()["active"],
        false
    );
    target
        .access
        .with_session(|session| controller.initialize(session))
        .unwrap();
    window.set_grid_index(1);
    window.invoke_preview_edit();
    assert!(window.get_ready());
    let expected_fields = fields(controller);
    let expected_state = state(&lock(&target.shared));
    let peer_state = state(&lock(&peer.shared));
    let peer_fields = fields(&peer.controller);
    let gate = target.document.admit().unwrap();
    let draining = target.document.clone();
    let (done, completed) = mpsc::sync_channel(1);
    let drain = std::thread::spawn(move || done.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + TIMEOUT;
    while target.document.lifecycle() == DocumentLifecycle::Live
        || lock(&target.shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    rejected_actions(controller, "the editor is closing");
    assert_eq!(fields(controller), expected_fields);
    assert_eq!(state(&lock(&target.shared)), expected_state);
    assert!(completed.try_recv().is_err());
    drop(gate);
    completed.recv_timeout(TIMEOUT).unwrap().unwrap();
    drain.join().unwrap();
    assert_eq!(target.document.lifecycle(), DocumentLifecycle::Finished);
    let store = crate::recovery::Store {
        directory: target.directory.path().join("recovery"),
    };
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let recovered = store.read(&entries[0].id).unwrap().1;
    assert_eq!(
        source_formats::vmf::Vmf::parse(&recovered).unwrap(),
        original
    );
    rejected_actions(controller, "the document is closed");
    assert_eq!(fields(controller), expected_fields);
    assert_eq!(state(&lock(&target.shared)), expected_state);
    assert_eq!(store.list().unwrap().entries.len(), 1);
    assert_eq!(store.read(&entries[0].id).unwrap().1, recovered);
    assert_eq!(state(&lock(&peer.shared)), peer_state);
    assert_eq!(fields(&peer.controller), peer_fields);
    peer.controller.window.set_grid_index(1);
    peer.controller.window.invoke_preview_edit();
    peer.controller.window.invoke_apply();
    assert_eq!(
        lock(&peer.shared)
            .terrain_grids(&[peer.face], false)
            .unwrap()[0]
            .power,
        3
    );
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    let weak = Arc::downgrade(&target.shared);
    let Fixture {
        directory: _directory,
        document,
        access,
        shared,
        controller,
        face: _,
    } = target;
    controller.window.hide().unwrap();
    drop(access);
    drop(shared);
    drop(document);
    assert!(weak.upgrade().is_none());
    rejected_actions(&controller, "the document is closed");
}

#[test]
fn terrain_foreign_reviews_masks_and_stale_center_pick_remain_exact() {
    i_slint_backend_testing::init_no_event_loop();
    let target = fixture();
    let window = &target.controller.window;
    window.set_grid_index(1);
    window.invoke_preview_edit();
    let owned = lock(&target.shared).face_preview_token();
    lock(&target.shared)
        .preview_terrain_resolution(Actor::Agent, &[target.face], 4)
        .unwrap();
    let foreign = state(&lock(&target.shared));
    assert_ne!(lock(&target.shared).face_preview_token(), owned);
    window.invoke_apply();
    assert_eq!(state(&lock(&target.shared)), foreign);
    window.invoke_cancel();
    assert_eq!(state(&lock(&target.shared)), foreign);
    window.show().unwrap();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert_eq!(state(&lock(&target.shared)), foreign);
    target
        .access
        .with_session(|session| {
            session.cancel_face_preview();
            session.highlight_terrain_vertices(Actor::User, &[(target.face, vec![12])])?;
            target.controller.initialize(session)
        })
        .unwrap();
    let mask = target
        .controller
        .target
        .borrow()
        .as_ref()
        .unwrap()
        .mask_token;
    lock(&target.shared)
        .highlight_terrain_vertices(Actor::Agent, &[(target.face, vec![0])])
        .unwrap();
    assert_ne!(
        lock(&target.shared)
            .terrain_vertex_mask(&[target.face], None)
            .unwrap()
            .token,
        mask.unwrap()
    );
    let foreign = state(&lock(&target.shared));
    window.set_operation_index(11);
    window.set_vertex_z("8".into());
    window.invoke_preview_edit();
    assert!(!window.get_ready());
    window.invoke_move_handles();
    assert!(!target.controller.is_handling());
    assert_eq!(state(&lock(&target.shared)), foreign);
    window.invoke_pick_center();
    assert!(target.controller.is_picking());
    lock(&target.shared)
        .select_faces(&[], SelectMode::Replace)
        .unwrap();
    let stale = state(&lock(&target.shared));
    let before = (
        window.get_center_x(),
        window.get_center_y(),
        window.get_center_z(),
    );
    let error = target
        .access
        .with_session(|session| {
            target.controller.select_center(
                session,
                &Ray {
                    origin: DVec3::new(32.0, 32.0, 128.0),
                    direction: DVec3::NEG_Z,
                },
            )
        })
        .unwrap_err();
    assert!(error.contains("Map or face selection changed"));
    assert_eq!(
        (
            window.get_center_x(),
            window.get_center_y(),
            window.get_center_z()
        ),
        before
    );
    assert!(!target.controller.is_picking());
    window.invoke_preview_edit();
    assert_eq!(state(&lock(&target.shared)), stale);
}

#[test]
fn failed_terrain_shutdown_preserves_pending_stroke_and_memoized_recovery_error() {
    i_slint_backend_testing::init_no_event_loop();
    let target = fixture();
    let window = &target.controller.window;
    window.set_operation_index(1);
    window.invoke_draw_stroke();
    assert!(target.controller.is_stroking());
    let expected_fields = fields(&target.controller);
    let expected_state = state(&lock(&target.shared));
    let recovery = target.directory.path().join("recovery");
    if recovery.exists() {
        std::fs::remove_dir_all(&recovery).unwrap();
    }
    std::fs::write(&recovery, b"blocked").unwrap();
    let error = target.document.shutdown().unwrap_err();
    assert_eq!(
        target.document.lifecycle(),
        DocumentLifecycle::Failed(error.clone())
    );
    rejected_actions(
        &target.controller,
        &format!("document shutdown failed: {error}"),
    );
    assert_eq!(fields(&target.controller), expected_fields);
    assert_eq!(state(&lock(&target.shared)), expected_state);
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    assert_eq!(target.document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
}

#[test]
fn admitted_terrain_apply_finishes_before_final_recovery_while_shutdown_waits() {
    i_slint_backend_testing::init_no_event_loop();
    let target = fixture();
    target.controller.window.set_grid_index(1);
    target.controller.window.invoke_preview_edit();
    assert!(target.controller.window.get_ready());
    let draft = lock(&target.shared).render_document().vmf().clone();
    let draining = target.document.clone();
    let observed = target.document.clone();
    let (done, completed) = mpsc::sync_channel(1);
    let join = Rc::new(RefCell::new(None));
    let output = join.clone();
    BEFORE_ACTION.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            *output.borrow_mut() = Some(std::thread::spawn(move || {
                done.send(draining.shutdown()).unwrap()
            }));
            let deadline = Instant::now() + TIMEOUT;
            while observed.lifecycle() == DocumentLifecycle::Live {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        }))
    });
    target.controller.window.invoke_apply();
    completed.recv_timeout(TIMEOUT).unwrap().unwrap();
    join.borrow_mut().take().unwrap().join().unwrap();
    assert_eq!(lock(&target.shared).document().vmf(), &draft);
    let store = crate::recovery::Store {
        directory: target.directory.path().join("recovery"),
    };
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let recovered = store.read(&entries[0].id).unwrap().1;
    assert_eq!(source_formats::vmf::Vmf::parse(&recovered).unwrap(), draft);
}

#[test]
fn vanished_terrain_window_abandons_before_document_admission() {
    i_slint_backend_testing::init_no_event_loop();
    let target = fixture();
    let window = target.controller.window.as_weak();
    let continuation = target.access.downgrade();
    drop(target.controller);
    assert!(window.upgrade().is_none());
    let visited = Cell::new(false);
    assert_eq!(
        with_session(&window, &continuation, |_, _| visited.set(true)),
        None
    );
    assert!(!visited.get());
    assert_eq!(target.document.lifecycle(), DocumentLifecycle::Live);
}

#[test]
fn terrain_cancel_and_close_preserve_foreign_strokes_and_move_handles() {
    i_slint_backend_testing::init_no_event_loop();
    let target = fixture();
    let controller = &target.controller;
    let window = &controller.window;
    window.set_operation_index(1);
    window.invoke_draw_stroke();
    let owned = controller.target.borrow().as_ref().unwrap().stroke.unwrap();
    let foreign = target
        .access
        .with_session(|session| {
            session.cancel_terrain_stroke(owned)?;
            session.begin_terrain_stroke(Actor::Agent, &[target.face], operation(window)?, None)
        })
        .unwrap();
    assert_ne!(foreign, owned);
    let expected = state(&lock(&target.shared));
    window.invoke_cancel();
    assert_eq!(state(&lock(&target.shared)), expected);
    window.show().unwrap();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert_eq!(state(&lock(&target.shared)), expected);
    target
        .access
        .with_session(|session| {
            session.cancel_terrain_stroke(foreign)?;
            session.highlight_terrain_vertices(Actor::User, &[(target.face, vec![12])])?;
            controller.initialize(session)
        })
        .unwrap();
    window.invoke_move_handles();
    let owned = controller
        .target
        .borrow()
        .as_ref()
        .unwrap()
        .handles
        .unwrap();
    let foreign = target
        .access
        .with_session(|session| {
            session.cancel_terrain_move_handles(owned)?;
            let mask = session.terrain_vertex_mask(&[target.face], None)?.token;
            session.enable_terrain_move_handles(Actor::Agent, &[target.face], mask)
        })
        .unwrap();
    assert_ne!(foreign, owned);
    let expected = state(&lock(&target.shared));
    window.invoke_cancel();
    assert_eq!(state(&lock(&target.shared)), expected);
    window.show().unwrap();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert_eq!(state(&lock(&target.shared)), expected);
}
