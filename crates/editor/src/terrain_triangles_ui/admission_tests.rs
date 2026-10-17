use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle, lock};
use crate::session::SelectMode;
use glam::DVec3;
use map_core::{ObjectId, query::Ray, surface::FaceId};
use serde_json::{Value, json};
use slint::Model;
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

fn fields(controller: &Controller) -> Value {
    let window = &controller.window;
    let target = controller.target.borrow();
    let target = target.as_ref().unwrap();
    json!({
        "faces":window.get_faces().iter().map(|face|face.to_string()).collect::<Vec<_>>(),
        "face":window.get_face_index(),"kind":window.get_kind_index(),
        "rows":window.get_triangles().iter().map(|row|(row.id,row.label.to_string(),row.detail.to_string(),row.selected)).collect::<Vec<_>>(),
        "selection":window.get_selection_label().to_string(),
        "stale":window.get_stale(),"ready":window.get_ready(),
        "walkable":window.get_walkable_index(),"buildable":window.get_buildable_index(),
        "aspect":window.get_aspect(),"preview":target.preview,"highlight":target.highlight,
        "selected":target.selected.iter().map(|(face,ids)|(format!("{face:?}"),ids.iter().copied().collect::<Vec<_>>())).collect::<Vec<_>>(),
        "vertices":target.vertices,"picking":controller.is_picking(),
    })
}

