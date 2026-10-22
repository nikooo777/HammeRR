use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn software_child_overlay_counts_keep_target_geometry_frame_and_root_drafts() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    fn preview(session: &mut Session, source: map_core::surface::FaceId) -> u64 {
        session
            .preview_overlay(
                Actor::User,
                session::overlay::Request {
                    source,
                    options: map_core::overlay::Options {
                        material: "test/checker".into(),
                        ..Default::default()
                    },
                },
                &session.overlay_stamp(),
            )
            .unwrap()["draft"]["token"]
            .as_u64()
            .unwrap()
    }
    fn top(solid: &source_formats::vmf::Solid) -> map_core::surface::FaceId {
        map_core::surface::FaceId {
            solid: solid.id,
            side: solid
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id,
        }
    }
    let (directory, game) = crate::test_assets::game();
    let parent = directory.path().join("parent.vmf");
    let child = directory.path().join("child.vmf");
    let mut session = Session::new(Ok(game));
    session
        .create_block(
            Actor::User,
            DVec3::new(0.0, 0.0, 32.0),
            DVec3::new(16.0, 16.0, 48.0),
            "test/checker",
            false,
        )
        .unwrap();
    let face = top(&session.document().vmf().world.solids[0]);
    let token = preview(&mut session, face);
    let reviewed = session.review_overlay(Actor::User, token).unwrap()["draft"]["token"]
        .as_u64()
        .unwrap();
    session.apply_overlay(Actor::User, reviewed).unwrap();
    session.save(Actor::User, Some(&child)).unwrap();
    std::fs::write(
        &parent,
        crate::test_assets::room().write(&Default::default()),
    )
    .unwrap();
    session.open(Actor::User, &parent, false).unwrap();
    let owner = session
        .create_entity(
            Actor::User,
            "func_instance",
            Some(DVec3::ZERO),
            BTreeMap::from([("file".into(), "child.vmf".into())]),
        )
        .unwrap();
    let root = top(&session.document().vmf().world.solids[0]);
    let token = preview(&mut session, root);
    session.apply_overlay(Actor::User, token).unwrap();
    session.save(Actor::User, None).unwrap();
    session.select(&[owner], SelectMode::Replace).unwrap();
    session.set_view(render::ViewOptions {
        instance_contents: true,
        ..session.view()
    });
    preview(&mut session, root);
    let draft = session.get_overlay_state();
    let memory = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let settings = session.compile_settings();
    let parent_bytes = std::fs::read(&parent).unwrap();
    let child_bytes = std::fs::read(&child).unwrap();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    let output = &result.as_ref().unwrap().output;
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert_eq!(output.stats.instances.as_ref().unwrap().overlays, 1);
    assert_eq!(output.stats.overlays, 2);
    session.finish_compile(Actor::User, &result);
    let request = session
        .begin_instance_contents_refresh(Actor::User)
        .unwrap();
    let report = session
        .finish_instance_contents_refresh(Actor::User, request.run())
        .unwrap();
    assert_eq!(report["state"], "ready");
    assert_eq!(report["stats"]["overlays"], 1);
    assert_eq!(report["overlay_preview"]["available"], false);
    assert_eq!(report["available"], true);
    assert_eq!(report["displayed"], true);
    assert_eq!(report["brushes_total"], 1);
    assert_eq!(report["face_provenance_total"], 6);
    assert_eq!(
        report["owners"][0]["bounds"],
        serde_json::json!([[0.0, 0.0, 32.0], [16.0, 16.0, 48.0]])
    );
    let shared = Arc::new(Mutex::new(session));
    let contents = Controller::new(shared.clone()).unwrap();
    let build = crate::compile_ui::Controller::new(&shared).unwrap();
    build.refresh(&lock(&shared));
    let count = "Overlay helpers: 1 · expanded before compile-region filtering";
    let reason = editor_core::session::instance_contents::OVERLAY_PREVIEW_REASON;
    assert_eq!(report["overlay_preview"]["reason"], reason);
    assert!(contents.window.get_summary().contains(count));
    assert!(contents.window.get_summary().contains(reason));
    assert!(build.window.get_details().contains(count));
    assert!(build.window.get_details().contains(reason));
    assert!(
        build
            .window
            .get_details()
            .contains("Overlays: 2 · Surface references:")
    );
    assert!(contents.window.get_can_frame());
    {
        let session = lock(&shared);
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(session.get_overlay_state(), draft);
        assert_eq!(session.selected_ids(), vec![owner]);
    }
    contents.window.invoke_frame_contents();
    let session = lock(&shared);
    assert_ne!(session.camera(), camera);
    assert_eq!(session.document().vmf(), &memory);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.compile_settings(), settings);
    assert_eq!(session.get_overlay_state(), draft);
    assert_eq!(std::fs::read(&parent).unwrap(), parent_bytes);
    assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
}

