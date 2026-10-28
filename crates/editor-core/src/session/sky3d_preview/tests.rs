use super::*;
use crate::session::{SelectMode, ViewUpdate, editor_visibility, working_region};
use map_core::{Document, edit};
use source_formats::keyvalues::Node;

fn fixture() -> (tempfile::TempDir, Session, u32, ObjectId, ObjectId) {
    let (directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let mut document = Document::empty();
    let (camera, wall, detail) = document
        .edit("sky fixture", |tx| {
            let mut walls = Vec::new();
            for (mins, maxs) in [
                ([-144.0, -144.0, -16.0], [144.0, 144.0, 0.0]),
                ([-144.0, -144.0, 128.0], [144.0, 144.0, 144.0]),
                ([-144.0, -144.0, 0.0], [-128.0, 144.0, 128.0]),
                ([128.0, -144.0, 0.0], [144.0, 144.0, 128.0]),
                ([-128.0, -144.0, 0.0], [128.0, -128.0, 128.0]),
                ([-128.0, 128.0, 0.0], [128.0, 144.0, 128.0]),
            ] {
                walls.push(edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "test/checker",
                )?);
            }
            let ObjectId::Solid(plate) = edit::create_block(
                tx,
                DVec3::new(32.0, -24.0, 0.0),
                DVec3::new(48.0, 24.0, 32.0),
                "test/checker",
            )?
            else {
                unreachable!()
            };
            let detail = edit::tie_to_entity(tx, &[plate], "func_detail", &[])?;
            let ObjectId::Entity(camera) = edit::create_entity(
                tx,
                "sky_camera",
                Some(DVec3::new(0.0, 0.0, 64.0)),
                &[
                    ("scale".into(), "16".into()),
                    ("angles".into(), "17 39 21".into()),
                ],
                Vec::new(),
            )?
            else {
                unreachable!()
            };
            Ok((camera, walls[0], detail))
        })
        .unwrap();
    session.replace_document(document, None);
    (directory, session, camera, wall, detail)
}

fn configure(session: &mut Session, camera: u32) {
    session.set_sky3d_preview(false, Some(camera)).unwrap();
}

fn prepared(request: &Request) -> Result<Ready, String> {
    let scene = sky3d::Scene::build(
        sky3d::Camera {
            origin: DVec3::ZERO,
            scale: 16.0,
        },
        &[sky3d::Surface {
            points: vec![DVec3::ZERO, DVec3::X, DVec3::Y],
            normal: DVec3::Z,
            material: "test/checker".into(),
            texture_u: [1.0, 0.0, 0.0, 0.0],
            texture_v: [0.0, 1.0, 0.0, 0.0],
        }],
        &request.game,
    )?;
    Ok(Ready {
        key: request.key.clone(),
        stats: json!({"geometry":scene.stats}),
        scene: Arc::new(scene),
        snapshot_bytes: request.snapshot_bytes,
    })
}

fn finish_synthetic(session: &mut Session) -> Value {
    let request = session.begin_sky3d_preview(Actor::User).unwrap();
    session
        .finish_sky3d_preview(Actor::User, request.run_with(prepared))
        .unwrap()
}

#[test]
fn explicit_camera_and_async_preparation_preserve_source_history_selection_and_camera() {
    let (directory, mut session, camera, _, detail) = fixture();
    let path = directory.path().join("sky.vmf");
    session.save(Actor::User, Some(&path)).unwrap();
    session.select(&[detail], SelectMode::Replace).unwrap();
    let source = session.document.vmf().clone();
    let history = session.history_snapshot();
    let selection = session.selected_ids();
    let view_camera = session.camera();
    let disk = std::fs::read(&path).unwrap();
    assert!(!session.view().sky3d_preview);
    assert!(!session.view().sky_preview);
    assert_eq!(session.sky3d_preview_state()["camera_entity"], Value::Null);
    assert!(session.begin_sky3d_preview(Actor::User).is_err());
    configure(&mut session, camera);
    let request = session.begin_sky3d_preview(Actor::User).unwrap();
    assert_eq!(session.sky3d_preview_state()["status"], "loading");
    assert!(session.build_scene().sky3d.is_none());
    let report = session
        .finish_sky3d_preview(Actor::User, request.run())
        .unwrap();
    assert_eq!(report["status"], "ready", "{report}");
    assert_eq!(report["ready"], true);
    assert_eq!(report["enabled"], false);
    assert_eq!(report["worker_active"], false);
    assert!(report["stats"]["geometry"]["vertices"].as_u64().unwrap() > 0);
    assert!(session.build_scene().sky3d.is_some());
    let view = session
        .resolve_view_update(ViewUpdate {
            sky3d_preview: Some(true),
            ..Default::default()
        })
        .unwrap();
    assert!(!session.view().sky3d_preview);
    assert!(session.build_scene_for_view(view).sky3d.is_some());
    session.set_view(view);
    assert_eq!(session.sky3d_preview_state()["enabled"], true);
    assert!(!session.view().sky_preview);
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.selected_ids(), selection);
    assert_eq!(session.camera(), view_camera);
    assert_eq!(std::fs::read(path).unwrap(), disk);
}