fn rejected_actions(controller: &Controller, error: &str) {
    let window = &controller.window;
    let expected = fields(controller);
    for action in [
        TerrainTrianglesWindow::invoke_kind_changed,
        TerrainTrianglesWindow::invoke_edited,
        TerrainTrianglesWindow::invoke_face_changed,
        TerrainTrianglesWindow::invoke_pick_triangle,
        TerrainTrianglesWindow::invoke_frame_selected,
        TerrainTrianglesWindow::invoke_preview_edit,
        TerrainTrianglesWindow::invoke_select_volume,
        TerrainTrianglesWindow::invoke_use_selection,
        TerrainTrianglesWindow::invoke_apply,
    ] {
        action(window);
        assert_eq!(window.get_message(), error);
        assert_eq!(fields(controller), expected);
    }
    window.invoke_toggle_triangle(0, false);
    assert_eq!(window.get_message(), error);
    assert_eq!(fields(controller), expected);
    for selected in [false, true] {
        window.invoke_select_all(selected);
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
fn managed_triangle_actions_keep_selection_and_live_peer_through_drain_and_owner_release() {
    i_slint_backend_testing::init_no_event_loop();
    let target = fixture();
    let peer = fixture();
    let controller = &target.controller;
    let window = &controller.window;
    let original = lock(&target.shared).document().vmf().clone();
    window.invoke_select_all(true);
    assert!(window.get_triangles().iter().all(|row| row.selected));
    window.invoke_select_all(false);
    assert!(window.get_triangles().iter().all(|row| !row.selected));
    window.invoke_toggle_triangle(0, true);
    window.invoke_face_changed();
    assert!(window.get_triangles().row_data(0).unwrap().selected);
    let camera = lock(&target.shared).camera();
    window.invoke_frame_selected();
    assert_ne!(lock(&target.shared).camera(), camera);
    window.invoke_pick_triangle();
    assert!(controller.is_picking());
    let center = lock(&target.shared)
        .terrain_triangles(&[target.face], false)
        .unwrap()[0]
        .triangles[7]
        .centroid;
    assert_eq!(
        target
            .access
            .with_session(|session| controller.select_at_ray(
                session,
                &Ray {
                    origin: center + DVec3::Z * 64.0,
                    direction: DVec3::NEG_Z,
                }
            ))
            .unwrap(),
        7
    );
    assert!(window.get_triangles().row_data(7).unwrap().selected);
    window.set_walkable_index(3);
    window.invoke_edited();
    window.invoke_preview_edit();
    assert!(window.get_ready(), "{}", window.get_message());
    window.invoke_apply();
    assert!(
        !lock(&target.shared)
            .terrain_triangles(&[target.face], false)
            .unwrap()[0]
            .triangles[7]
            .walkable
    );
    target
        .access
        .with_session(|session| {
            session
                .undo(Actor::User)
                .expect("applied triangle edit must be undoable");
            session.select_faces(&[target.face], SelectMode::Replace)?;
            controller.initialize(session)
        })
        .unwrap();
    let volume = lock(&target.shared)
        .preview_volume_selection(crate::session::volume::Spec {
            mins: DVec3::new(0.0, 0.0, 63.0),
            maxs: DVec3::new(64.0, 64.0, 65.0),
            rule: map_core::region::Match::Intersecting,
            mode: SelectMode::Replace,
            faces: true,
        })
        .unwrap();
    window.invoke_select_volume();
    assert!(window.get_triangles().iter().all(|row| row.selected));
    assert_eq!(lock(&target.shared).volume_preview().unwrap().token, volume);
    window.invoke_use_selection();
    assert!(
        lock(&target.shared)
            .terrain_vertex_mask(&[target.face], None)
            .is_ok()
    );
    window.invoke_cancel();
    assert!(
        lock(&target.shared)
            .terrain_vertex_mask(&[target.face], None)
            .is_ok()
    );
    target
        .access
        .with_session(|session| controller.initialize(session))
        .unwrap();
    window.set_kind_index(1);
    window.invoke_kind_changed();
    assert_eq!(window.get_triangles().row_count(), 25);
    window.invoke_toggle_triangle(12, true);
    window.invoke_frame_selected();
    window.invoke_use_selection();
    assert_eq!(
        lock(&target.shared)
            .terrain_vertex_mask(&[target.face], None)
            .unwrap()
            .faces[0]
            .vertices,
        [12]
    );
    target
        .access
        .with_session(|session| controller.initialize(session))
        .unwrap();
    window.invoke_toggle_triangle(0, true);
    window.set_buildable_index(3);
    window.invoke_preview_edit();
    assert!(window.get_ready(), "{}", window.get_message());
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
    peer.controller.window.invoke_toggle_triangle(0, true);
    peer.controller.window.set_walkable_index(3);
    peer.controller.window.invoke_preview_edit();
    peer.controller.window.invoke_apply();
    assert!(
        !lock(&peer.shared)
            .terrain_triangles(&[peer.face], false)
            .unwrap()[0]
            .triangles[0]
            .walkable
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
fn triangle_foreign_reviews_highlights_and_resolution_picks_preserve_exact_ownership() {
    i_slint_backend_testing::init_no_event_loop();
    let target = fixture();
    let window = &target.controller.window;
    window.invoke_toggle_triangle(0, true);
    window.set_walkable_index(3);
    window.invoke_preview_edit();
    let own = lock(&target.shared).face_preview_token();
    lock(&target.shared)
        .preview_terrain_resolution(Actor::Agent, &[target.face], 3)
        .unwrap();
    lock(&target.shared)
        .highlight_terrain_vertices(Actor::Agent, &[(target.face, vec![12])])
        .unwrap();
    assert_ne!(lock(&target.shared).face_preview_token(), own);
    let foreign = state(&lock(&target.shared));
    window.invoke_apply();
    assert_eq!(state(&lock(&target.shared)), foreign);
    window.invoke_pick_triangle();
    assert!(target.controller.is_picking());
    let before = fields(&target.controller);
    let center = lock(&target.shared)
        .terrain_triangles(&[target.face], true)
        .unwrap()[0]
        .triangles[0]
        .centroid;
    let error = target
        .access
        .with_session(|session| {
            target.controller.select_at_ray(
                session,
                &Ray {
                    origin: center + DVec3::Z * 64.0,
                    direction: DVec3::NEG_Z,
                },
            )
        })
        .unwrap_err();
    assert!(error.contains("grid-resolution preview"));
    assert_eq!(fields(&target.controller), before);
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
            target.controller.initialize(session)
        })
        .unwrap();
    window.invoke_pick_triangle();
    lock(&target.shared)
        .select_faces(&[], SelectMode::Replace)
        .unwrap();
    let stale = state(&lock(&target.shared));
    let error = target
        .access
        .with_session(|session| {
            target.controller.select_at_ray(
                session,
                &Ray {
                    origin: DVec3::new(32.0, 32.0, 128.0),
                    direction: DVec3::NEG_Z,
                },
            )
        })
        .unwrap_err();
    assert!(error.contains("Map or face selection changed"));
    window.invoke_toggle_triangle(1, true);
    window.invoke_preview_edit();
    assert_eq!(state(&lock(&target.shared)), stale);
}

#[test]
fn failed_triangle_shutdown_keeps_pending_pick_and_memoized_recovery_error() {
    i_slint_backend_testing::init_no_event_loop();
    let target = fixture();
    target.controller.window.invoke_toggle_triangle(0, true);
    target.controller.window.invoke_pick_triangle();
    assert!(target.controller.is_picking());
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
fn admitted_triangle_apply_finishes_before_final_recovery_while_shutdown_waits() {
    i_slint_backend_testing::init_no_event_loop();
    let target = fixture();
    target.controller.window.invoke_toggle_triangle(0, true);
    target.controller.window.set_walkable_index(3);
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
fn vanished_triangle_window_abandons_before_document_admission() {
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