#[test]
fn software_controls_share_visibility_reports_frame_clear_and_preserve_parent() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (_directory, game, parent, _child) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &parent, true).unwrap();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera();
    let request = session
        .begin_instance_contents_refresh(Actor::User)
        .unwrap();
    let result = request.run();
    session
        .finish_instance_contents_refresh(Actor::User, result)
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let app = AppWindow::new().unwrap();
    super::super::bind_view_controls(&app, shared.clone());
    super::super::refresh_view_controls(&app, &lock(&shared));
    assert!(!app.get_instance_contents());
    let (serials, mut displayed_key, mut draw_options) = {
        let session = lock(&shared);
        (
            (session.scene_serial(), session.view_serial()),
            session.render_scene_key(session.view()),
            session.view(),
        )
    };
    app.set_instance_contents(true);
    app.invoke_options_changed();
    assert!(lock(&shared).view().instance_contents);
    {
        let mut session = lock(&shared);
        displayed_key =
            super::super::prepare_scene_refresh(&mut session, displayed_key, &mut draw_options)
                .expect("Reference-only enable must refresh the production viewport route");
        assert!(draw_options.instance_contents);
        assert!(
            !session
                .build_scene_for_view(draw_options)
                .reference_lines
                .is_empty()
        );
        assert_eq!((session.scene_serial(), session.view_serial()), serials);
        assert!(
            super::super::prepare_scene_refresh(&mut session, displayed_key, &mut draw_options,)
                .is_none()
        );
    }
    for enabled in [false, true] {
        let mut session = lock(&shared);
        let view = session
            .resolve_view_update(session::ViewUpdate {
                instance_contents: Some(enabled),
                ..Default::default()
            })
            .unwrap();
        session.set_view(view);
        displayed_key =
            super::super::prepare_scene_refresh(&mut session, displayed_key, &mut draw_options)
                .expect("Shared visibility changes must update retained draw options");
        assert_eq!(draw_options.instance_contents, enabled);
        let mesh = session.build_scene_for_view(draw_options);
        assert_eq!(!mesh.reference_lines.is_empty(), enabled);
        assert_eq!(
            session.instance_contents_rendered_report()["displayed"],
            enabled
        );
        assert_eq!((session.scene_serial(), session.view_serial()), serials);
    }
    let controller = Controller::new(shared.clone()).unwrap();
    assert!(
        controller
            .window
            .get_summary()
            .contains("1 instance from 1 file")
    );
    assert!(controller.window.get_owners().contains("child.vmf"));
    assert!(
        !controller
            .window
            .get_summary()
            .contains("Instance terrain patches:")
    );
    assert!(!controller.window.get_owners().contains("Terrain:"));
    assert!(
        !controller
            .window
            .get_summary()
            .contains("Expanded static lights:")
    );
    assert!(
        !controller
            .window
            .get_summary()
            .contains("Expanded static props:")
    );
    assert!(
        !controller
            .window
            .get_summary()
            .contains("Expanded lighting origins:")
    );
    assert!(controller.window.get_can_frame());
    controller.window.show().unwrap();
    controller
        .window
        .window()
        .set_size(slint::PhysicalSize::new(850, 690));
    crate::ui::tests::save_ui_snapshot(controller.window.window(), "instance-contents");
    controller.window.invoke_frame_contents();
    assert_ne!(lock(&shared).camera(), camera);
    lock(&shared).set_camera(camera);
    controller.window.invoke_clear_contents();
    assert!(!controller.window.get_can_frame());
    assert!(lock(&shared).view().instance_contents);
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.camera(), camera);
}

