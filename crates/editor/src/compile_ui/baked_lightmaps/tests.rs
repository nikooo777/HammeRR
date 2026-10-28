use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;
use crate::compile_ui::Controller;

fn baked_surface_workflow(controller: &Controller, shared: &SharedSession) {
    controller.window.invoke_inspect_baked_surfaces();
    let window = controller.baked_surfaces.window();
    assert!(window.window().is_visible());
    window.invoke_prepare_surfaces();
    let started = Instant::now();
    loop {
        let report = lock(shared).inspect_baked_surfaces(None, 0, 1).unwrap();
        if report["worker_active"] == false {
            assert!(report["page"]["total"].as_u64().unwrap() > 0);
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "Surface preparation did not drain"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    controller.refresh_visible(&lock(shared));
    window.invoke_refresh_inspection();
    let report = lock(shared).inspect_baked_surfaces(None, 0, 64).unwrap();
    let style = report["lighting"]["available_styles"]
        .as_array()
        .unwrap()
        .iter()
        .position(|style| style == 0)
        .unwrap();
    window.invoke_choose_style(style as i32);
    window.set_style_gain(0.5);
    window.set_exposure_ev(0.5);
    window.invoke_apply_lighting();
    let options = lock(shared).baked_surfaces_options_report();
    assert_eq!(options["exposure_ev"], 0.5);
    assert_eq!(
        options["style_gains"],
        serde_json::json!([{"style":0,"gain":0.5}])
    );
    window.set_channel_index(99);
    window.set_exposure_ev(2.0);
    window.invoke_apply_lighting();
    assert_eq!(window.get_channel_index(), 0);
    assert_eq!(window.get_exposure_ev(), 0.5);
    assert_eq!(lock(shared).baked_surfaces_options_report(), options);
    window.invoke_show_surfaces();
    assert_eq!(
        lock(shared).inspect_baked_surfaces(None, 0, 1).unwrap()["displayed"],
        true
    );
    window.invoke_hide_surfaces();
    assert_eq!(lock(shared).baked_surfaces_options_report(), options);
    assert_eq!(
        lock(shared).inspect_baked_surfaces(None, 0, 1).unwrap()["displayed"],
        false
    );
    let camera = lock(shared).camera();
    window.set_record_index(1);
    window.invoke_choose_record_kind(1);
    assert!(!window.get_can_frame());
    assert_eq!(lock(shared).camera(), camera);
    assert_eq!(lock(shared).baked_surfaces_options_report(), options);
    window.set_record_index(0);
    window.invoke_choose_record_kind(0);
    window.invoke_choose_face(0);
    assert!(window.get_can_frame());
    window.invoke_frame_face();
    assert_ne!(lock(shared).camera(), camera);
    assert_eq!(lock(shared).camera().projection, camera.projection);
    lock(shared).set_camera(camera);
    window.invoke_clear_surfaces();
    assert!(!window.get_has_token());
    assert_eq!(window.get_exposure_ev(), 0.0);
    window.set_compiled_static_props(true);
    window.set_draw_lod(1);
    window.invoke_prepare_surfaces();
    let started = Instant::now();
    loop {
        let report = lock(shared).inspect_baked_surfaces(None, 0, 1).unwrap();
        if report["worker_active"] == false {
            assert_eq!(report["status"], "unavailable", "{report}");
            assert_eq!(report["complete"], false);
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(5));
    }
    controller.refresh_visible(&lock(shared));
    window.invoke_hide_surfaces();
    assert!(window.get_compiled_static_props());
    assert_eq!(window.get_draw_lod(), 1);
    window.invoke_clear_surfaces();
    assert!(!window.get_compiled_static_props());
    assert_eq!(window.get_draw_lod(), 0);
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
}

#[test]
fn baked_lightmap_controls_inspect_successful_build_and_clear_changed_output() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = crate::test_assets::game();
    let maps = directory.path().join("maps");
    std::fs::create_dir(&maps).unwrap();
    let mut vmf = crate::test_assets::room();
    for solid in &mut vmf.world.solids {
        for side in &mut solid.sides {
            side.material = "test/checker".into();
        }
    }
    let mut child = map_core::Document::empty();
    child
        .edit("external contributor", |tx| {
            map_core::edit::create_block(
                tx,
                glam::DVec3::new(0.0, 0.0, 16.0),
                glam::DVec3::new(16.0, 16.0, 32.0),
                "test/checker",
            )
        })
        .unwrap();
    std::fs::write(
        maps.join("child.vmf"),
        child.vmf().write(&Default::default()),
    )
    .unwrap();
    let mut helper = source_formats::vmf::Entity::new(9100, "func_instance");
    helper.properties.set("file", "child.vmf");
    vmf.entities.push(helper);
    let mut light = source_formats::vmf::Entity::new(9000, "light");
    light.properties.set("origin", "-160 -100 96");
    light.properties.set("_light", "255 160 60 600");
    light.properties.set("_lightHDR", "80 160 255 1200");
    vmf.entities.push(light);
    let source = maps.join("lightmaps.vmf");
    let output = maps.join("lightmaps.bsp");
    std::fs::write(&source, vmf.write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session
        .open(super::super::Actor::User, &source, false)
        .unwrap();
    session
        .update_compile_settings(
            super::super::Actor::User,
            Some("direct_ldr_hdr"),
            Some("none"),
        )
        .unwrap();
    let request = session
        .begin_compile(super::super::Actor::User, Some(&output))
        .unwrap();
    let result = request.run();
    session.finish_compile(super::super::Actor::User, &result);
    assert_eq!(session.compile_status()["job"]["status"], "completed");
    let before = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    controller.window.invoke_inspect_baked_lightmaps();
    let window = &controller.baked_lightmaps.window;
    assert!(window.window().is_visible());
    assert!(window.get_has_preview());
    assert!(window.get_build_state().contains("current"));
    assert!(window.get_selected().contains("Linear RGB intensity"));
    window.set_channel("HDR".into());
    window.invoke_inspect_face();
    assert!(window.get_has_preview());
    assert!(window.get_selected().contains("HDR"));
    assert!(window.get_can_frame());
    controller.set_aspect(1.7);
    assert_eq!(window.get_aspect(), 1.7);
    baked_surface_workflow(&controller, &shared);
    window.invoke_frame_face();
    assert!(
        window
            .get_diagnostics()
            .contains("Framed compiled HDR face")
    );
    assert_ne!(lock(&shared).camera(), camera);
    assert_eq!(lock(&shared).camera().projection, camera.projection);
    lock(&shared).set_camera(camera);
    assert!(window.get_mapping_status().contains("available"));
    assert!(!controller.baked_lightmaps.lists.borrow().origins.is_empty());
    assert_eq!(window.get_page_size(), 32);
    window.set_page_size(1);
    window.invoke_query_changed();
    assert!(!window.get_can_frame());
    window.invoke_frame_face();
    assert!(window.get_diagnostics().contains("Inspect face"));
    assert_eq!(lock(&shared).camera(), camera);
    let single_query = query(window, false);
    assert_eq!(single_query.limit, 1);
    let single = lock(&shared)
        .inspect_baked_lightmaps(&single_query)
        .unwrap();
    assert_eq!(single.report["mapping"]["contributor_page"]["limit"], 1);
    window.invoke_inspect_face();
    let expected_origins = single.report["mapping"]["contributors"]
        .as_array()
        .unwrap()
        .iter()
        .map(source_reference)
        .collect::<Vec<_>>();
    assert_eq!(
        controller.baked_lightmaps.lists.borrow().origins,
        expected_origins
    );
    assert_eq!(expected_origins.len(), 1);
    window.set_page_size(32);
    window.invoke_query_changed();
    window.invoke_inspect_face();
    let original_selection = lock(&shared).selected_faces();
    window.invoke_inspect_origin(0);
    assert!(window.get_has_source());
    assert!(!controller.baked_lightmaps.lists.borrow().matches.is_empty());
    window.invoke_inspect_match(0);
    assert!(window.get_has_preview());
    assert_eq!(lock(&shared).selected_faces(), original_selection);
    let original = &child.vmf().world.solids[0];
    let external = lock(&shared)
        .inspect_baked_lightmaps(&Query {
            channel: Channel::Hdr,
            source: Some(SourceReference::Instance {
                instance: 9100,
                file: "child.vmf".into(),
                solid: original.id,
                side: original.sides[0].id,
                occurrence: None,
            }),
            ..Query::default()
        })
        .unwrap();
    window.set_compiled_face(external.report["mapping"]["face"].as_i64().unwrap() as i32);
    window.invoke_inspect_face();
    let index = controller
        .baked_lightmaps
        .lists
        .borrow()
        .origins
        .iter()
        .position(|origin| matches!(origin, Some(SourceReference::Instance { .. })))
        .unwrap();
    window.invoke_inspect_origin(index as i32);
    assert!(matches!(
        controller.baked_lightmaps.lists.borrow().source,
        Some(SourceReference::Instance {
            occurrence: Some(_),
            ..
        })
    ));
    assert!(
        window
            .get_origin_details()
            .contains("entity:9100 → child.vmf")
    );
    assert_eq!(lock(&shared).selected_faces(), original_selection);
    crate::ui::tests::save_ui_snapshot(window.window(), "baked-lightmaps");
    window.set_style_index(3);
    window.invoke_query_changed();
    assert!(!window.get_has_preview());
    window.invoke_inspect_face();
    assert!(!window.get_has_preview());
    assert!(window.get_diagnostics().contains("absent"));
    window.set_style_index(0);
    window.invoke_inspect_face();
    assert!(window.get_has_preview());
    let inspection = lock(&shared)
        .inspect_baked_lightmaps(&Query {
            channel: Channel::Hdr,
            ..Query::default()
        })
        .unwrap();
    assert_eq!(inspection.report["build"]["status"], "current");
    std::fs::write(&output, b"external replacement").unwrap();
    present(
        window,
        inspection,
        &controller.baked_lightmaps.baseline,
        &controller.baked_lightmaps.lists,
    );
    assert!(window.get_has_preview());
    assert!(window.get_build_state().contains("current"));
    controller.refresh(&lock(&shared));
    assert!(!window.get_has_preview());
    assert!(window.get_build_state().contains("unavailable"));
    assert!(!window.get_can_frame());
    window.invoke_frame_face();
    assert_eq!(lock(&shared).camera(), camera);
    window.invoke_inspect_face();
    assert!(!window.get_has_preview());
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &before);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.camera(), camera);
}