#[test]
fn invalid_camera_settings_are_atomic_and_changed_or_removed_camera_never_reroutes() {
    let (_directory, mut session, camera, _, _) = fixture();
    let ObjectId::Entity(other) = session
        .edit(Actor::User, "other sky camera", |tx| {
            edit::create_entity(
                tx,
                "sky_camera",
                Some(DVec3::new(16.0, 0.0, 64.0)),
                &[("scale".into(), "16".into())],
                Vec::new(),
            )
        })
        .unwrap()
    else {
        unreachable!()
    };
    configure(&mut session, camera);
    finish_synthetic(&mut session);
    assert!(session.set_sky3d_preview(true, None).is_err());
    assert!(session.set_sky3d_preview(true, Some(u32::MAX)).is_err());
    assert_eq!(session.sky3d_preview.camera, Some(camera));
    assert!(!session.view().sky3d_preview);
    assert!(session.sky3d_preview.ready.is_some());
    session
        .set_entity_properties(Actor::User, camera, &[("scale".into(), Some("NaN".into()))])
        .unwrap();
    let changed = session.sky3d_preview_state();
    assert_eq!(changed["status"], "stale");
    assert_eq!(changed["ready"], false);
    assert_eq!(changed["camera_entity"], format!("entity:{camera}"));
    assert!(changed["issue"].as_str().unwrap().contains("integer"));
    assert!(session.build_scene().sky3d.is_none());
    session.undo(Actor::User).unwrap();
    assert_eq!(session.sky3d_preview_state()["ready"], false);
    finish_synthetic(&mut session);
    session
        .edit(Actor::User, "remove selected sky camera", |tx| {
            edit::delete(tx, &[ObjectId::Entity(camera)])
        })
        .unwrap();
    let removed = session.sky3d_preview_state();
    assert_eq!(removed["camera_entity"], format!("entity:{camera}"));
    assert!(removed["issue"].as_str().unwrap().contains("missing"));
    assert_eq!(removed["cameras"][0]["id"], format!("entity:{other}"));
    assert!(session.begin_sky3d_preview(Actor::Agent).is_err());
}

#[test]
fn completion_requires_exact_owner_and_keeps_worker_slot_until_result_retires() {
    let (_directory, mut origin, camera, _, _) = fixture();
    let (_peer_directory, mut peer, peer_camera, _, _) = fixture();
    configure(&mut origin, camera);
    configure(&mut peer, peer_camera);
    let completed = origin
        .begin_sky3d_preview(Actor::User)
        .unwrap()
        .run_with(prepared);
    let peer_request = peer.begin_sky3d_preview(Actor::Agent).unwrap();
    assert_eq!(completed.token, peer_request.token());
    assert!(origin.instance_workers_busy());
    assert!(origin.begin_sky3d_preview(Actor::User).is_err());
    let peer_token = peer.sky3d_preview.serial;
    assert!(peer.finish_sky3d_preview(Actor::User, completed).is_err());
    assert_eq!(peer.sky3d_preview.serial, peer_token);
    assert!(peer.instance_workers_busy());
    assert!(!origin.instance_workers_busy());
    assert_eq!(origin.sky3d_preview_state()["status"], "cancelled");
    assert_eq!(
        peer.finish_sky3d_preview(Actor::Agent, peer_request.run_with(prepared))
            .unwrap()["status"],
        "ready"
    );
}

