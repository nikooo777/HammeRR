use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn software_environment_lights_share_sun_counts_without_native_sky_or_reference_geometry() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (_directory, game, parent, child) = crate::test_assets::instance_map(false);
    let definitions = source_formats::fgd::Catalog::parse(
        r#"
@PointClass = light_environment [
    angles(angle) : "Pitch Yaw Roll (Y Z X)" : "0 0 0"
    pitch(integer) : "Pitch" : 0
    _light(color255) : "Brightness" : "255 255 255 200"
    _ambient(color255) : "Ambient" : "255 255 255 20"
    _lightHDR(color255) : "BrightnessHDR" : "-1 -1 -1 1"
    _lightscaleHDR(float) : "BrightnessScaleHDR" : "1"
    _ambientHDR(color255) : "AmbientHDR" : "-1 -1 -1 1"
    _AmbientScaleHDR(float) : "AmbientScaleHDR" : "1"
    SunSpreadAngle(float) : "SunSpreadAngle" : 0
]
"#,
    )
    .unwrap();
    std::fs::write(
        &child,
        map_core::Document::empty().vmf().write(&Default::default()),
    )
    .unwrap();
    let mut session = Session::new(Ok(game));
    session.set_definitions(Actor::User, definitions);
    session.open(Actor::User, &child, true).unwrap();
    session
        .create_entity(
            Actor::User,
            "light_environment",
            Some(DVec3::new(80.0, -16.0, 96.0)),
            BTreeMap::new(),
        )
        .unwrap();
    session.save(Actor::User, None).unwrap();
    session.open(Actor::User, &parent, false).unwrap();
    let owner = ObjectId::Entity(session.document().vmf().entities.last().unwrap().id);
    session.select(&[owner], SelectMode::Replace).unwrap();
    session.set_view(render::ViewOptions {
        instance_contents: true,
        ..session.view()
    });
    let memory = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let settings = session.compile_settings();
    let parent_bytes = std::fs::read(&parent).unwrap();
    let child_bytes = std::fs::read(&child).unwrap();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    let output = &result.as_ref().unwrap().output;
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert_eq!(
        output.stats.instances.as_ref().unwrap().environment_lights,
        1
    );
    assert_eq!(output.stats.instances.as_ref().unwrap().point_lights, 0);
    assert_eq!(output.stats.instances.as_ref().unwrap().spot_lights, 0);
    assert_eq!(output.stats.world_lights, 0);
    session.finish_compile(Actor::User, &result);
    let request = session
        .begin_instance_contents_refresh(Actor::User)
        .unwrap();
    let report = session
        .finish_instance_contents_refresh(Actor::User, request.run())
        .unwrap();
    assert_eq!(report["state"], "ready");
    assert_eq!(report["stats"]["environment_lights"], 1);
    assert!(report["stats"].get("point_lights").is_none());
    assert!(report["stats"].get("spot_lights").is_none());
    assert_eq!(report["static_light_preview"]["available"], false);
    assert_eq!(report["available"], false);
    assert_eq!(report["geometry"]["vertices"], 0);
    assert!(report["owners"][0]["bounds"].is_null());
    let shared = Arc::new(Mutex::new(session));
    let contents = Controller::new(shared.clone()).unwrap();
    let build = crate::compile_ui::Controller::new(&shared).unwrap();
    build.refresh(&lock(&shared));
    let count = "Sun and sky lights: 1 · expanded before compile-region filtering";
    let reason = editor_core::session::instance_contents::ENVIRONMENT_LIGHT_PREVIEW_REASON;
    assert_eq!(report["static_light_preview"]["reason"], reason);
    assert!(contents.window.get_summary().contains(count));
    assert!(contents.window.get_summary().contains(reason));
    assert!(build.window.get_details().contains(count));
    assert!(build.window.get_details().contains(reason));
    assert!(
        !contents
            .window
            .get_summary()
            .contains("Expanded static lights:")
    );
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
    assert_eq!(session.selected_ids(), vec![owner]);
    assert_eq!(std::fs::read(&parent).unwrap(), parent_bytes);
    assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
}

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
fn software_occluder_helpers_keep_expanded_counts_separate_from_retained_and_reference_geometry() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (_directory, mut game, parent, child) = crate::test_assets::instance_map(false);
    let tools = game.game_dir.join("materials/tools");
    std::fs::create_dir_all(&tools).unwrap();
    for (name, material) in [
        ("toolsblack", "LightmappedGeneric {}"),
        (
            "toolsoccluder",
            "UnlitGeneric { $translucent 1 %compilenolight 1 }",
        ),
    ] {
        std::fs::write(tools.join(format!("{name}.vmt")), material).unwrap();
    }
    game.mounts.push(source_formats::gamefs::MountedPath {
        ids: vec!["game".into()],
        mount: source_formats::gamefs::Mount::Directory(game.game_dir.clone()),
    });
    let definitions = crate::steam::find_css_fgd(&game)
        .map_or_else(
            || source_formats::fgd::Catalog::parse(
                "@SolidClass = func_occluder [ StartActive(choices) : \"Start Active\" : 1 = [ 0 : \"No\" 1 : \"Yes\" ] ]",
            ),
            |path| source_formats::fgd::Catalog::load(&path),
        )
        .unwrap();
    std::fs::write(
        &child,
        map_core::Document::empty().vmf().write(&Default::default()),
    )
    .unwrap();
    let mut session = Session::new(Ok(game));
    session.set_definitions(Actor::User, definitions);
    session.open(Actor::User, &child, true).unwrap();
    for x in [-96.0, 192.0] {
        let brush = session
            .create_block(
                Actor::User,
                DVec3::new(x, -32.0, 32.0),
                DVec3::new(x + 16.0, 32.0, 128.0),
                "tools/toolsoccluder",
                false,
            )
            .unwrap();
        session
            .tie_to_entity(Actor::User, &[brush], "func_occluder", BTreeMap::new())
            .unwrap();
    }
    session.save(Actor::User, None).unwrap();
    session.open(Actor::User, &parent, false).unwrap();
    let stamp = session.compile_region_stamp();
    let token = session
        .preview_compile_region(
            Actor::User,
            Some(map_core::compile_region::Definition {
                enabled: true,
                bounds: map_core::compile_region::Bounds {
                    mins: DVec3::new(-128.0, -128.0, 0.0),
                    maxs: DVec3::new(128.0, 128.0, 192.0),
                },
            }),
            &stamp,
        )
        .unwrap()["draft"]["token"]
        .as_u64()
        .unwrap();
    session.apply_compile_region(Actor::User, token).unwrap();
    session
        .begin_property_edit(session::PropertyTarget::World)
        .unwrap();
    let token = session.property_draft().unwrap().token;
    session
        .append_property_change(
            token,
            session::PropertyChange::Set {
                key: "message".into(),
                value: "Keep the occluder review".into(),
            },
        )
        .unwrap();
    let memory = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let settings = session.compile_settings();
    let draft = serde_json::json!(session.property_draft());
    let parent_bytes = std::fs::read(&parent).unwrap();
    let child_bytes = std::fs::read(&child).unwrap();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    let output = &result.as_ref().unwrap().output;
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert_eq!(output.stats.instances.as_ref().unwrap().occluders, 2);
    assert_eq!(output.stats.occluders, 1);
    session.finish_compile(Actor::User, &result);
    let request = session
        .begin_instance_contents_refresh(Actor::User)
        .unwrap();
    let report = session
        .finish_instance_contents_refresh(Actor::User, request.run())
        .unwrap();
    assert_eq!(report["state"], "ready");
    assert_eq!(report["stats"]["occluders"], 2);
    assert_eq!(report["occluder_preview"]["available"], false);
    assert_eq!(report["available"], false);
    assert_eq!(report["geometry"]["vertices"], 0);
    assert_eq!(report["face_provenance_total"], 0);
    assert!(report["owners"][0]["bounds"].is_null());
    let shared = Arc::new(Mutex::new(session));
    let contents = Controller::new(shared.clone()).unwrap();
    let build = crate::compile_ui::Controller::new(&shared).unwrap();
    build.refresh(&lock(&shared));
    let count = "Occluder helpers: 2 · expanded before compile-region filtering";
    let reason = editor_core::session::instance_contents::OCCLUDER_PREVIEW_REASON;
    assert_eq!(report["occluder_preview"]["reason"], reason);
    assert!(contents.window.get_summary().contains(count));
    assert!(contents.window.get_summary().contains(reason));
    assert!(build.window.get_details().contains(count));
    assert!(build.window.get_details().contains(reason));
    assert!(
        build
            .window
            .get_details()
            .contains("Occluders: 1 · Polygons:")
    );
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
    assert_eq!(serde_json::json!(session.property_draft()), draft);
    assert_eq!(std::fs::read(&parent).unwrap(), parent_bytes);
    assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
}

