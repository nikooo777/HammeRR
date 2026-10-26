use std::sync::{Arc, Mutex};

use slint::Model;

use super::*;
use crate::{
    compile_ui::Controller,
    session::{PropertyChange, PropertyTarget},
};

#[test]
fn visibility_controls_share_owned_navigation_frame_guide_and_invalidation() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (_directory, mut session, source) =
        editor_core::session::compiled_visibility::test_support::compiled();
    let mut camera = session.camera();
    camera.projection = render::Projection::Perspective;
    camera.position = glam::Vec3::new(0.0, 0.0, 16.0);
    session.set_camera(camera);
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let draft = session.property_draft().unwrap().token;
    session
        .append_property_change(
            draft,
            PropertyChange::Set {
                key: "skyname".into(),
                value: "pending_visibility_ui".into(),
            },
        )
        .unwrap();
    let before = (
        session.document().vmf().clone(),
        session.history_snapshot(),
        session.selected_ids(),
        serde_json::to_value(session.property_draft()).unwrap(),
        std::fs::read(source.with_extension("bsp")).unwrap(),
    );
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    controller.window.invoke_inspect_visibility();
    let form = &controller.compiled_visibility;
    let window = &form.window;
    assert!(window.window().is_visible());
    assert!(window.get_build_state().contains("current"));
    window.set_position_x("0".into());
    window.set_position_y("0".into());
    window.set_position_z("16".into());
    window.invoke_inspect_position();
    assert!(
        window
            .get_position_result()
            .contains("Open clustered space")
    );
    let explicit = form.state.borrow().report.as_ref().unwrap().target;
    window.invoke_inspect_camera_position();
    assert!(
        window
            .get_position_result()
            .contains("Current camera position")
    );
    assert_eq!(
        form.state.borrow().report.as_ref().unwrap().target,
        explicit
    );
    window.set_position_x("128".into());
    window.invoke_inspect_position();
    assert!(
        window
            .get_position_result()
            .contains("Solid/exterior; no PVS/PAS source row")
    );
    window.invoke_refresh_inspection();
    assert!(window.get_position_result().is_empty());
    let target = form
        .state
        .borrow()
        .report
        .as_ref()
        .unwrap()
        .page
        .rows
        .iter()
        .find(|row| matches!(row, Row::Leaf(leaf) if leaf.cluster >= 0))
        .unwrap()
        .target();
    form.state.borrow_mut().query.limit = 2;
    window.invoke_inspect_target();
    assert_eq!(window.get_rows().row_count(), 2);
    assert!(window.get_next_rows());
    window.invoke_page_rows(1);
    assert_eq!(form.state.borrow().report.as_ref().unwrap().page.offset, 2);
    window.invoke_page_rows(-1);
    let Target::Leaf { ordinal } = target else {
        panic!();
    };
    window.set_target_kind("Leaf".into());
    window.set_ordinal(ordinal.to_string().into());
    window.invoke_inspect_target();
    assert!(window.get_can_frame());
    assert!(window.get_visibility_rows().row_count() > 0);
    window.set_visibility_kind("PAS".into());
    window.invoke_inspect_target();
    assert_eq!(form.state.borrow().report.as_ref().unwrap().kind, Kind::Pas);
    assert!(window.get_rows().row_count() > 0);
    assert!(window.get_can_show_destinations());
    for (kind, name) in [(Kind::Pvs, "PVS"), (Kind::Pas, "PAS")] {
        if kind == Kind::Pvs {
            window.invoke_show_pvs_bounds();
        } else {
            window.invoke_show_pas_bounds();
        }
        let guide = lock(&shared).compiled_visibility_overlay();
        assert_eq!(
            guide["mode"],
            serde_json::json!(GuideMode::Destinations { visibility: kind })
        );
        assert_eq!(
            guide["leaf_count"].as_u64().unwrap() as usize,
            guide["native_leaves"].as_array().unwrap().len()
        );
        assert!(
            window
                .get_guide_state()
                .contains(&format!("{name} potential destination bounds"))
        );
        let mut session = lock(&shared);
        assert_eq!(
            session
                .build_scene()
                .lines
                .iter()
                .filter(|line| line.color == [70, 220, 255, 255])
                .count(),
            guide["leaf_count"].as_u64().unwrap() as usize * 24
        );
    }
    window.invoke_inspect_row(0);
    assert!(matches!(
        form.state.borrow().report.as_ref().unwrap().target,
        Target::Portal { .. }
    ));
    assert!(window.get_details().contains("portal_vertices"));
    assert!(!window.get_can_show_destinations());
    window.invoke_frame_selection();
    window.invoke_show_guide();
    assert_eq!(
        lock(&shared).compiled_visibility_overlay()["displayed"],
        true
    );
    assert!(window.get_guide_state().contains("displayed"));
    crate::ui::tests::save_ui_snapshot(window.window(), "visibility-inspection");
    window.invoke_clear_guide();
    let session = lock(&shared);
    assert_eq!(session.compiled_visibility_overlay()["requested"], false);
    assert_eq!(
        before,
        (
            session.document().vmf().clone(),
            session.history_snapshot(),
            session.selected_ids(),
            serde_json::to_value(session.property_draft()).unwrap(),
            std::fs::read(source.with_extension("bsp")).unwrap()
        )
    );
    drop(session);
    std::fs::write(source.with_extension("bsp"), b"changed owned output").unwrap();
    window.invoke_refresh_inspection();
    assert!(window.get_build_state().contains("unavailable"));
    assert_eq!(window.get_rows().row_count(), 0);
    assert_eq!(window.get_visibility_rows().row_count(), 0);
    assert!(!window.get_can_frame());
    assert!(!window.get_can_show_destinations());
    assert!(!controller.window.window().is_visible());
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
}