#[test]
fn software_terrain_counts_and_provenance_match_build_details_and_preserve_parent() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, mut game, parent, child) = crate::test_assets::instance_map(false);
    let materials = directory.path().join("materials/dev");
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::write(
        materials.join("dev_measuregeneric01.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    game.mounts.push(source_formats::gamefs::MountedPath {
        ids: vec!["game".into()],
        mount: source_formats::gamefs::Mount::Directory(directory.path().to_owned()),
    });
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &child, true).unwrap();
    let solid = &session.document().vmf().world.solids[0];
    let side = solid
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
        .unwrap()
        .id;
    let face = map_core::surface::FaceId {
        solid: solid.id,
        side,
    };
    session.create_terrain(Actor::User, &[face], 2, 0).unwrap();
    session
        .preview_terrain_edit(
            Actor::User,
            &[face],
            map_core::displacement::Operation::VertexMove {
                vertices: vec![12],
                offset: DVec3::new(0.0, 0.0, 8.0),
            },
        )
        .unwrap();
    session.commit_face_preview(Actor::User).unwrap();
    session.save(Actor::User, None).unwrap();
    let child_bytes = std::fs::read(&child).unwrap();
    session.open(Actor::User, &parent, true).unwrap();
    session
        .update_compile_settings(Actor::User, Some("fullbright"), Some("none"))
        .unwrap();
    session
        .begin_property_edit(session::PropertyTarget::World)
        .unwrap();
    let token = session.property_draft().unwrap().token;
    session
        .append_property_change(
            token,
            session::PropertyChange::Set {
                key: "message".into(),
                value: "Retain the terrain review".into(),
            },
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let settings = session.compile_settings();
    let draft = serde_json::json!(session.property_draft());
    let parent_bytes = std::fs::read(&parent).unwrap();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    assert!(
        result.as_ref().unwrap().output.bsp.is_some(),
        "{:?}",
        result.as_ref().unwrap().output.report
    );
    session.finish_compile(Actor::User, &result);
    assert_eq!(session.compile_status()["job"]["result"]["published"], true);
    let request = session
        .begin_instance_contents_refresh(Actor::User)
        .unwrap();
    session
        .finish_instance_contents_refresh(Actor::User, request.run())
        .unwrap();
    let report = session.inspect_instance_contents(0, 64).unwrap();
    assert_eq!(report["stats"]["terrain_patches"], 1);
    assert_eq!(report["stats"]["terrain_vertices"], 25);
    assert_eq!(report["geometry"]["triangles"], 44);
    let shared = Arc::new(Mutex::new(session));
    let contents = Controller::new(shared.clone()).unwrap();
    let build = crate::compile_ui::Controller::new(&shared).unwrap();
    build.refresh(&lock(&shared));
    let summary = "Instance terrain patches: 1 · Authored grid vertices: 25";
    assert!(build.window.get_details().contains(summary));
    assert!(contents.window.get_summary().contains(summary));
    assert!(contents.window.get_owners().contains(&format!(
        "child.vmf · brush {} · side {}",
        face.solid, face.side
    )));
    assert!(
        contents
            .window
            .get_owners()
            .contains("Power 2 · 25 grid vertices · 32 triangles")
    );
    build.window.show().unwrap();
    build.window.window().take_snapshot().unwrap();
    build
        .window
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(360.0, 450.0),
            delta_x: 0.0,
            delta_y: -10_000.0,
        });
    crate::ui::tests::save_ui_snapshot(build.window.window(), "build-instance-terrain");
    contents.window.show().unwrap();
    contents
        .window
        .window()
        .set_size(slint::PhysicalSize::new(850, 690));
    crate::ui::tests::save_ui_snapshot(contents.window.window(), "instance-contents-terrain");
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.compile_settings(), settings);
    assert_eq!(serde_json::json!(session.property_draft()), draft);
    assert_eq!(std::fs::read(&parent).unwrap(), parent_bytes);
    assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
}

#[test]
fn software_controls_show_refresh_failure_without_relocking_the_session() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("No game assets required".into()));
    session
        .create_entity(
            Actor::User,
            "func_instance",
            Some(DVec3::ZERO),
            BTreeMap::from([("file".into(), "child.vmf".into())]),
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(shared.clone()).unwrap();
    controller.window.invoke_refresh_contents();
    assert!(
        controller
            .window
            .get_message()
            .contains("Save the parent VMF")
    );
    assert!(!controller.window.get_loading());
    assert!(!controller.window.get_can_frame());
}