#[test]
fn software_cubemap_samples_distinguish_expanded_and_retained_counts_without_reference_geometry() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (_directory, mut game, parent, child) = crate::test_assets::instance_map(false);
    let tools = game.game_dir.join("materials/tools");
    std::fs::create_dir_all(&tools).unwrap();
    std::fs::write(tools.join("toolsblack.vmt"), "LightmappedGeneric {}").unwrap();
    game.mounts.push(source_formats::gamefs::MountedPath {
        ids: vec!["game".into()],
        mount: source_formats::gamefs::Mount::Directory(game.game_dir.clone()),
    });
    let definitions = crate::steam::find_css_fgd(&game)
        .map_or_else(
            || {
                source_formats::fgd::Catalog::parse(
                    "@PointClass = env_cubemap [ cubemapsize(integer) : \"Cubemap Size\" : 0 ]",
                )
            },
            |path| source_formats::fgd::Catalog::load(&path),
        )
        .unwrap();
    let mut session = Session::new(Ok(game));
    session.set_definitions(Actor::User, definitions);
    for x in [16.0, 192.0] {
        session
            .create_entity(
                Actor::User,
                "env_cubemap",
                Some(DVec3::new(x, 8.0, 64.0)),
                BTreeMap::new(),
            )
            .unwrap();
    }
    session.save(Actor::User, Some(&child)).unwrap();
    session.open(Actor::User, &parent, false).unwrap();
    let stamp = session.compile_region_stamp();
    let token = session
        .preview_compile_region(
            Actor::User,
            Some(map_core::compile_region::Definition {
                enabled: true,
                bounds: map_core::compile_region::Bounds {
                    mins: DVec3::new(-64.0, -64.0, 0.0),
                    maxs: DVec3::new(64.0, 64.0, 160.0),
                },
            }),
            &stamp,
        )
        .unwrap()["draft"]["token"]
        .as_u64()
        .unwrap();
    session.apply_compile_region(Actor::User, token).unwrap();
    session
        .begin_property_edit(session::PropertyTarget::World)
        .unwrap();
    let token = session.property_draft().unwrap().token;
    session
        .append_property_change(
            token,
            session::PropertyChange::Set {
                key: "message".into(),
                value: "Keep the cubemap review".into(),
            },
        )
        .unwrap();
    let memory = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let settings = session.compile_settings();
    let draft = serde_json::json!(session.property_draft());
    let parent_bytes = std::fs::read(&parent).unwrap();
    let child_bytes = std::fs::read(&child).unwrap();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    let output = &result.as_ref().unwrap().output;
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert_eq!(output.stats.cubemap_samples, 1);
    assert_eq!(output.stats.instances.as_ref().unwrap().cubemaps, 2);
    session.finish_compile(Actor::User, &result);
    let request = session
        .begin_instance_contents_refresh(Actor::User)
        .unwrap();
    let report = session
        .finish_instance_contents_refresh(Actor::User, request.run())
        .unwrap();
    assert_eq!(report["state"], "ready");
    assert_eq!(report["stats"]["cubemaps"], 2);
    assert_eq!(report["cubemap_preview"]["available"], false);
    assert_eq!(report["available"], false);
    assert_eq!(report["geometry"]["vertices"], 0);
    assert_eq!(report["face_provenance_total"], 0);
    assert!(report["owners"][0]["bounds"].is_null());
    let shared = Arc::new(Mutex::new(session));
    let contents = Controller::new(shared.clone()).unwrap();
    let build = crate::compile_ui::Controller::new(&shared).unwrap();
    build.refresh(&lock(&shared));
    let count = "Cubemap samples: 2 · expanded before compile-region filtering";
    let reason = editor_core::session::instance_contents::CUBEMAP_PREVIEW_REASON;
    assert_eq!(report["cubemap_preview"]["reason"], reason);
    assert!(contents.window.get_summary().contains(count));
    assert!(contents.window.get_summary().contains(reason));
    assert!(build.window.get_details().contains(count));
    assert!(build.window.get_details().contains(reason));
    assert!(
        build
            .window
            .get_details()
            .lines()
            .any(|line| line == "Cubemap samples: 1")
    );
    assert!(!contents.window.get_can_frame());
    assert!(contents.window.get_owners().contains("child.vmf"));
    contents.window.invoke_frame_contents();
    assert!(
        contents
            .window
            .get_message()
            .contains("no visible geometry")
    );
    contents.window.show().unwrap();
    contents
        .window
        .window()
        .set_size(slint::PhysicalSize::new(850, 690));
    crate::ui::tests::save_ui_snapshot(contents.window.window(), "instance-contents-cubemaps");
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &memory);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.compile_settings(), settings);
    assert_eq!(serde_json::json!(session.property_draft()), draft);
    assert_eq!(std::fs::read(&parent).unwrap(), parent_bytes);
    assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
}

