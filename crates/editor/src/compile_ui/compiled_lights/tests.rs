use std::sync::{Arc, Mutex};

use super::*;
use crate::compile_ui::Controller;
use crate::session::{Actor, PropertyChange, PropertyTarget};

fn ordinals(window: &CompiledLightsWindow) -> Vec<String> {
    window
        .get_light_rows()
        .iter()
        .map(|row| row.row_data(0).unwrap().text.to_string())
        .collect()
}

#[test]
fn compiled_lights_controls_share_pages_channels_details_and_owned_build_invalidation() {
    let extremes = [f32::from_bits(1), f32::MAX, -0.0];
    let formatted = vector(extremes);
    assert!(formatted.len() < 64);
    for (actual, expected) in formatted.split(", ").zip(extremes) {
        assert_eq!(actual.parse::<f32>().unwrap().to_bits(), expected.to_bits());
    }
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (_directory, mut session, source) =
        editor_core::session::compiled_world_lights::test_support::compiled(true);
    let output = source.with_extension("bsp");
    let pointfile = output.with_extension("lin");
    std::fs::write(&pointfile, b"retained pointfile\n").unwrap();
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let token = session.property_draft().unwrap().token;
    session
        .append_property_change(
            token,
            PropertyChange::Set {
                key: "skyname".into(),
                value: "unapplied_inspection_review".into(),
            },
        )
        .unwrap();
    let before = (
        session.document().vmf().clone(),
        session.history_snapshot(),
        session.camera(),
        session.selected_ids(),
        session.compile_settings(),
        session.build_recipes()["recipes"].clone(),
        serde_json::json!(session.property_draft()),
        std::fs::read(&source).unwrap(),
        std::fs::read(&output).unwrap(),
        std::fs::read(&pointfile).unwrap(),
    );
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    let form = &controller.compiled_lights;
    let window = &form.window;
    form.state.borrow_mut().query.limit = 2;
    controller.window.invoke_inspect_compiled_lights();
    assert!(window.window().is_visible());
    assert!(window.get_build_state().contains("current"));
    assert!(window.get_build_state().contains("Direct LDR + HDR"));
    assert_eq!(ordinals(window), ["0", "1"]);
    assert_eq!(
        window
            .get_light_columns()
            .iter()
            .map(|column| column.title.to_string())
            .collect::<Vec<_>>(),
        ["Record", "Kind", "Style", "Origin", "Intensity", "Flags"]
    );
    assert!(window.get_next_lights());
    window.invoke_inspect_light(1);
    assert!(window.get_has_selection());
    assert_eq!(window.get_details().record, "1");
    assert_eq!(window.get_details().kind, "spot (2)");
    assert_eq!(window.get_details().style, "7");
    assert_eq!(window.get_details().intensity, "0, 0, 1");
    assert!(window.get_can_frame());
    let camera = lock(&shared).camera();
    for aspect in [0.5, 2.5] {
        controller.set_aspect(aspect);
        let origin = glam::Vec3::new(64.0, 32.0, 192.0);
        let expected = lock(&shared).camera().frame_bounds(
            origin - glam::Vec3::splat(16.0),
            origin + glam::Vec3::splat(16.0),
            aspect,
        );
        window.invoke_frame_light();
        assert_eq!(lock(&shared).camera(), expected);
        assert_eq!(window.get_details().record, "1");
        assert!(window.get_diagnostics().contains("Framed LDR record 1"));
        assert_eq!(lock(&shared).document().vmf(), &before.0);
        assert_eq!(serde_json::json!(lock(&shared).property_draft()), before.6);
    }
    assert_ne!(lock(&shared).camera(), camera);
    lock(&shared).set_camera(camera);
    let ldr = window.get_details().intensity;
    window.invoke_page_lights(1);
    assert_eq!(ordinals(window), ["2", "3"]);
    assert!(!window.get_has_selection());
    assert!(!window.get_next_lights());
    assert!(window.get_previous_lights());
    window.invoke_inspect_light(0);
    assert!(!window.get_can_frame());
    window.invoke_page_lights(-1);
    assert_eq!(ordinals(window), ["0", "1"]);
    window.set_channel("HDR".into());
    window.invoke_change_channel();
    window.invoke_inspect_light(1);
    assert_ne!(window.get_details().intensity, ldr);
    assert_eq!(window.get_details().intensity, "3, 0, 3");
    {
        let state = form.state.borrow();
        let expected = lock(&shared)
            .inspect_compiled_world_lights(&state.query)
            .unwrap();
        assert_eq!(
            serde_json::to_value(state.report.as_ref().unwrap()).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }
    let columns = window.get_light_columns();
    let mut column = columns.row_data(0).unwrap();
    column.sort_order = slint::language::SortOrder::Descending;
    columns.set_row_data(0, column);
    window.invoke_reset_sort();
    assert!(
        columns
            .iter()
            .all(|column| column.sort_order == slint::language::SortOrder::Unsorted)
    );
    assert_eq!(ordinals(window), ["0", "1"]);
    form.state.borrow_mut().query.limit = 64;
    window.invoke_refresh_inspection();
    window.invoke_inspect_light(1);
    assert!(window.get_details().normal.len() < 64);
    crate::ui::tests::save_ui_snapshot(window.window(), "compiled-lights");
    window
        .window()
        .set_size(slint::LogicalSize::new(880.0, 720.0));
    crate::ui::tests::save_ui_snapshot(window.window(), "compiled-lights-compact");
    assert!(!controller.window.window().is_visible());
    controller
        .window
        .set_summary("Hidden build status sentinel".into());
    controller
        .window
        .set_launch_details("Hidden launch status sentinel".into());
    controller
        .window
        .set_diagnostics("Hidden diagnostic sentinel".into());
    lock(&shared)
        .update_build_settings(
            Actor::User,
            crate::session::compilation::BuildSettingsUpdate {
                bounces: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    controller.refresh_visible(&lock(&shared));
    assert_eq!(
        controller.window.get_summary(),
        "Hidden build status sentinel"
    );
    assert_eq!(
        controller.window.get_launch_details(),
        "Hidden launch status sentinel"
    );
    assert_eq!(
        controller.window.get_diagnostics(),
        "Hidden diagnostic sentinel"
    );
    assert!(ordinals(window).is_empty());
    assert!(!window.get_has_selection());
    assert!(!window.get_can_frame());
    assert_eq!(window.get_details().record, "");
    assert!(!window.get_next_lights());
    controller.window.show().unwrap();
    controller.refresh_visible(&lock(&shared));
    assert_ne!(
        controller.window.get_summary(),
        "Hidden build status sentinel"
    );
    controller.window.hide().unwrap();
    window.invoke_inspect_light(1);
    assert!(!window.get_has_selection());
    window.invoke_refresh_inspection();
    assert!(window.get_build_state().contains("stale"));
    assert_eq!(ordinals(window), ["0", "1", "2", "3"]);
    lock(&shared)
        .update_build_settings(
            Actor::User,
            crate::session::compilation::BuildSettingsUpdate {
                bounces: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
    window.invoke_refresh_inspection();
    window.invoke_inspect_light(1);
    assert!(window.get_can_frame());
    let camera = lock(&shared).camera();
    std::fs::write(&output, b"changed owned output").unwrap();
    window.invoke_frame_light();
    assert_eq!(lock(&shared).camera(), camera);
    assert!(!window.get_can_frame());
    assert!(ordinals(window).is_empty());
    window.invoke_change_channel();
    assert!(ordinals(window).is_empty());
    assert!(!window.get_has_selection());
    window.invoke_refresh_inspection();
    assert!(window.get_build_state().contains("unavailable"));
    assert!(window.get_light_page().contains("No available"));
    assert!(!window.get_previous_lights());
    assert!(!window.get_next_lights());
    assert_eq!(window.get_current_row(), -1);
    std::fs::write(&output, &before.8).unwrap();
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
    controller.window.invoke_inspect_compiled_lights();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: slint::platform::Key::Escape.into(),
        });
    assert!(!window.window().is_visible());
    let session = lock(&shared);
    assert_eq!(
        (
            session.document().vmf().clone(),
            session.history_snapshot(),
            session.camera(),
            session.selected_ids(),
            session.compile_settings(),
            session.build_recipes()["recipes"].clone(),
            serde_json::json!(session.property_draft()),
            std::fs::read(&source).unwrap(),
            std::fs::read(&output).unwrap(),
            std::fs::read(&pointfile).unwrap(),
        ),
        before
    );
}
