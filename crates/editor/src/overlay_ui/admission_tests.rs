use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use crate::session::SelectMode;
use map_core::ObjectId;
use serde_json::json;
use slint::Model;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);
const VIEWPORT: (u32, u32) = (900, 700);

struct Fixture {
    document: DocumentHandle,
    access: DocumentAccess,
    shared: SharedSession,
    controller: Controller,
    source: FaceId,
    directory: tempfile::TempDir,
    _assets: tempfile::TempDir,
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

fn fixture(handles: bool) -> Fixture {
    let (assets, game) = crate::test_assets::game();
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Ok(game));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::new(192.0, 128.0, 16.0),
            "test/checker",
            false,
        )
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
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
        .unwrap()
        .id;
    let source = FaceId { solid, side };
    session
        .select_faces(&[source], SelectMode::Replace)
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    controller.show(&mut lock(&shared), "test/checker").unwrap();
    controller.window.invoke_fit();
    assert!(
        controller.window.get_ready(),
        "{}",
        controller.window.get_message()
    );
    let fixture = Fixture {
        document,
        access,
        shared,
        controller,
        source,
        directory,
        _assets: assets,
    };
    if handles {
        fixture.controller.window.invoke_apply();
        corners(&fixture);
        fixture.controller.window.invoke_handle_corners();
        assert!(lock(&fixture.shared).overlay_corner_handles_active());
        assert!(!fixture.controller.window.window().is_visible());
    }
    fixture
}

fn corners(target: &Fixture) {
    target
        .controller
        .show(&mut lock(&target.shared), "test/checker")
        .unwrap();
    target.controller.window.invoke_edit_corners();
    assert!(target.controller.window.get_corners_mode());
    target.controller.window.invoke_preview_corners();
    assert!(
        target.controller.window.get_ready(),
        "{}",
        target.controller.window.get_message()
    );
    let state = lock(&target.shared).get_overlay_state();
    let origin =
        serde_json::from_value::<[f64; 3]>(state["draft"]["basis_origin"].clone()).unwrap();
    lock(&target.shared).set_camera(render::Camera::planar(
        render::Projection::Top,
        DVec3::from_array(origin).as_vec3(),
        200.0,
    ));
    target.controller.set_viewport(VIEWPORT);
}

fn files(directory: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(files(&path));
        } else {
            result.insert(path.clone(), std::fs::read(path).unwrap());
        }
    }
    result
}

fn snapshot(target: &Fixture) -> (source_formats::vmf::Vmf, Value, BTreeMap<PathBuf, Vec<u8>>) {
    let session = lock(&target.shared);
    (
        session.document().vmf().clone(),
        json!({
            "overlay":session.get_overlay_state(),"handles":session.overlay_corner_handle_state(),
            "history":session.history_snapshot(),"material_generation":session.materials().generation(),
            "faces":session.selected_faces().into_iter().map(|face|(face.solid,face.side)).collect::<Vec<_>>(),
            "objects":session.selected_ids().into_iter().map(|id|id.to_string()).collect::<Vec<_>>(),
            "camera":format!("{:?}",session.camera()),
            "serials":[session.scene_serial(),session.view_serial(),session.log_serial(),session.camera_serial()],
        }),
        files(target.directory.path()),
    )
}

fn forms(controller: &Controller) -> Value {
    let form = controller.form.borrow();
    let window = &controller.window;
    json!({
        "form": {
            "source":form.source.map(|face|(face.solid,face.side)),"token":form.token,"reviewed":format!("{:?}",form.reviewed),
            "refit":form.refit,"rebind":form.rebind,"corners":form.corners,"reviewed_corners":form.reviewed_corners,
            "target":form.target,"choices":form.choices,"choices_snapshot":form.choices_snapshot,"offset":form.choices_offset,
            "snapshot":form.snapshot,"seen":form.seen,"serial":form.serial,"generation":form.material_generation,
        },
        "fields":[window.get_material().to_string(),window.get_targetname().to_string(),window.get_u_start().to_string(),window.get_u_end().to_string(),window.get_v_start().to_string(),window.get_v_end().to_string(),window.get_fade_min().to_string(),window.get_fade_max().to_string()],
        "render_order":window.get_render_order(),"corners":corner_fields(window).ok(),
        "search":window.get_overlay_search().to_string(),"ready":window.get_ready(),"outline":window.get_outline().to_string(),
        "source":window.get_source_label().to_string(),"rebind_target":window.get_rebind_target().to_string(),
        "detail":window.get_material_detail().to_string(),"image":[window.get_material_image().size().width,window.get_material_image().size().height],
        "rows":window.get_overlay_targets().iter().map(|row|(row.entity.to_string(),row.label.to_string(),row.detail.to_string(),row.eligible)).collect::<Vec<_>>(),
        "viewport":controller.viewport.get(),
    })
}

