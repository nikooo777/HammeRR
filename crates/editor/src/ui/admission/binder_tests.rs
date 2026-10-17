use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle, SharedSession, lock};
use crate::session::{Actor, Session, working_region::Action};
use crate::ui::{clipping, manipulation, transform_pivot};
use glam::DVec3;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

type ActionCallback = (&'static str, fn(&AppWindow));

const ACTIONS: [ActionCallback; 22] = [
    ("clip handles", AppWindow::invoke_clip_handles_changed),
    (
        "cancel clip positioning",
        AppWindow::invoke_cancel_clip_positioning,
    ),
    ("clip points", |app| app.invoke_clip_points_action(3)),
    ("start clip", AppWindow::invoke_start_clip),
    ("preview clip", AppWindow::invoke_preview_clip_plane),
    ("clip settings", AppWindow::invoke_clip_settings_changed),
    ("clip inputs", AppWindow::invoke_clip_inputs_changed),
    ("apply clip", AppWindow::invoke_apply_clip),
    ("cancel clip", AppWindow::invoke_cancel_clip),
    ("snap preview", AppWindow::invoke_preview_snap_to_grid),
    ("transform tool", AppWindow::invoke_transform_tool_changed),
    ("paint settings", AppWindow::invoke_paint_settings_changed),
    ("area settings", AppWindow::invoke_area_settings_changed),
    ("apply transform", AppWindow::invoke_apply_transform_preview),
    ("apply area", AppWindow::invoke_apply_area_selection),
    ("cancel interaction", AppWindow::invoke_cancel_interaction),
    ("create terrain", |app| app.invoke_create_terrain(2, 0)),
    ("remove terrain", AppWindow::invoke_remove_terrain),
    ("pick pivot", AppWindow::invoke_pick_transform_pivot),
    (
        "cancel pivot",
        AppWindow::invoke_cancel_transform_pivot_pick,
    ),
    ("fly speed", AppWindow::invoke_fly_speed_changed),
    ("clear region", AppWindow::invoke_clear_working_region),
];

fn fixture() -> (
    tempfile::TempDir,
    DocumentHandle,
    DocumentAccess,
    SharedSession,
    AppWindow,
) {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(64.0), "test", false)
        .unwrap();
    session.set_camera(render::Camera::planar(
        render::Projection::Top,
        glam::Vec3::splat(32.0),
        256.0,
    ));
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let app = AppWindow::new().unwrap();
    (directory, document, access, shared, app)
}

fn bind(app: &AppWindow, access: &DocumentAccess) {
    clipping::bind_clip_controls_with_access(app, access);
    manipulation::bind_manipulation_controls_with_access(app, access);
    manipulation::bind_terrain_controls_with_access(app, access);
    transform_pivot::bind_pivot_pick_controls_with_access(app, access);
    crate::navigation_defaults_ui::bind_speed_with_access(app, access);
    crate::working_region_ui::bind_clear_with_access(app, access);
}

fn snapshot(shared: &SharedSession) -> (source_formats::vmf::Vmf, source_formats::vmf::Vmf, Value) {
    let session = lock(shared);
    (
        session.document().vmf().clone(),
        session.render_document().vmf().clone(),
        json!({
            "history": session.history_snapshot(),
            "clip": session.clip_point_stamp(),
            "clip_tool": session.clip_tool(),
            "clip_preview": session.clip_preview().is_some(),
            "pivot": session.transform_pivot_pick_stamp(),
            "region": session.working_region_stamp(),
            "region_enabled": session.working_region_enabled(),
            "interaction": session.interaction_epoch(),
            "transform": format!("{:?}", session.transform_tool()),
            "paint_enabled": session.paint_enabled(),
            "paint_radius": session.paint_radius(),
            "paint_preview": session.paint_stroke().is_some(),
            "area_rule": format!("{:?}", session.area_rule()),
            "area_mode": format!("{:?}", session.area_mode()),
            "fly_speed": session.fly_speed(),
            "camera": format!("{:?}", session.camera()),
        }),
    )
}

