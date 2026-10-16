use super::*;
use map_core::ObjectId;
use std::sync::{Arc, Mutex};

#[test]
fn cubemap_window_keeps_cached_inspection_and_requires_current_owned_review() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (assets, mut session, near, far, face) =
        editor_core::session::cubemap_bindings::test_support::fixture();
    session
        .select(&[ObjectId::Entity(far)], SelectMode::Replace)
        .unwrap();
    let before = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    controller.show(&lock(&shared)).unwrap();
    assert!(
        controller
            .window
            .get_helper_label()
            .contains(&format!("Helper {far}"))
    );
    lock(&shared)
        .select_faces(&[face], SelectMode::Replace)
        .unwrap();
    controller.refresh(&lock(&shared));
    assert!(controller.window.get_can_bind());
    controller.window.invoke_bind();
    assert!(
        controller.window.get_ready(),
        "{}",
        controller.window.get_message()
    );
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    lock(&shared)
        .select_faces(&[], SelectMode::Replace)
        .unwrap();
    lock(&shared)
        .select_faces(&[face], SelectMode::Replace)
        .unwrap();
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_ready());
    controller.window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &before);
    controller.window.invoke_bind();
    assert!(controller.window.get_ready());
    let foreign = {
        let mut session = lock(&shared);
        let snapshot = session.cubemap_bindings_stamp();
        session
            .preview_cubemap_bindings(
                Actor::Agent,
                Request {
                    helper: far,
                    faces: vec![face],
                    operation: Operation::Bind,
                },
                &snapshot,
            )
            .unwrap()
    };
    let foreign_token = foreign["draft"]["token"].as_u64().unwrap();
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_ready());
    assert!(controller.window.get_can_review());
    controller.window.invoke_cancel();
    assert_eq!(
        lock(&shared).cubemap_binding_state()["draft"]["token"],
        foreign_token
    );
    controller.show(&lock(&shared)).unwrap();
    controller.window.invoke_review();
    assert!(controller.window.get_ready());
    controller.window.invoke_apply();
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    controller.window.invoke_refresh();
    assert!(controller.window.get_can_clear());
    controller.window.invoke_clear_bindings();
    let state = lock(&shared).cubemap_binding_state();
    assert_eq!(state["draft"]["report"]["assignments"][0]["helper"], near);
    assert_eq!(state["draft"]["report"]["assignments"][0]["manual"], false);
    assert!(controller.window.get_ready());
    assert!(controller.window.window().is_visible());
    crate::ui::tests::save_ui_snapshot(controller.window.window(), "cubemap-faces");
    for label in [
        "Bind selected faces",
        "Clear all manual bindings",
        "Refresh",
        "Apply",
        "Cancel",
    ] {
        crate::ui::tests::assert_dialog_button_visible(&controller.window, label);
    }
    controller.window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &before);
    controller.window.invoke_refresh();
    let cached = controller.form.borrow().report.clone();
    std::fs::write(
        assets.path().join("materials/test/checker.vmt"),
        "LightmappedGeneric { $envmap",
    )
    .unwrap();
    controller.refresh(&lock(&shared));
    assert_eq!(controller.form.borrow().report, cached);
    controller.window.invoke_refresh();
    assert!(controller.window.get_helper_status().contains("checker"));
    assert!(
        !controller.form.borrow().report.as_ref().unwrap()["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    std::fs::write(
        assets.path().join("materials/test/checker.vmt"),
        "LightmappedGeneric { $basetexture test/checker $envmap env_cubemap }",
    )
    .unwrap();
    controller.window.invoke_refresh();
    controller.window.invoke_bind();
    assert!(controller.window.get_ready());
    controller.window.invoke_cancel();
    assert!(lock(&shared).cubemap_binding_state()["draft"].is_null());
    assert_eq!(lock(&shared).document().vmf(), &before);
}
