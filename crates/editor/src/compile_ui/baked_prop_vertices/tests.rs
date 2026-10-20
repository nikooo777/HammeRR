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
    assert!(!window.get_diagnostics().is_empty());
    window.invoke_inspect_prop(1);
    assert_eq!(window.get_vertex_rows().row_count(), 256);
    crate::ui::tests::save_ui_snapshot(window.window(), "baked-prop-vertices");
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
    assert_eq!(window.get_vertex_rows().row_count(), 0);
    window.invoke_inspect_section(1);
    assert_eq!(window.get_vertex_rows().row_count(), 0);
    window.invoke_refresh_inspection();
    assert!(window.get_build_state().contains("stale"));
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
