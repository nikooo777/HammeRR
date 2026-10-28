use std::sync::{Arc, Mutex};

use super::*;
use crate::compile_ui::Controller;
use crate::session::{Actor, PropertyChange, PropertyTarget};
use slint::Model;

#[test]
fn baked_prop_vertices_controls_inspect_real_pages_channels_disabled_props_and_stale_output() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game, vmf) = editor_core::session::baked_prop_vertices::test_support::fixture();
    let source = directory.path().join("maps/inspection.vmf");
    let output = directory.path().join("maps/inspection.bsp");
    std::fs::write(&source, vmf.write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &source, false).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    controller.window.invoke_inspect_baked_prop_vertices();
    let window = &controller.baked_prop_vertices.window;
    assert!(window.window().is_visible());
    assert!(window.get_build_state().contains("unavailable"));
    assert_eq!(window.get_vertex_rows().row_count(), 0);
    let request = {
        let mut session = lock(&shared);
        session
            .update_build_settings(
                Actor::User,
                crate::session::compilation::BuildSettingsUpdate {
                    lighting: Some("direct_ldr_hdr"),
                    bake_static_prop_vertices: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        session.begin_compile(Actor::User, Some(&output)).unwrap()
    };
    let result = request.run();
    lock(&shared).finish_compile(Actor::User, &result);
    assert_eq!(lock(&shared).compile_status()["job"]["status"], "completed");
    {
        let mut session = lock(&shared);
        session.begin_property_edit(PropertyTarget::World).unwrap();
        let token = session.property_draft().unwrap().token;
        session
            .append_property_change(
                token,
                PropertyChange::Set {
                    key: "skyname".into(),
                    value: "pending_sky".into(),
                },
            )
            .unwrap();
    }
    let before = {
        let session = lock(&shared);
        (
            session.document().vmf().clone(),
            session.history_snapshot(),
            session.camera(),
            session.selected_ids(),
            session.build_recipes()["recipes"].clone(),
            session.compile_settings(),
            serde_json::json!(session.property_draft()),
            std::fs::read(&source).unwrap(),
        )
    };
    window.invoke_refresh_inspection();
    assert_eq!(window.get_prop_rows().row_count(), 3);
    assert!(
        window
            .get_prop_rows()
            .row_data(0)
            .unwrap()
            .contains("entity:9100")
    );
    assert!(
        window
            .get_selected()
            .contains("Captured source: root entity:9101")
    );
    window.set_prop_limit(1);
    window.invoke_refresh_inspection();
    assert_eq!(window.get_prop_rows().row_count(), 1);
    window.invoke_page_props(1);
    assert_eq!(window.get_prop_rows().row_count(), 1);
    assert!(
        window
            .get_prop_rows()
            .row_data(0)
            .unwrap()
            .contains("entity:9101")
    );
    window.set_prop_limit(32);
    window.invoke_refresh_inspection();
    assert_eq!(window.get_prop_rows().row_count(), 3);
    assert_eq!(window.get_section_rows().row_count(), 2);
    assert_eq!(window.get_vertex_rows().row_count(), 256);
    assert!(window.get_next_vertices());
    window.invoke_page_vertices(1);
    assert_eq!(window.get_vertex_rows().row_count(), 44);
    assert!(
        window
            .get_vertex_rows()
            .row_data(0)
            .unwrap()
            .starts_with("256")
    );
    window.invoke_page_vertices(-1);
    assert_eq!(window.get_vertex_rows().row_count(), 256);
    window.set_channel("HDR".into());
    window.invoke_change_channel();
    assert!(window.get_selected().contains("hdr"));
    let state = controller.baked_prop_vertices.state.borrow();
    let expected = lock(&shared)
        .inspect_baked_prop_vertices(&state.query)
        .unwrap()
        .report;
    assert_eq!(state.report.as_ref(), Some(&expected));
    drop(state);
    window.invoke_inspect_section(1);
    assert_eq!(window.get_vertex_rows().row_count(), 6);
    assert!(window.get_selected().contains("LOD 1"));
    window.invoke_inspect_prop(0);
    assert_eq!(window.get_vertex_rows().row_count(), 0);
    controller.set_aspect(1.7);
    assert!(window.get_can_frame());
    assert!(window.get_can_select_source());
    assert!(window.get_can_show_leaf_bounds());
    assert!(!window.get_can_clear_leaf_bounds());
    window.invoke_show_leaf_bounds();
    assert!(window.get_leaf_bounds_state().contains("Prop 0"));
    assert!(
        window
            .get_leaf_bounds_state()
            .contains("native leaf membership bounds")
    );
    assert!(window.get_can_clear_leaf_bounds());
    let guide = lock(&shared).compiled_prop_leaf_bounds();
    assert_eq!(guide["prop"], 0);
    assert_eq!(guide["displayed"], true);
    assert!(guide["leaf_count"].as_u64().unwrap() > 0);
    window.invoke_clear_leaf_bounds();
    assert!(!window.get_can_clear_leaf_bounds());
    assert_eq!(
        lock(&shared).compiled_prop_leaf_bounds()["requested"],
        false
    );
    window.invoke_show_leaf_bounds();
    assert_eq!(window.get_source_selection_label(), "Select source");
    let camera = lock(&shared).camera();
    window.invoke_select_source();
    assert_eq!(
        lock(&shared).selected_ids(),
        [map_core::ObjectId::Entity(9100)]
    );
    assert!(!lock(&shared).face_mode());
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(serde_json::json!(lock(&shared).property_draft()), before.6);
    lock(&shared)
        .select(&before.3, crate::session::SelectMode::Replace)
        .unwrap();

    window.invoke_frame_prop_origin();
    assert!(window.get_diagnostics().contains("Framed native prop 0"));
    assert_ne!(lock(&shared).camera(), camera);
    let framed = lock(&shared).camera();
    let clip = framed.view_projection(1.7) * glam::Vec3::new(-64., 0., 32.).extend(1.);
    let ndc = clip.truncate() / clip.w;
    assert!(clip.w > 0. && ndc.x.abs() < 0.001 && ndc.y.abs() < 0.001);
    lock(&shared).set_camera(camera);
    assert!(
        window
            .get_selected()
            .contains("Captured source: root entity:9100")
    );
    assert!(!window.get_diagnostics().is_empty());
    window.invoke_inspect_prop(1);
    assert_eq!(window.get_vertex_rows().row_count(), 256);
    crate::ui::tests::assert_ui_renders(window.window(), "baked-prop-vertices");
    {
        let mut session = lock(&shared);
        session
            .update_build_settings(
                Actor::User,
                crate::session::compilation::BuildSettingsUpdate {
                    bounces: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    controller.refresh(&lock(&shared));
    assert!(window.get_leaf_bounds_state().contains("suppressed"));
    assert!(window.get_can_clear_leaf_bounds());
    assert!(!window.get_can_show_leaf_bounds());
    assert_eq!(
        lock(&shared).compiled_prop_leaf_bounds()["displayed"],
        false
    );
    assert_eq!(window.get_vertex_rows().row_count(), 0);
    window.invoke_inspect_section(1);
    assert_eq!(window.get_vertex_rows().row_count(), 0);
    window.invoke_refresh_inspection();
    assert!(window.get_build_state().contains("stale"));
    assert!(!window.get_can_frame());
    assert!(!window.get_can_select_source());
    assert!(!window.get_can_show_leaf_bounds());
    assert!(
        window
            .get_selected()
            .contains("Historical source: root entity:9101")
    );
    assert_eq!(window.get_vertex_rows().row_count(), 256);
    {
        let mut session = lock(&shared);
        session
            .update_build_settings(
                Actor::User,
                crate::session::compilation::BuildSettingsUpdate {
                    bounces: Some(0),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let original = std::fs::read(&output).unwrap();
    std::fs::write(&output, b"replacement").unwrap();
    controller.refresh(&lock(&shared));
    assert_eq!(window.get_vertex_rows().row_count(), 0);
    window.invoke_refresh_inspection();
    assert!(window.get_build_state().contains("unavailable"));
    assert_eq!(window.get_prop_rows().row_count(), 0);
    assert_eq!(window.get_section_rows().row_count(), 0);
    assert_eq!(window.get_vertex_rows().row_count(), 0);
    std::fs::write(&output, original).unwrap();
    window.invoke_clear_leaf_bounds();
    assert_eq!(
        lock(&shared).compiled_prop_leaf_bounds()["requested"],
        false
    );
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
    let session = lock(&shared);
    assert_eq!(
        (
            session.document().vmf().clone(),
            session.history_snapshot(),
            session.camera(),
            session.selected_ids(),
            session.build_recipes()["recipes"].clone(),
            session.compile_settings(),
            serde_json::json!(session.property_draft()),
            std::fs::read(&source).unwrap()
        ),
        before
    );
}