#[test]
fn software_visibility_helpers_share_counts_and_missing_geometry_with_build_details() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (_directory, game, parent, child) = crate::test_assets::instance_map(false);
    std::fs::write(
        &child,
        map_core::Document::empty().vmf().write(&Default::default()),
    )
    .unwrap();
    let mut session = Session::new(Ok(game));
    session.set_definitions(
        Actor::User,
        source_formats::fgd::Catalog::parse("@SolidClass color(180 180 0) = func_viscluster []")
            .unwrap(),
    );
    session.open(Actor::User, &child, true).unwrap();
    let brush = session
        .create_block(
            Actor::User,
            DVec3::new(-250.0, -250.0, 8.0),
            DVec3::new(250.0, 250.0, 248.0),
            "test/checker",
            false,
        )
        .unwrap();
    session
        .tie_to_entity(Actor::User, &[brush], "func_viscluster", BTreeMap::new())
        .unwrap();
    session.save(Actor::User, None).unwrap();
    session.open(Actor::User, &parent, false).unwrap();
    let memory = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let parent_bytes = std::fs::read(&parent).unwrap();
    let child_bytes = std::fs::read(&child).unwrap();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    assert!(
        result.as_ref().unwrap().output.bsp.is_some(),
        "{:?}",
        result.as_ref().unwrap().output.report
    );
    session.finish_compile(Actor::User, &result);
    let request = session
        .begin_instance_contents_refresh(Actor::User)
        .unwrap();
    let report = session
        .finish_instance_contents_refresh(Actor::User, request.run())
        .unwrap();
    assert_eq!(report["state"], "ready");
    assert_eq!(report["stats"]["visclusters"], 1);
    assert_eq!(report["available"], false);
    assert_eq!(report["geometry"]["vertices"], 0);
    assert!(report["owners"][0]["bounds"].is_null());
    let shared = Arc::new(Mutex::new(session));
    let contents = Controller::new(shared.clone()).unwrap();
    let build = crate::compile_ui::Controller::new(&shared).unwrap();
    build.refresh(&lock(&shared));
    let count = "Visibility helpers: 1 · expanded before compile-region filtering";
    let reason = editor_core::session::instance_contents::VISCLUSTER_PREVIEW_REASON;
    assert_eq!(report["viscluster_preview"]["reason"], reason);
    assert!(contents.window.get_summary().contains(count));
    assert!(contents.window.get_summary().contains(reason));
    assert!(build.window.get_details().contains(count));
    assert!(build.window.get_details().contains(reason));
    assert!(
        build
            .window
            .get_details()
            .contains("Authored visclusters: 1 helpers")
    );
    assert!(!contents.window.get_can_frame());
    assert!(contents.window.get_owners().contains("child.vmf"));
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
    assert_eq!(std::fs::read(&parent).unwrap(), parent_bytes);
    assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
}