#[test]
fn fixed_contents_root_child_and_timer_reject_after_real_drain_and_release_owners() {
    i_slint_backend_testing::init_no_event_loop();
    let (directory, game, parent, _child) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session.open(Actor::User, &parent, true).unwrap();
    let request = session
        .begin_instance_contents_refresh(Actor::User)
        .unwrap();
    session
        .finish_instance_contents_refresh(Actor::User, request.run())
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    let app = AppWindow::new().unwrap();
    bind_controller(&app, controller.clone());
    app.invoke_inspect_instance_contents();
    assert!(controller.window.window().is_visible());
    let camera = lock(&shared).camera();
    controller.window.invoke_frame_contents();
    assert_ne!(lock(&shared).camera(), camera);
    controller.window.set_summary("refresh pending".into());
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    assert_ne!(controller.window.get_summary(), "refresh pending");
    controller.window.hide().unwrap();
    controller.window.set_summary("hidden unchanged".into());
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    assert_eq!(controller.window.get_summary(), "hidden unchanged");
    let mut peer = Session::new(Err("peer assets unavailable".into()));
    peer.open(Actor::User, &parent, true).unwrap();
    let peer_source = peer.document().vmf().clone();
    let peer_history = peer.history_snapshot();
    document.shutdown().unwrap();
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let camera = lock(&shared).camera();
    let state = lock(&shared).inspect_instance_contents(0, 64).unwrap();
    let offset = controller.offset.get();
    let token = controller.token.get();
    app.invoke_inspect_instance_contents();
    assert_eq!(app.get_message(), "the document is closed");
    assert!(!controller.window.window().is_visible());
    controller.window.invoke_refresh_contents();
    controller.window.invoke_cancel_refresh();
    controller.window.invoke_clear_contents();
    controller.window.invoke_frame_contents();
    controller.window.invoke_next();
    controller.window.invoke_previous();
    assert_eq!(controller.window.get_message(), "the document is closed");
    assert_eq!(controller.offset.get(), offset);
    assert_eq!(controller.token.get(), token);
    controller.window.show().unwrap();
    controller.window.set_message("terminal message".into());
    controller.window.set_summary("terminal summary".into());
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    assert_eq!(controller.window.get_message(), "terminal message");
    assert_eq!(controller.window.get_summary(), "terminal summary");
    controller.window.invoke_close_dialog();
    assert!(!controller.window.window().is_visible());
    assert_eq!(
        lock(&shared).inspect_instance_contents(0, 64).unwrap(),
        state
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(peer.document().vmf(), &peer_source);
    assert_eq!(peer.history_snapshot(), peer_history);
    let retained = controller.window.clone_strong();
    let weak_controller = Rc::downgrade(&controller);
    let weak_session = Arc::downgrade(&shared);
    drop(controller);
    drop(app);
    assert!(weak_controller.upgrade().is_none());
    drop(access);
    drop(shared);
    drop(document);
    assert!(weak_session.upgrade().is_none());
    retained.invoke_refresh_contents();
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    assert_eq!(retained.get_summary(), "terminal summary");
}

#[test]
fn instance_binders_preserve_failed_shutdown_error_and_timer_messages_without_retry() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let recovery = directory.path().join("recovery");
    let mut session = Session::new(Err("no assets required".into()));
    session.configure_recovery(Ok(recovery.clone()));
    session
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(16.0), "test", false)
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let app = AppWindow::new().unwrap();
    let _contents = bind_with_access(&app, &access);
    let _reference = super::super::instance_reference::bind_with_access(&app, &access);
    let _extraction = super::super::instance_extraction::bind_with_access(&app, &access);
    let controller = Controller::new_with_access(&access).unwrap();
    controller.window.show().unwrap();
    std::fs::write(&recovery, b"not a directory").unwrap();
    let error = document.shutdown().unwrap_err();
    let expected = format!("document shutdown failed: {error}");
    app.invoke_inspect_instance_contents();
    assert_eq!(app.get_message(), expected);
    app.invoke_create_instance_reference();
    assert_eq!(app.get_message(), expected);
    app.invoke_edit_instance_reference();
    assert_eq!(app.get_message(), expected);
    app.invoke_extract_instance();
    assert_eq!(app.get_message(), expected);
    controller.window.invoke_frame_contents();
    assert_eq!(controller.window.get_message(), expected);
    controller
        .window
        .set_message("preserve failed status".into());
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    assert_eq!(controller.window.get_message(), "preserve failed status");
    controller.window.invoke_close_dialog();
    assert!(!controller.window.window().is_visible());
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    assert_eq!(document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
}