#[test]
fn replacement_resets_selection_and_prevents_overlapping_work_until_old_result_drains() {
    let (directory, mut session, camera, _, _) = fixture();
    let path = directory.path().join("sky.vmf");
    session.save(Actor::User, Some(&path)).unwrap();
    session.set_sky3d_preview(true, Some(camera)).unwrap();
    let request = session.begin_sky3d_preview(Actor::User).unwrap();
    session.new_map(Actor::User, true).unwrap();
    assert!(request.control.cancelled());
    assert!(!session.view().sky3d_preview);
    assert_eq!(session.sky3d_preview.camera, None);
    assert!(session.begin_sky3d_preview(Actor::User).is_err());
    session.open(Actor::User, &path, true).unwrap();
    configure(&mut session, camera);
    assert!(session.begin_sky3d_preview(Actor::User).is_err());
    let completed = request.run_with(prepared);
    assert!(session.instance_workers_busy());
    assert!(
        session
            .finish_sky3d_preview(Actor::User, completed)
            .is_err()
    );
    assert!(!session.instance_workers_busy());
    assert!(!session.view().sky3d_preview);
    assert_eq!(session.sky3d_preview_state()["status"], "not_prepared");
    assert_eq!(finish_synthetic(&mut session)["status"], "ready");
}

#[test]
fn cancelled_panicked_and_dropped_requests_cannot_publish_or_strand_admission() {
    let (_directory, mut session, camera, _, _) = fixture();
    configure(&mut session, camera);
    let request = session.begin_sky3d_preview(Actor::User).unwrap();
    session.cancel_sky3d_preview().unwrap();
    let report = session
        .finish_sky3d_preview(Actor::User, request.run_with(prepared))
        .unwrap();
    assert_eq!(report["status"], "cancelled");
    assert_eq!(report["ready"], false);
    let request = session.begin_sky3d_preview(Actor::User).unwrap();
    let completed = request.run_with(|_| panic!("injected preview worker panic"));
    let report = session
        .finish_sky3d_preview(Actor::User, completed)
        .unwrap();
    assert_eq!(report["status"], "failed");
    assert!(report["issue"].as_str().unwrap().contains("panicked"));
    let request = session.begin_sky3d_preview(Actor::User).unwrap();
    drop(request);
    assert_eq!(session.sky3d_preview_state()["status"], "cancelled");
    assert!(!session.instance_workers_busy());
    assert_eq!(finish_synthetic(&mut session)["status"], "ready");
}