#[test]
fn software_player_spawn_counts_share_missing_geometry_reason_with_build_details() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let fixture = crate::test_assets::instance_player_spawns::fixture(Actor::User, false);
    let mut session = fixture.session;
    session
        .begin_property_edit(session::PropertyTarget::World)
        .unwrap();
    let token = session.property_draft().unwrap().token;
    session
        .append_property_change(
            token,
            session::PropertyChange::Set {
                key: "message".into(),
                value: "Keep the spawn review".into(),
            },
        )
        .unwrap();
    let memory = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let settings = session.compile_settings();
    let draft = serde_json::json!(session.property_draft());
    let parent_bytes = std::fs::read(&fixture.source).unwrap();
    let child_bytes = std::fs::read(&fixture.child).unwrap();
    let output = fixture.directory.path().join("instances.bsp");
    let result = session
        .begin_compile(Actor::User, Some(&output))
        .unwrap()
        .run();
    assert!(
        result.as_ref().unwrap().output.bsp.is_some(),
        "{:?}",
        result.as_ref().unwrap().output.report
    );
    session.finish_compile(Actor::User, &result);
    assert_eq!(session.compile_status()["job"]["status"], "completed");
    assert_eq!(
        std::fs::read(&output).unwrap(),
        result
            .as_ref()
            .unwrap()
            .output
            .bsp
            .as_ref()
            .unwrap()
            .to_bytes()
    );
    let request = session
        .begin_instance_contents_refresh(Actor::User)
        .unwrap();
    session
        .finish_instance_contents_refresh(Actor::User, request.run())
        .unwrap();
    let report = session.inspect_instance_contents(0, 64).unwrap();
    assert_eq!(report["state"], "ready");
    assert_eq!(report["stats"]["player_spawns"], 2);
    assert_eq!(report["player_spawn_preview"]["available"], false);
    assert_eq!(report["available"], false);
    assert_eq!(report["geometry"]["vertices"], 0);
    assert!(report["owners"][0]["bounds"].is_null());
    let shared = Arc::new(Mutex::new(session));
    let contents = Controller::new(shared.clone()).unwrap();
    let build = crate::compile_ui::Controller::new(&shared).unwrap();
    build.refresh(&lock(&shared));
    let count = "Expanded player spawns: 2 · before compile-region filtering";
    let reason = editor_core::session::instance_contents::PLAYER_SPAWN_PREVIEW_REASON;
    assert_eq!(report["player_spawn_preview"]["reason"], reason);
    assert!(contents.window.get_summary().contains(count));
    assert!(contents.window.get_summary().contains(reason));
    assert!(build.window.get_details().contains(count));
    assert!(build.window.get_details().contains(reason));
    assert!(!contents.window.get_can_frame());
    assert!(contents.window.get_owners().contains("child.vmf"));
    assert!(!contents.window.get_owners().contains("Content bounds"));
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &memory);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.compile_settings(), settings);
    assert_eq!(serde_json::json!(session.property_draft()), draft);
    assert_eq!(std::fs::read(&fixture.source).unwrap(), parent_bytes);
    assert_eq!(std::fs::read(&fixture.child).unwrap(), child_bytes);
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
fn software_fgd_child_static_props_share_counts_and_explicit_unavailable_preview() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game, parent, child) = crate::test_assets::instance_static_props::fixture();
    let mut session = Session::new(Ok(game));
    session.set_definitions(
        Actor::User,
        source_formats::fgd::Catalog::parse(
            r#"
        @PointClass = prop_static [
            angles(angle) : "Angles" : "0 0 0"
            model(studio) : "World Model" : ""
            solid(choices) : "Collisions" : 6 = [ 0 : "None" 6 : "VPhysics" ]
            skin(integer) : "Skin" : 0
            body(integer) : "Body" : 0
            modelscale(float) : "Scale" : "1"
            rendercolor(color255) : "Color" : "255 255 255"
            renderamt(integer) : "Opacity" : 255
            rendermode(integer) : "Render mode" : 0
            renderfx(integer) : "Render effect" : 0
            fademindist(float) : "Start fade" : "-1"
            fademaxdist(float) : "End fade" : "0"
            fadescale(float) : "Fade scale" : "1"
            targetname(string) : "Name" : ""
            lightingorigin(string) : "Lighting origin" : ""
            spawnflags(flags) = [ 1 : "Unused" : 0 ]
        ]
    "#,
        )
        .unwrap(),
    );
    session.open(Actor::User, &child, true).unwrap();
    let prop = session
        .create_entity(
            Actor::User,
            "prop_static",
            Some(DVec3::new(96.0, 32.0, 8.0)),
            BTreeMap::from([
                (
                    "model".into(),
                    crate::test_assets::instance_static_props::MODEL.into(),
                ),
                ("solid".into(), "0".into()),
            ]),
        )
        .unwrap();
    session.save(Actor::User, None).unwrap();
    session.open(Actor::User, &child, false).unwrap();
    let ObjectId::Entity(prop) = prop else {
        unreachable!()
    };
    let entity = session.document().entity(prop).unwrap();
    for (key, expected) in [
        ("solid", "0"),
        ("body", "0"),
        ("skin", "0"),
        ("rendercolor", "255 255 255"),
        ("targetname", ""),
    ] {
        assert_eq!(entity.properties.get(key), Some(expected));
    }
    session.open(Actor::User, &parent, false).unwrap();
    session
        .begin_property_edit(session::PropertyTarget::World)
        .unwrap();
    let token = session.property_draft().unwrap().token;
    session
        .append_property_change(
            token,
            session::PropertyChange::Set {
                key: "message".into(),
                value: "Retain the child-model review".into(),
            },
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let settings = session.compile_settings();
    let draft = serde_json::json!(session.property_draft());
    let parent_bytes = std::fs::read(&parent).unwrap();
    let child_bytes = std::fs::read(&child).unwrap();
    let output = directory.path().join("outputs/instance-props.bsp");
    let result = session
        .begin_compile(Actor::User, Some(&output))
        .unwrap()
        .run();
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
    assert_eq!(report["stats"]["static_props"], 2);
    assert_eq!(report["static_prop_preview"]["available"], false);
    assert_eq!(report["available"], false);
    assert_eq!(report["geometry"]["vertices"], 0);
    assert!(report["owners"][0]["bounds"].is_null());
    let shared = Arc::new(Mutex::new(session));
    let contents = Controller::new(shared.clone()).unwrap();
    let build = crate::compile_ui::Controller::new(&shared).unwrap();
    build.refresh(&lock(&shared));
    let counts = "Expanded static props: 2 · before compile-region filtering";
    let reason = editor_core::session::instance_contents::STATIC_PROP_PREVIEW_REASON;
    assert!(contents.window.get_summary().contains(counts));
    assert!(contents.window.get_summary().contains(reason));
    assert!(build.window.get_details().contains(counts));
    assert!(build.window.get_details().contains(reason));
    assert!(!contents.window.get_can_frame());
    assert!(contents.window.get_owners().contains("child.vmf"));
    assert!(!contents.window.get_owners().contains("Content bounds"));
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
fn software_lighting_origin_helpers_share_counts_and_unavailable_geometry_with_build_details() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let fixture = crate::test_assets::instance_lighting_origins::fixture(Actor::User);
    let mut child =
        source_formats::vmf::Vmf::parse(&std::fs::read_to_string(&fixture.child).unwrap()).unwrap();
    child
        .entities
        .retain(|entity| entity.classname() == Some("info_lighting"));
    std::fs::write(&fixture.child, child.write(&Default::default())).unwrap();
    let mut session = fixture.session;
    let memory = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let settings = session.compile_settings();
    let parent_bytes = std::fs::read(&fixture.source).unwrap();
    let child_bytes = std::fs::read(&fixture.child).unwrap();
    let output = fixture.directory.path().join("outputs/origins.bsp");
    let result = session
        .begin_compile(Actor::User, Some(&output))
        .unwrap()
        .run();
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
    assert_eq!(report["state"], "ready");
    assert_eq!(report["stats"]["lighting_origins"], 1);
    assert_eq!(report["lighting_origin_preview"]["available"], false);
    assert_eq!(report["available"], false);
    assert_eq!(report["geometry"]["vertices"], 0);
    assert!(report["owners"][0]["bounds"].is_null());
    let shared = Arc::new(Mutex::new(session));
    let contents = Controller::new(shared.clone()).unwrap();
    let build = crate::compile_ui::Controller::new(&shared).unwrap();
    build.refresh(&lock(&shared));
    let counts = "Expanded lighting origins: 1 · before compile-region filtering";
    let reason = editor_core::session::instance_contents::LIGHTING_ORIGIN_PREVIEW_REASON;
    assert_eq!(report["lighting_origin_preview"]["reason"], reason);
    assert!(contents.window.get_summary().contains(counts));
    assert!(contents.window.get_summary().contains(reason));
    assert!(build.window.get_details().contains(counts));
    assert!(build.window.get_details().contains(reason));
    assert!(!contents.window.get_can_frame());
    assert!(contents.window.get_owners().contains("child.vmf"));
    assert!(!contents.window.get_owners().contains("Content bounds"));
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &memory);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.compile_settings(), settings);
    assert_eq!(std::fs::read(&fixture.source).unwrap(), parent_bytes);
    assert_eq!(std::fs::read(&fixture.child).unwrap(), child_bytes);
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
fn software_fgd_static_child_lights_share_counts_without_preview_substitution() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = crate::test_assets::game();
    let child = directory.path().join("child-lights.vmf");
    let parent = directory.path().join("instance-lights.vmf");
    let mut session = Session::new(Ok(game));
    session.set_definitions(
        Actor::User,
        source_formats::fgd::Catalog::parse(
            r#"
        @BaseClass = StaticLight [
            angles(angle) : "Angles" : "0 0 0"
            _light(color255) : "Light" : "255 192 128 64"
            _lightHDR(color255) : "HDR light" : "128 192 255 96"
            _constant_attn(float) : "Constant attenuation" : "1"
            _linear_attn(float) : "Linear attenuation" : "0"
            _quadratic_attn(float) : "Quadratic attenuation" : "0"
            style(integer) : "Style" : "0"
            defaultstyle(integer) : "Default style" : "0"
            targetname(string) : "Name" : ""
            target(string) : "Target" : ""
            spawnflags(flags) = [ 1 : "Initially dark" : 0 ]
        ]
        @PointClass base(StaticLight) = light [ ]
        @PointClass base(StaticLight) = light_spot [
            pitch(float) : "Pitch" : "-90"
            _inner_cone(float) : "Inner cone" : "20"
            _cone(float) : "Outer cone" : "45"
            _exponent(float) : "Exponent" : "2"
        ]
    "#,
        )
        .unwrap(),
    );
    let point = session
        .create_entity(
            Actor::User,
            "light",
            Some(DVec3::new(-48.0, 16.0, 128.0)),
            BTreeMap::new(),
        )
        .unwrap();
    let spot = session
        .create_entity(
            Actor::User,
            "light_spot",
            Some(DVec3::new(64.0, -16.0, 160.0)),
            BTreeMap::from([("pitch".into(), "-45".into())]),
        )
        .unwrap();
    session.save(Actor::User, Some(&child)).unwrap();
    session.open(Actor::User, &child, false).unwrap();
    for id in [point, spot] {
        let ObjectId::Entity(id) = id else {
            unreachable!()
        };
        let entity = session.document().entity(id).unwrap();
        assert_eq!(entity.properties.get("angles"), Some("0 0 0"));
        assert_eq!(entity.properties.get("style"), Some("0"));
        assert_eq!(entity.properties.get("defaultstyle"), Some("0"));
        assert_eq!(entity.properties.get("spawnflags"), Some("0"));
        assert_eq!(entity.properties.get("targetname"), Some(""));
    }
    let ObjectId::Entity(spot) = spot else {
        unreachable!()
    };
    assert_eq!(
        session
            .document()
            .entity(spot)
            .unwrap()
            .properties
            .get("pitch"),
        Some("-45")
    );
    let child_bytes = std::fs::read(&child).unwrap();
    let mut vmf = crate::test_assets::room();
    for solid in &mut vmf.world.solids {
        for side in &mut solid.sides {
            side.material = "test/checker".into();
        }
    }
    std::fs::write(&parent, vmf.write(&Default::default())).unwrap();
    session.open(Actor::User, &parent, false).unwrap();
    session
        .create_entity(
            Actor::User,
            "func_instance",
            Some(DVec3::new(16.0, -8.0, 4.0)),
            BTreeMap::from([
                ("file".into(), "child-lights.vmf".into()),
                ("angles".into(), "0 0 0".into()),
            ]),
        )
        .unwrap();
    session.save(Actor::User, None).unwrap();
    session
        .update_compile_settings(Actor::User, Some("direct_ldr"), Some("none"))
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
                value: "Keep the static light review".into(),
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
    assert_eq!(report["state"], "ready");
    assert_eq!(report["available"], false);
    assert_eq!(report["stats"]["point_lights"], 1);
    assert_eq!(report["static_light_preview"]["available"], false);
    assert_eq!(report["stats"]["spot_lights"], 1);
    assert_eq!(report["stats"]["world_brushes"], 0);
    assert_eq!(report["stats"]["detail_brushes"], 0);
    assert_eq!(report["geometry"]["triangles"], 0);
    assert_eq!(report["face_provenance_total"], 0);
    let shared = Arc::new(Mutex::new(session));
    let contents = Controller::new(shared.clone()).unwrap();
    let build = crate::compile_ui::Controller::new(&shared).unwrap();
    build.refresh(&lock(&shared));
    let counts = "Expanded static lights: 1 point · 1 spot · before compile-region filtering";
    let limit = "Child lights, illumination and icons are not shown in the reference or authored-lighting preview.";
    assert!(build.window.get_details().contains(counts));
    assert!(build.window.get_details().contains(limit));
    assert!(contents.window.get_summary().contains(counts));
    assert!(contents.window.get_summary().contains(limit));
    assert!(!contents.window.get_can_frame());
    assert!(contents.window.get_owners().contains("child-lights.vmf"));
    assert!(!contents.window.get_owners().contains("Terrain:"));
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
    crate::ui::tests::save_ui_snapshot(build.window.window(), "build-instance-lights");
    contents.window.show().unwrap();
    contents
        .window
        .window()
        .set_size(slint::PhysicalSize::new(850, 690));
    crate::ui::tests::save_ui_snapshot(contents.window.window(), "instance-contents-lights");
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