#[test]
fn software_brush_model_counts_refuse_frame_without_reference_geometry() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = crate::test_assets::game();
    std::fs::write(
        directory.path().join("materials/test/origin.vmt"),
        "LightmappedGeneric { %compileorigin 1 }",
    )
    .unwrap();
    let parent = directory.path().join("parent.vmf");
    let child = directory.path().join("child.vmf");
    let mut session = Session::new(Ok(game));
    let mut blocks = Vec::new();
    for (mins, maxs) in [
        ([-48.0, -32.0, 16.0], [48.0, 32.0, 24.0]),
        ([-8.0, -32.0, 32.0], [8.0, 32.0, 40.0]),
    ] {
        blocks.push(
            session
                .create_block(Actor::User, mins.into(), maxs.into(), "test/checker", false)
                .unwrap(),
        );
    }
    blocks.push(
        session
            .create_block(
                Actor::User,
                DVec3::new(12.0, -4.0, 0.0),
                DVec3::new(20.0, 20.0, 8.0),
                "test/origin",
                false,
            )
            .unwrap(),
    );
    session
        .tie_to_entity(
            Actor::User,
            &blocks,
            "func_brush",
            std::collections::BTreeMap::from([("vrad_brush_cast_shadows".into(), "1".into())]),
        )
        .unwrap();
    session.save(Actor::User, Some(&child)).unwrap();
    std::fs::write(
        &parent,
        crate::test_assets::room().write(&Default::default()),
    )
    .unwrap();
    session.open(Actor::User, &parent, false).unwrap();
    let owner = session
        .create_entity(
            Actor::User,
            "func_instance",
            Some(DVec3::ZERO),
            std::collections::BTreeMap::from([("file".into(), "child.vmf".into())]),
        )
        .unwrap();
    session.save(Actor::User, None).unwrap();
    session.select(&[owner], SelectMode::Replace).unwrap();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    let output = &result.as_ref().unwrap().output;
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert_eq!(output.stats.instances.as_ref().unwrap().brush_models, 1);
    assert_eq!(
        output
            .stats
            .instances
            .as_ref()
            .unwrap()
            .consumed_origin_brushes,
        1
    );
    assert_eq!(output.stats.brush_shadow_models, 0);
    session.finish_compile(Actor::User, &result);
    let request = session
        .begin_instance_contents_refresh(Actor::User)
        .unwrap();
    let report = session
        .finish_instance_contents_refresh(Actor::User, request.run())
        .unwrap();
    assert_eq!(report["brush_model_preview"]["available"], false);
    assert_eq!(report["brush_model_preview"]["preparation"], "deferred");
    assert!(report["brush_model_preview"]["consumed_origin_brushes"].is_null());
    let memory = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let settings = session.compile_settings();
    let shared = Arc::new(Mutex::new(session));
    let contents = Controller::new(shared.clone()).unwrap();
    let build = crate::compile_ui::Controller::new(&shared).unwrap();
    build.refresh(&lock(&shared));
    let count = "Expanded brush models: 1 · before compile-region filtering";
    let reason = editor_core::session::instance_contents::BRUSH_MODEL_PREVIEW_REASON;
    assert!(contents.window.get_summary().contains(count));
    assert!(contents.window.get_summary().contains(reason));
    assert!(build.window.get_details().contains(count));
    assert!(
        build.window.get_details().contains(
            "Consumed child origin brushes: 1 · expanded before compile-region filtering"
        )
    );
    assert!(build.window.get_details().contains(reason));
    assert!(build.window.get_details().contains(
        "Brush global shadows: 1 admitted Shadows Yes source request(s); not baked in Fullbright"
    ));
    assert!(!build.window.get_details().contains("baked models"));
    assert!(!contents.window.get_can_frame());
    contents.window.invoke_frame_contents();
    assert!(
        contents
            .window
            .get_message()
            .contains("no visible geometry")
    );
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &memory);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.compile_settings(), settings);
    drop(session);
    lock(&shared)
        .set_compile_settings(Actor::User, "direct_ldr")
        .unwrap();
    let request = lock(&shared).begin_compile(Actor::User, None).unwrap();
    let result = request.run();
    let output = &result.as_ref().unwrap().output;
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert_eq!(output.stats.brush_shadow_models, 1);
    lock(&shared).finish_compile(Actor::User, &result);
    build.refresh(&lock(&shared));
    assert!(
        build
            .window
            .get_details()
            .contains("Brush global shadows: 1 baked models")
    );
    assert!(
        !build
            .window
            .get_details()
            .contains("not baked in Fullbright")
    );
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &memory);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
}
