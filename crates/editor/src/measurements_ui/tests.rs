use super::*;
use slint::Model;
use std::sync::{Arc, Mutex};

fn edit(window: &MeasurementsWindow, label: &str, value: &str) {
    window.window().take_snapshot().unwrap();
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(window, label)
        .next()
        .unwrap()
        .set_accessible_value(value);
}

#[test]
fn measurement_window_edits_exact_endpoints_previews_saves_frames_deletes_and_cancels() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let shared = Arc::new(Mutex::new(Session::new(Err("no assets needed".into()))));
    let original = lock(&shared).document().vmf().clone();
    let controller = Controller::new(&shared).unwrap();
    let window = &controller.window;
    controller.show(&lock(&shared)).unwrap();
    edit(window, "Measurement name", "Stair clearance");
    edit(window, "Start X", "4.125");
    edit(window, "Start Y", "7");
    edit(window, "Start Z", "20");
    edit(window, "End X", "1.125");
    edit(window, "End Y", "11");
    edit(window, "End Z", "8");
    assert_eq!(window.get_start_x(), "4.125");
    assert!(window.get_result().contains("13.000000"));
    assert!(window.get_result().contains("ΔX -3.000000"));
    window.invoke_preview();
    assert!(window.get_ready(), "{}", window.get_message());
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert_eq!(lock(&shared).document().undo_labels().count(), 0);
    edit(window, "End X", "NaN");
    assert!(!window.get_ready());
    window.invoke_preview();
    assert!(window.get_message().contains("finite"));
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &original);
    edit(window, "End X", "1.125");
    window.invoke_preview();
    assert!(window.get_ready(), "{}", window.get_message());
    crate::ui::tests::save_ui_snapshot(window.window(), "measurements-preview");
    for label in [
        "New measurement",
        "Frame",
        "Delete",
        "Preview",
        "Apply",
        "Cancel",
    ] {
        crate::ui::tests::assert_dialog_button_visible(window, label);
    }
    window.invoke_apply();
    assert_eq!(lock(&shared).document().undo_labels().count(), 1);
    assert_eq!(
        lock(&shared).measurements().unwrap().measurements[0]
            .start
            .x,
        4.125
    );
    let saved = lock(&shared).document().vmf().clone();
    window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &saved);
    window.invoke_choose("1".into());
    let camera = lock(&shared).camera();
    window.invoke_frame();
    assert_ne!(lock(&shared).camera(), camera);
    assert_eq!(lock(&shared).document().undo_labels().count(), 1);
    window.set_show_guides(false);
    window.invoke_view_changed();
    assert_eq!(lock(&shared).measurement_state()["show_guides"], false);
    window.set_show_guides(true);
    window.invoke_view_changed();
    crate::ui::tests::save_ui_snapshot(window.window(), "measurements");
    let row = window.get_measurements().row_data(0).unwrap();
    let label = format!("{}\n{}", row.name, row.detail);
    let button = i_slint_backend_testing::ElementHandle::find_by_accessible_label(window, &label)
        .next()
        .unwrap();
    assert!((button.size().height - 54.0).abs() < 0.5);
    edit(window, "Measurement name", "Revised clearance");
    edit(window, "End Z", "32");
    window.invoke_preview();
    window.invoke_apply();
    assert_eq!(
        lock(&shared).measurements().unwrap().measurements[0].name,
        "Revised clearance"
    );
    assert_eq!(lock(&shared).document().undo_labels().count(), 2);
    window.invoke_delete_measurement();
    assert!(
        lock(&shared)
            .measurements()
            .unwrap()
            .measurements
            .is_empty()
    );
    lock(&shared).undo(Actor::User).unwrap();
    controller.refresh(&lock(&shared));
    assert_eq!(lock(&shared).measurements().unwrap().measurements.len(), 1);
    window.invoke_choose("1".into());
    edit(window, "End Z", "48");
    window.invoke_preview();
    let before_cancel = lock(&shared).document().vmf().clone();
    window.invoke_cancel();
    assert_eq!(lock(&shared).document().vmf(), &before_cancel);
    assert!(lock(&shared).measurement_state()["draft"].is_null());
    assert!(!window.window().is_visible());
}

#[test]
fn measurement_window_reviews_foreign_previews_and_guards_stale_map_callbacks() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let shared = Arc::new(Mutex::new(Session::new(Err("no assets needed".into()))));
    let foreign = {
        let mut session = lock(&shared);
        let snapshot = session.measurement_stamp();
        session
            .preview_measurement(
                Actor::Agent,
                Request {
                    id: None,
                    name: "Agent ruler".into(),
                    start: DVec3::ZERO,
                    end: DVec3::new(3.0, 4.0, 12.0),
                },
                &snapshot,
            )
            .unwrap()
    };
    let controller = Controller::new(&shared).unwrap();
    let window = &controller.window;
    controller.show(&lock(&shared)).unwrap();
    assert!(window.get_can_review());
    assert!(!window.get_ready());
    assert!(!window.get_can_preview());
    window.invoke_apply();
    assert!(
        lock(&shared)
            .measurements()
            .unwrap()
            .measurements
            .is_empty()
    );
    window.invoke_cancel();
    assert_eq!(
        lock(&shared).measurement_state()["draft"]["token"],
        foreign["draft"]["token"]
    );
    controller.show(&lock(&shared)).unwrap();
    window.invoke_review();
    assert!(window.get_ready(), "{}", window.get_message());
    assert_eq!(window.get_measurement_name(), "Agent ruler");
    assert_eq!(window.get_end_z(), "12");
    crate::ui::tests::save_ui_snapshot(window.window(), "measurements-reviewed");
    window.invoke_apply();
    assert_eq!(lock(&shared).measurements().unwrap().measurements.len(), 1);
    window.invoke_choose("1".into());
    lock(&shared).new_map(Actor::Agent, true).unwrap();
    window.invoke_delete_measurement();
    assert!(window.get_message().contains("changed"));
    window.invoke_preview();
    assert!(window.get_message().contains("changed"));
    controller.refresh(&lock(&shared));
    assert!(!window.get_editable());
    assert!(
        lock(&shared)
            .measurements()
            .unwrap()
            .measurements
            .is_empty()
    );
    assert!(lock(&shared).measurement_state()["draft"].is_null());
}