#[test]
fn all_six_managed_binders_keep_live_tool_operations_on_the_fixed_document() {
    i_slint_backend_testing::init_no_event_loop();
    let (_directory, document, access, shared, app) = fixture();
    let (_peer_directory, peer, _peer_access, peer_shared, _peer_app) = fixture();
    let peer_before = snapshot(&peer_shared);
    bind(&app, &access);
    app.set_fly_speed("987.625".into());
    app.invoke_fly_speed_changed();
    assert_eq!(lock(&shared).fly_speed(), 987.625);
    app.invoke_start_clip();
    assert!(lock(&shared).clip_tool());
    app.set_clip_normal_x("1".into());
    app.set_clip_normal_y("0".into());
    app.set_clip_normal_z("0".into());
    app.set_clip_distance("32".into());
    app.invoke_preview_clip_plane();
    assert!(lock(&shared).clip_preview().is_some());
    app.invoke_cancel_clip();
    assert!(!lock(&shared).clip_tool());
    assert!(lock(&shared).clip_preview().is_none());
    app.set_handles_mode_index(2);
    app.set_rotation_step("22.5".into());
    app.invoke_transform_tool_changed();
    assert_eq!(lock(&shared).transform_tool().angle_step, 22.5);
    app.invoke_pick_transform_pivot();
    assert!(lock(&shared).transform_pivot_picking());
    app.invoke_cancel_transform_pivot_pick();
    assert!(!lock(&shared).transform_pivot_picking());
    let face = {
        let mut session = lock(&shared);
        let solid = &session.document().vmf().world.solids[0];
        let side = solid
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap();
        let face = map_core::surface::FaceId {
            solid: solid.id,
            side: side.id,
        };
        session
            .select_faces(&[face], crate::session::SelectMode::Replace)
            .unwrap();
        face
    };
    app.invoke_create_terrain(2, 0);
    assert!(
        lock(&shared)
            .document()
            .solid(face.solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| side.id == face.side)
            .unwrap()
            .displacement
            .is_some()
    );
    app.invoke_remove_terrain();
    assert!(
        lock(&shared)
            .document()
            .solid(face.solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| side.id == face.side)
            .unwrap()
            .displacement
            .is_none()
    );
    {
        let mut session = lock(&shared);
        let stamp = session.working_region_stamp();
        session
            .set_working_region(
                Actor::User,
                Action::Set {
                    bounds: [DVec3::splat(-8.0), DVec3::splat(96.0)],
                    enabled: true,
                },
                &stamp,
            )
            .unwrap();
    }
    app.invoke_clear_working_region();
    assert!(!lock(&shared).working_region_enabled());
    assert_eq!(snapshot(&peer_shared), peer_before);
    assert_eq!(peer.lifecycle(), DocumentLifecycle::Live);
    document.shutdown().unwrap();
    peer.shutdown().unwrap();
}

#[test]
fn every_real_binder_callback_rejects_finished_and_failed_documents_while_peer_stays_live() {
    i_slint_backend_testing::init_no_event_loop();
    for failed in [false, true] {
        let (directory, document, access, shared, app) = fixture();
        let (_peer_directory, peer, peer_access, peer_shared, peer_app) = fixture();
        bind(&app, &access);
        bind(&peer_app, &peer_access);
        app.invoke_start_clip();
        app.set_clip_normal_x("1".into());
        app.set_clip_normal_y("0".into());
        app.set_clip_normal_z("0".into());
        app.set_clip_distance("32".into());
        app.invoke_preview_clip_plane();
        assert!(lock(&shared).clip_preview().is_some());
        let recovery = directory.path().join("recovery");
        if failed {
            std::fs::write(&recovery, b"not a directory").unwrap();
        }
        let outcome = document.shutdown();
        let expected = match &outcome {
            Ok(()) => "the document is closed".to_owned(),
            Err(error) => format!("document shutdown failed: {error}"),
        };
        assert_eq!(outcome.is_err(), failed);
        let before = snapshot(&shared);
        let peer_before = snapshot(&peer_shared);
        for (name, invoke) in ACTIONS {
            app.set_message("waiting".into());
            invoke(&app);
            assert_eq!(app.get_message(), expected.as_str(), "{name}");
            assert_eq!(snapshot(&shared), before, "{name}");
            assert_eq!(snapshot(&peer_shared), peer_before, "{name}");
        }
        peer_app.set_fly_speed("1200".into());
        peer_app.invoke_fly_speed_changed();
        assert_eq!(lock(&peer_shared).fly_speed(), 1200.0);
        assert_eq!(snapshot(&shared), before);
        assert_eq!(peer.lifecycle(), DocumentLifecycle::Live);
        if failed {
            std::fs::remove_file(&recovery).unwrap();
            std::fs::create_dir(&recovery).unwrap();
            assert_eq!(document.shutdown(), outcome);
            assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
        }
        peer.shutdown().unwrap();
    }
}