#[test]
fn document_material_and_visibility_changes_reject_even_finished_worker_results() {
    let (_directory, mut session, camera, wall, _) = fixture();
    configure(&mut session, camera);
    for change in ["source", "materials", "visibility", "region"] {
        let completed = session
            .begin_sky3d_preview(Actor::User)
            .unwrap()
            .run_with(prepared);
        match change {
            "source" => session
                .set_entity_properties(
                    Actor::User,
                    camera,
                    &[("targetname".into(), Some("renamed".into()))],
                )
                .unwrap(),
            "materials" => {
                session.reload_material_assets(Actor::User);
            }
            "visibility" => {
                let snapshot = session.working_region_stamp();
                session
                    .set_editor_visibility(
                        Actor::User,
                        editor_visibility::Action::Hide(vec![wall]),
                        &snapshot,
                    )
                    .unwrap();
            }
            "region" => {
                let snapshot = session.working_region_stamp();
                session
                    .set_working_region(
                        Actor::User,
                        working_region::Action::Set {
                            bounds: [DVec3::splat(-256.0), DVec3::splat(256.0)],
                            enabled: true,
                        },
                        &snapshot,
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            session
                .finish_sky3d_preview(Actor::User, completed)
                .is_err()
        );
        let state = session.sky3d_preview_state();
        assert_eq!(state["status"], "stale", "{change}: {state}");
        assert_eq!(state["ready"], false);
        assert!(!session.instance_workers_busy());
    }
}

#[test]
fn visibility_filter_preserves_complete_enclosure_and_filters_whole_detail_owner() {
    let (_directory, mut session, camera, wall, detail) = fixture();
    configure(&mut session, camera);
    let request = session.begin_sky3d_preview(Actor::User).unwrap();
    let initial = session
        .finish_sky3d_preview(Actor::User, request.run())
        .unwrap();
    assert_eq!(initial["status"], "ready", "{initial}");
    let snapshot = session.working_region_stamp();
    session
        .set_editor_visibility(
            Actor::User,
            editor_visibility::Action::Hide(vec![wall, detail]),
            &snapshot,
        )
        .unwrap();
    let request = session.begin_sky3d_preview(Actor::User).unwrap();
    let hidden = session
        .finish_sky3d_preview(Actor::User, request.run())
        .unwrap();
    assert_eq!(hidden["status"], "ready", "{hidden}");
    assert!(
        hidden["stats"]["geometry"]["vertices"].as_u64().unwrap()
            < initial["stats"]["geometry"]["vertices"].as_u64().unwrap()
    );
}

#[test]
fn shutdown_cancels_owned_worker_and_prevents_publication() {
    let (_directory, mut session, camera, _, _) = fixture();
    configure(&mut session, camera);
    let request = session.begin_sky3d_preview(Actor::User).unwrap();
    let close = session.document_close_review();
    assert_eq!(close.inspection()["work"]["sky3d"]["worker_active"], true);
    session.shutdown();
    assert!(request.control.cancelled());
    assert!(session.begin_sky3d_preview(Actor::User).is_err());
    let completed = request.run_with(prepared);
    assert!(session.instance_workers_busy());
    assert!(
        session
            .finish_sky3d_preview(Actor::User, completed)
            .is_err()
    );
    assert!(!session.instance_workers_busy());
    assert!(session.sky3d_preview.ready.is_none());
}

#[test]
fn shutdown_preserves_other_owned_reviews_while_invalidating_scene_with_idle_or_running_sky_preparation()
 {
    for preparing in [false, true] {
        let (_directory, mut session, camera, floor, _) = fixture();
        let ObjectId::Solid(floor) = floor else {
            unreachable!()
        };
        let ObjectId::Solid(cutter) = session
            .create_block(
                Actor::User,
                DVec3::new(-8.0, -160.0, -32.0),
                DVec3::new(8.0, 160.0, 16.0),
                "test/checker",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let request = if preparing {
            configure(&mut session, camera);
            Some(session.begin_sky3d_preview(Actor::User).unwrap())
        } else {
            None
        };
        let stamp = session.carve_stamp();
        session.preview_carve(cutter, &[floor], &stamp).unwrap();
        let carve = session.carve_state();
        assert!(carve["preview"].is_object());
        let render = session.render_document().vmf().clone();
        let source = session.document.vmf().clone();
        let history = session.history_snapshot();
        let selection = session.selected_ids();
        let camera = session.camera();
        let scene = session.scene_serial();
        let view = session.view_serial();
        let review = session.document_close_review();
        let sky_work = review.inspection()["work"]["sky3d"].clone();
        assert!(review.requires_discard());
        session.shutdown();
        let mut after = session.carve_state();
        assert_ne!(after["stamp"], carve["stamp"]);
        after["stamp"] = carve["stamp"].clone();
        assert_eq!(after, carve);
        assert_eq!(session.render_document().vmf(), &render);
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.selected_ids(), selection);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.scene_serial(), scene + 1);
        assert_eq!(session.view_serial(), view);
        assert!(session.ensure_document_close_review(&review, true).is_err());
        if let Some(request) = request {
            assert!(request.control.cancelled());
            assert!(session.instance_workers_busy());
            let completed = request.run_with(prepared);
            assert!(
                session
                    .finish_sky3d_preview(Actor::User, completed)
                    .is_err()
            );
            assert!(!session.instance_workers_busy());
        } else {
            assert_eq!(
                session.document_close_review().inspection()["work"]["sky3d"],
                sky_work
            );
        }
    }
}

#[test]
fn missing_admitted_texture_fails_preparation_after_material_reload() {
    let (directory, mut session, camera, _, _) = fixture();
    configure(&mut session, camera);
    let request = session.begin_sky3d_preview(Actor::User).unwrap();
    let initial = session
        .finish_sky3d_preview(Actor::User, request.run())
        .unwrap();
    assert_eq!(initial["status"], "ready", "{initial}");
    let source = session.document.vmf().clone();
    let history = session.history_snapshot();
    std::fs::remove_file(directory.path().join("materials/test/checker.vtf")).unwrap();
    session.reload_material_assets(Actor::User);
    assert_eq!(session.sky3d_preview_state()["status"], "stale");
    let request = session.begin_sky3d_preview(Actor::User).unwrap();
    let missing = session
        .finish_sky3d_preview(Actor::User, request.run())
        .unwrap();
    assert_eq!(missing["status"], "failed", "{missing}");
    assert_eq!(missing["ready"], false);
    assert!(missing["issue"].as_str().unwrap().contains("texture"));
    assert!(session.build_scene().sky3d.is_none());
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
}

#[test]
fn explicit_selected_camera_remains_inspectable_beyond_camera_list_budget() {
    let (_directory, mut session, _, _, _) = fixture();
    let camera = session
        .edit(Actor::User, "many sky cameras", |tx| {
            let mut last = None;
            for _ in 0..MAX_CAMERAS {
                last = Some(edit::create_entity(
                    tx,
                    "sky_camera",
                    Some(DVec3::new(0.0, 0.0, 64.0)),
                    &[("scale".into(), "16".into())],
                    Vec::new(),
                )?);
            }
            Ok(last.unwrap())
        })
        .unwrap();
    let ObjectId::Entity(camera) = camera else {
        unreachable!()
    };
    configure(&mut session, camera);
    let state = session.sky3d_preview_state();
    assert_eq!(state["cameras"].as_array().unwrap().len(), MAX_CAMERAS);
    assert_eq!(state["cameras_truncated"], true);
    assert_eq!(state["selected_camera"]["id"], format!("entity:{camera}"));
    assert_eq!(state["selected_camera"]["ready"], true);
}

#[test]
fn snapshot_admission_bounds_opaque_metadata_before_copying_and_preserves_source() {
    let (_directory, session, _, _, _) = fixture();
    let mut vmf = session.document.vmf().clone();
    let (copy, bytes) = snapshot::capture(&vmf).unwrap();
    assert_eq!(copy, vmf);
    assert!(bytes < snapshot::MAX_BYTES);
    vmf.extra
        .push(Node::string("opaque", "x".repeat(snapshot::MAX_BYTES)));
    assert!(snapshot::capture(&vmf).unwrap_err().contains("32 MiB"));
    let mut vmf = session.document.vmf().clone();
    let mut node = Node::string("leaf", "value");
    for _ in 0..snapshot::MAX_DEPTH {
        node = Node::block("nested", vec![node]);
    }
    vmf.extra.push(node);
    assert!(snapshot::capture(&vmf).unwrap_err().contains("nested"));
}

#[test]
fn temporary_view_and_camera_overrides_report_actual_eligibility_without_changing_shared_settings()
{
    let (_directory, mut session, camera, _, _) = fixture();
    configure(&mut session, camera);
    finish_synthetic(&mut session);
    let source = session.document.vmf().clone();
    let history = session.history_snapshot();
    let shared_view = session.view();
    let shared_camera = session.camera();
    let ordinary = session.sky3d_preview_state();
    assert_eq!(ordinary["enabled"], false);
    assert_eq!(ordinary["requested"], false);
    assert_eq!(ordinary["suppression_reason"], "disabled");
    let mut view = render::ViewOptions {
        sky3d_preview: true,
        surface: render::SurfaceView::Textured,
        tool_textures: true,
        ..shared_view
    };
    let active = session.sky3d_preview_state_for(view, shared_camera);
    assert_eq!(active["enabled"], false);
    assert_eq!(active["requested"], true);
    assert_eq!(active["active"], true);
    assert_eq!(active["eligible"], true);
    assert_eq!(active["suppression_reason"], Value::Null);
    let mut orthographic = shared_camera;
    orthographic.projection = render::Projection::Top;
    let top = session.sky3d_preview_state_for(view, orthographic);
    assert_eq!(top["suppression_reason"], "unsupported_view");
    assert_eq!(top["active"], false);
    view.surface = render::SurfaceView::Wireframe;
    let wireframe = session.sky3d_preview_state_for(view, shared_camera);
    assert_eq!(wireframe["suppression_reason"], "unsupported_view");
    view.surface = render::SurfaceView::Textured;
    view.tool_textures = false;
    let tools_hidden = session.sky3d_preview_state_for(view, shared_camera);
    assert_eq!(tools_hidden["suppression_reason"], Value::Null);
    assert_eq!(tools_hidden["eligible"], true);
    assert_eq!(session.sky3d_preview_state(), ordinary);
    assert_eq!(session.view(), shared_view);
    assert_eq!(session.camera(), shared_camera);
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
}