#[test]
fn measurement_surface_controls_pick_exact_endpoints_cancel_and_guard_replaced_or_stale_views() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session
        .create_block(
            Actor::User,
            DVec3::new(-128.0, -128.0, -16.0),
            DVec3::new(128.0, 128.0, 0.0),
            "brick/test",
            false,
        )
        .unwrap();
    session.set_camera(render::Camera::planar(
        render::Projection::Top,
        glam::Vec3::new(3.125, -5.25, 0.0),
        256.0,
    ));
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    controller.set_viewport((511, 511));
    let window = &controller.window;
    controller.show(&lock(&shared)).unwrap();
    edit(window, "Measurement name", "Surface clearance");
    edit(window, "End X", "64.123456789012");
    edit(window, "End Z", "8.125");
    window.invoke_pick_start();
    assert!(lock(&shared).measurement_state()["draft"].is_null());
    window.invoke_preview();
    assert!(window.get_can_pick());
    for label in ["Pick start", "Pick end"] {
        crate::ui::tests::assert_dialog_button_visible(window, label);
    }
    window.invoke_pick_start();
    assert!(!window.window().is_visible());
    assert!(!window.get_ready());
    assert_eq!(
        lock(&shared).measurement_state()["draft"]["picking"]["endpoint"],
        "start"
    );
    controller
        .pick_at_pixel(&mut lock(&shared), glam::DVec2::splat(255.2), (511, 511))
        .unwrap();
    assert!(window.window().is_visible());
    assert!(window.get_ready(), "{}", window.get_preview_status());
    assert_eq!(window.get_start_x(), "3.125");
    assert_eq!(window.get_start_y(), "-5.25");
    assert_eq!(window.get_start_z(), "0");
    assert_eq!(window.get_end_x(), "64.123456789012");
    assert_eq!(window.get_end_z(), "8.125");
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    crate::ui::tests::save_ui_snapshot(window.window(), "measurements-surface-picked");
    window.invoke_pick_end();
    let state = lock(&shared).measurement_state();
    assert!(
        controller
            .pick_at_pixel(&mut lock(&shared), glam::DVec2::splat(255.2), (512, 511))
            .is_err()
    );
    assert_eq!(lock(&shared).measurement_state(), state);
    controller.cancel_pick(&mut lock(&shared)).unwrap();
    assert!(window.get_ready());
    assert_eq!(window.get_end_x(), "64.123456789012");
    window.invoke_pick_end();
    {
        let mut session = lock(&shared);
        let mut camera = session.camera();
        camera.position.x += 16.0;
        session.publish_camera(camera);
    }
    let state = lock(&shared).measurement_state();
    assert!(
        controller
            .pick_at_pixel(&mut lock(&shared), glam::DVec2::splat(255.2), (511, 511))
            .is_err()
    );
    assert_eq!(lock(&shared).measurement_state(), state);
    controller.cancel_pick(&mut lock(&shared)).unwrap();
    edit(window, "Start X", "1.00000000012345");
    assert!(!window.get_can_pick());
    window.invoke_preview();
    window.invoke_apply();
    assert_eq!(
        lock(&shared).measurements().unwrap().measurements[0]
            .start
            .x,
        1.00000000012345
    );
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    let foreign = {
        let mut session = lock(&shared);
        let snapshot = session.measurement_stamp();
        session
            .preview_measurement(
                Actor::Agent,
                Request {
                    id: Some(1),
                    name: "Agent revision".into(),
                    start: DVec3::ZERO,
                    end: DVec3::splat(64.0),
                },
                &snapshot,
            )
            .unwrap()
    };
    controller.refresh(&lock(&shared));
    window.invoke_pick_end();
    assert_eq!(lock(&shared).measurement_state(), foreign);
    window.invoke_review();
    window.invoke_pick_end();
    let replacement = {
        let mut session = lock(&shared);
        let token = session.measurement_state()["draft"]["token"]
            .as_u64()
            .unwrap();
        session.review_measurement(Actor::Agent, token).unwrap()
    };
    assert!(
        controller
            .pick_at_pixel(&mut lock(&shared), glam::DVec2::splat(255.2), (511, 511))
            .is_err()
    );
    window.invoke_cancel();
    assert_eq!(lock(&shared).measurement_state(), replacement);
}