#[test]
fn retained_real_binders_do_not_keep_the_document_or_session_owner_alive() {
    i_slint_backend_testing::init_no_event_loop();
    let (directory, document, access, shared, app) = fixture();
    let owners = Arc::strong_count(&shared);
    bind(&app, &access);
    assert_eq!(Arc::strong_count(&shared), owners);
    drop(access);
    drop(document);
    let store = crate::recovery::Store {
        directory: directory.path().join("recovery"),
    };
    assert_eq!(store.list().unwrap().entries.len(), 1);
    let before = snapshot(&shared);
    for (name, invoke) in ACTIONS {
        invoke(&app);
        assert_eq!(snapshot(&shared), before, "{name}");
    }
    let weak = Arc::downgrade(&shared);
    drop(shared);
    assert!(weak.upgrade().is_none());
    for (_, invoke) in ACTIONS {
        invoke(&app);
    }
}

#[test]
fn session_callback_abandons_a_missing_window_before_touching_a_live_document() {
    i_slint_backend_testing::init_no_event_loop();
    let (_directory, document, access, shared, app) = fixture();
    bind(&app, &access);
    let callback = SessionCallback::new(&app, &access);
    let before = snapshot(&shared);
    let weak = app.as_weak();
    drop(app);
    assert!(weak.upgrade().is_none());
    callback.input(|_, _| panic!("missing originating window executed callback"));
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    assert_eq!(snapshot(&shared), before);
    document.shutdown().unwrap();
}

#[test]
fn session_callback_keeps_the_complete_locked_body_admitted_until_final_capture() {
    i_slint_backend_testing::init_no_event_loop();
    let (directory, document, access, shared, app) = fixture();
    let (entered, start) = mpsc::channel();
    let (finished, done) = mpsc::channel();
    let draining = document.clone();
    let shutdown = std::thread::spawn(move || {
        start.recv_timeout(TIMEOUT).unwrap();
        finished.send(draining.shutdown()).unwrap();
    });
    app.on_start_clip({
        let callback = SessionCallback::new(&app, &access);
        let observed = document.clone();
        move || {
            callback.input(|_, session| {
                entered.send(()).unwrap();
                let deadline = Instant::now() + TIMEOUT;
                while observed.lifecycle() == DocumentLifecycle::Live {
                    assert!(
                        Instant::now() < deadline,
                        "shutdown did not close admission"
                    );
                    std::thread::yield_now();
                }
                assert!(done.try_recv().is_err());
                session
                    .create_block(
                        Actor::User,
                        DVec3::splat(96.0),
                        DVec3::splat(112.0),
                        "test",
                        false,
                    )
                    .unwrap();
            });
        }
    });
    app.invoke_start_clip();
    shutdown.join().unwrap();
    assert_eq!(document.lifecycle(), DocumentLifecycle::Finished);
    let store = crate::recovery::Store {
        directory: directory.path().join("recovery"),
    };
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let (_, text) = store.read(&entries[0].id).unwrap();
    let recovered = source_formats::vmf::Vmf::parse(&text).unwrap();
    assert_eq!(recovered.world.solids.len(), 2);
    assert_eq!(&recovered, lock(&shared).document().vmf());
}