fn retained_actions(target: &Fixture, expected: &str) {
    let window = &target.controller.window;
    window.set_overlay_search("pending rebind search".into());
    let before = snapshot(target);
    let form = forms(&target.controller);
    for invoke in [
        OverlayWindow::invoke_edited,
        OverlayWindow::invoke_preview,
        OverlayWindow::invoke_search_overlays,
        OverlayWindow::invoke_rebind,
        OverlayWindow::invoke_edit_corners,
        OverlayWindow::invoke_preview_corners,
        OverlayWindow::invoke_handle_corners,
        OverlayWindow::invoke_corners_edited,
        OverlayWindow::invoke_refit,
        OverlayWindow::invoke_fit,
        OverlayWindow::invoke_review,
        OverlayWindow::invoke_apply,
        OverlayWindow::invoke_cancel,
    ] {
        window.show().unwrap();
        invoke(window);
        assert_eq!(window.get_message(), expected);
        assert_eq!(snapshot(target), before);
        assert_eq!(forms(&target.controller), form);
    }
    assert!(!window.window().is_visible());
    window.invoke_overlay_page_change(1);
    assert_eq!(window.get_message(), expected);
    window.invoke_choose_overlay("entity:1".into());
    assert_eq!(window.get_message(), expected);
    assert_eq!(snapshot(target), before);
    assert_eq!(forms(&target.controller), form);
    window.show().unwrap();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!window.window().is_visible());
    assert_eq!(window.get_message(), expected);
    assert_eq!(snapshot(target), before);
    assert_eq!(forms(&target.controller), form);
}

fn drag(target: &Fixture) {
    let state = lock(&target.shared).get_overlay_state();
    let start = Pointer {
        position: DVec2::new(
            state["draft"]["handles"]["pixels"][0][0].as_f64().unwrap(),
            state["draft"]["handles"]["pixels"][0][1].as_f64().unwrap(),
        ),
        size: VIEWPORT,
    };
    target
        .access
        .with_session(|session| {
            target.controller.pick_handle(session, start)?;
            target.controller.move_handle(
                session,
                Pointer {
                    position: start.position + DVec2::new(-12.0, 8.0),
                    ..start
                },
            )
        })
        .unwrap();
}

#[test]
fn managed_overlay_actions_create_refit_rebind_and_move_exact_corner_handles() {
    software();
    let target = fixture(false);
    let original = lock(&target.shared).document().vmf().clone();
    target.controller.window.invoke_apply();
    assert_ne!(lock(&target.shared).document().vmf(), &original);
    let selected = lock(&target.shared).selected_ids();
    target
        .controller
        .show(&mut lock(&target.shared), "test/checker")
        .unwrap();
    target.controller.window.invoke_refit();
    assert!(target.controller.window.get_ready());
    target.controller.window.invoke_cancel();
    corners(&target);
    let before = lock(&target.shared).get_overlay_state()["draft"]["local_corners"].clone();
    target.controller.window.invoke_handle_corners();
    drag(&target);
    target
        .access
        .with_session(|session| target.controller.cancel_handles(session))
        .unwrap();
    assert_eq!(
        lock(&target.shared).get_overlay_state()["draft"]["local_corners"],
        before
    );
    target.controller.window.invoke_handle_corners();
    drag(&target);
    target
        .access
        .with_session(|session| target.controller.finish_handles(session))
        .unwrap();
    assert_ne!(
        lock(&target.shared).get_overlay_state()["draft"]["local_corners"],
        before
    );
    assert!(target.controller.window.get_ready());
    target.controller.window.invoke_apply();
    assert_eq!(lock(&target.shared).selected_ids(), selected);
    let destination = {
        let session = lock(&target.shared);
        let side = session
            .document()
            .solid(target.source.solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| side.id != target.source.side)
            .unwrap()
            .id;
        FaceId {
            solid: target.source.solid,
            side,
        }
    };
    lock(&target.shared)
        .select_faces(&[destination], SelectMode::Replace)
        .unwrap();
    target
        .controller
        .show(&mut lock(&target.shared), "test/checker")
        .unwrap();
    target.controller.window.invoke_search_overlays();
    let entity = crate::identity::id_string(selected[0]);
    assert!(
        target
            .controller
            .window
            .get_overlay_targets()
            .iter()
            .any(|row| row.entity.as_str() == entity)
    );
    target
        .controller
        .window
        .invoke_choose_overlay(entity.into());
    assert!(target.controller.window.get_rebind_mode());
    target.controller.window.invoke_rebind();
    assert!(
        target.controller.window.get_ready(),
        "{}",
        target.controller.window.get_message()
    );
    target.controller.window.invoke_apply();
    assert_eq!(lock(&target.shared).selected_ids(), selected);
    assert_eq!(
        lock(&target.shared).get_overlay_state()["selected_overlay"]["source"]["side"],
        destination.side
    );
}

#[test]
fn managed_overlay_actions_preserve_exact_material_choices_stale_forms_and_foreign_preview_ownership()
 {
    software();
    let target = fixture(false);
    let material = target.controller.material_target(&lock(&target.shared));
    target
        .controller
        .choose_material(&mut lock(&target.shared), &material, "test/checker")
        .unwrap();
    let before = snapshot(&target);
    assert!(
        target
            .controller
            .choose_material(&mut lock(&target.shared), &material, "test/checker")
            .is_err()
    );
    assert_eq!(snapshot(&target), before);
    target.controller.window.invoke_preview();
    target.controller.window.set_u_start("2".into());
    let before = snapshot(&target);
    target.controller.window.invoke_apply();
    assert_eq!(snapshot(&target), before);
    target.controller.window.invoke_review();
    assert!(target.controller.window.get_ready());
    let foreign = {
        let mut session = lock(&target.shared);
        let stamp = session.overlay_stamp();
        session
            .preview_overlay(
                Actor::Agent,
                Request {
                    source: target.source,
                    options: Options {
                        material: "test/checker".into(),
                        ..Default::default()
                    },
                },
                &stamp,
            )
            .unwrap()
    };
    target.controller.window.invoke_apply();
    target.controller.window.invoke_cancel();
    assert_eq!(
        lock(&target.shared).get_overlay_state()["draft"]["token"],
        foreign["draft"]["token"]
    );
    target
        .controller
        .show(&mut lock(&target.shared), "test/checker")
        .unwrap();
    target.controller.window.invoke_review();
    assert!(target.controller.window.get_ready());
    target.controller.window.invoke_cancel();
    assert!(lock(&target.shared).get_overlay_state()["draft"].is_null());
    target
        .controller
        .show(&mut lock(&target.shared), "test/checker")
        .unwrap();
    lock(&target.shared)
        .select_faces(&[], SelectMode::Replace)
        .unwrap();
    let before = snapshot(&target);
    target.controller.window.invoke_preview();
    assert_eq!(snapshot(&target), before);
    assert!(!target.controller.window.get_ready());
}

#[test]
fn overlay_forms_and_handle_ownership_survive_real_drain_finish_and_live_peer_activity() {
    software();
    for handles in [false, true] {
        let target = fixture(handles);
        let peer = fixture(false);
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
        let peer_before = snapshot(&peer);
        retained_actions(&target, "the editor is closing");
        assert_eq!(snapshot(&peer), peer_before);
        peer.controller.window.invoke_apply();
        assert_ne!(snapshot(&peer).0, peer_before.0);
        assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
        drop(gate);
        receive.recv_timeout(TIMEOUT).unwrap().unwrap();
        thread.join().unwrap();
        assert_eq!(target.document.lifecycle(), DocumentLifecycle::Finished);
        let entries = lock(&target.shared).recovery_entries().unwrap().entries;
        assert_eq!(entries.len(), 1);
        let store = crate::recovery::Store {
            directory: target.directory.path().join("recovery"),
        };
        let recovered = store.read(&entries[0].id).unwrap().1;
        assert_eq!(
            &source_formats::vmf::Vmf::parse(&recovered).unwrap(),
            lock(&target.shared).document().vmf()
        );
        retained_actions(&target, "the document is closed");
        assert_eq!(store.read(&entries[0].id).unwrap().1, recovered);
        peer.document.shutdown().unwrap();
    }
}

#[test]
fn retained_overlay_window_releases_runtime_session_and_vanished_origin_abandons_work() {
    software();
    let target = fixture(false);
    let cold = Controller::new_with_access(&target.access).unwrap();
    let vanished = cold.window.as_weak();
    drop(cold);
    assert!(vanished.upgrade().is_none());
    callback(&vanished, &target.access.downgrade(), |_, _| {
        panic!("vanished Overlay window accessed Session")
    });
    let window = target.controller.window.clone_strong();
    window.hide().unwrap();
    let weak = Arc::downgrade(&target.shared);
    let continuation = target.access.downgrade();
    drop(target);
    assert!(weak.upgrade().is_none());
    assert!(continuation.upgrade().is_err());
    window.invoke_edited();
    window.invoke_handle_corners();
    window.invoke_review();
    window.invoke_apply();
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_cancel();
    assert!(!window.window().is_visible());
}
