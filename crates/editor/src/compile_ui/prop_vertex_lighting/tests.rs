use std::sync::{Arc, Mutex};

use super::*;
use crate::compile_ui::Controller;
use crate::ui::tests::{assert_dialog_button_visible, assert_ui_renders};

#[test]
fn prop_vertex_lighting_controls_share_real_callbacks_recipes_recent_maps_and_active_job_guards() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = crate::test_assets::game();
    let path = directory.path().join("lit-props.vmf");
    std::fs::write(&path, crate::test_assets::room().write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.configure_recent_maps(Ok(directory.path().join("preferences")));
    session.configure_build_recipes(Ok(directory.path().join("preferences")));
    session.open(Actor::User, &path, false).unwrap();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    controller.show(&lock(&shared)).unwrap();
    assert!(!controller.window.get_bake_static_prop_vertices());
    assert!(!controller.window.get_prop_vertex_lighting_enabled());
    assert!(
        controller
            .window
            .get_prop_vertex_lighting_description()
            .contains("Choose Direct LDR")
    );
    controller
        .window
        .invoke_change_bake_static_prop_vertices(true);
    assert!(!lock(&shared).compile_options().bake_static_prop_vertices);
    assert!(!controller.window.get_bake_static_prop_vertices());
    assert!(
        controller
            .window
            .get_message()
            .contains("requires Direct LDR")
    );
    controller.window.invoke_change_lighting(1);
    assert!(controller.window.get_prop_vertex_lighting_enabled());
    assert_dialog_button_visible(&controller.window, "Bake static-prop vertex lighting");
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &controller.window,
        "Bake static-prop vertex lighting",
    )
    .next()
    .unwrap()
    .invoke_accessible_default_action();
    assert!(lock(&shared).compile_options().bake_static_prop_vertices);
    assert!(controller.window.get_bake_static_prop_vertices());
    for depth in 2..=compiler::MAX_LIGHT_BOUNCES {
        controller.window.invoke_change_bounces(i32::from(depth));
        assert_eq!(lock(&shared).compile_options().bounces, depth);
        assert_eq!(
            controller.window.get_selected_bounces(),
            compiler::bounce_label(depth)
        );
        assert!(controller.window.get_prop_vertex_lighting_enabled());
    }
    let maximum = lock(&shared).compile_settings();
    controller
        .window
        .invoke_change_bounces(i32::from(compiler::MAX_LIGHT_BOUNCES) + 1);
    assert_eq!(lock(&shared).compile_settings(), maximum);
    assert_eq!(
        controller.window.get_selected_bounces(),
        "Four diffuse bounces"
    );
    controller.window.invoke_change_bounces(1);
    assert_eq!(lock(&shared).compile_options().bounces, 1);
    assert_eq!(
        controller.window.get_selected_bounces(),
        "One diffuse bounce"
    );
    assert!(controller.window.get_prop_vertex_lighting_enabled());
    controller.window.invoke_change_bounces(0);
    assert_eq!(controller.window.get_selected_bounces(), "Direct only");
    controller.window.invoke_change_lighting(0);
    assert_eq!(controller.window.get_selected_lighting(), "Direct LDR");
    assert!(controller.window.get_bake_static_prop_vertices());
    controller
        .window
        .invoke_change_bounces(i32::from(compiler::MAX_LIGHT_BOUNCES));
    controller
        .window
        .set_recipe_name("Four-bounce prop bake".into());
    controller.window.invoke_save_recipe();
    controller
        .window
        .invoke_change_bake_static_prop_vertices(false);
    controller.window.invoke_change_bounces(0);
    controller.window.invoke_apply_recipe();
    assert!(controller.window.get_bake_static_prop_vertices());
    assert_eq!(
        lock(&shared).compile_options().bounces,
        compiler::MAX_LIGHT_BOUNCES
    );
    controller.window.invoke_change_lighting(2);
    assert_eq!(
        lock(&shared).compile_options().lighting,
        compiler::LightingMode::DirectLdrHdr
    );
    assert!(controller.window.get_bake_static_prop_vertices());
    assert_ui_renders(controller.window.window(), "build-prop-vertex-lighting");
    let request = lock(&shared).begin_compile(Actor::User, None).unwrap();
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_prop_vertex_lighting_enabled());
    controller
        .window
        .invoke_change_bake_static_prop_vertices(false);
    assert!(controller.window.get_bake_static_prop_vertices());
    assert!(controller.window.get_message().contains("active compile"));
    assert_eq!(
        lock(&shared).compile_status()["job"]["settings"]["bake_static_prop_vertices"],
        true
    );
    assert_eq!(
        lock(&shared).compile_status()["job"]["settings"]["bounces"],
        compiler::MAX_LIGHT_BOUNCES
    );
    assert!(
        !controller
            .window
            .get_details()
            .contains("Prop vertex lighting bytes:")
    );
    controller.window.invoke_cancel_build();
    let result = request.run();
    assert!(result.is_err());
    lock(&shared).finish_compile(Actor::User, &result);
    controller.refresh(&lock(&shared));
    assert!(controller.window.get_prop_vertex_lighting_enabled());
    lock(&shared).new_map(Actor::User, false).unwrap();
    controller
        .window
        .invoke_change_bake_static_prop_vertices(false);
    controller.window.invoke_change_bounces(0);
    lock(&shared)
        .open_recent_map(Actor::User, &path, false)
        .unwrap();
    controller.refresh(&lock(&shared));
    assert!(controller.window.get_bake_static_prop_vertices());
    assert_eq!(
        controller.window.get_selected_bounces(),
        "Four diffuse bounces"
    );
    assert_eq!(
        controller.window.get_selected_lighting(),
        "Direct LDR + HDR"
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
}

#[test]
fn prop_vertex_lighting_details_show_only_current_completed_published_actual_channels() {
    let mut job = serde_json::json!({
        "status":"completed","stale":false,
        "result":{"published":true,"stats":{"prop_vertex_lighting":{
            "props":2,"models":1,"meshes":4,"vertices":18,
            "ldr_bytes":1024,"hdr_bytes":2048,"work":72,"asset_bytes":5000
        }}}
    });
    let text = details(&job);
    assert!(text.contains("2 props · 1 models · 4 hardware meshes · 18 vertices"));
    assert!(text.contains("LDR 1024 · HDR 2048"));
    for status in ["running", "cancelled", "failed"] {
        job["status"] = status.into();
        assert!(details(&job).is_empty());
    }
    job["status"] = "completed".into();
    job["result"]["published"] = false.into();
    assert!(details(&job).is_empty());
    job["result"]["published"] = true.into();
    job["stale"] = true.into();
    assert!(details(&job).is_empty());
    job["stale"] = false.into();
    job["result"]["stats"]["prop_vertex_lighting"]["hdr_bytes"] = 0.into();
    assert!(details(&job).contains("LDR 1024 · HDR 0"));
    job["result"]["stats"]["prop_vertex_lighting"]["vertices"] = serde_json::Value::Null;
    assert!(details(&job).is_empty());
    job["result"]["stats"]["prop_vertex_lighting"] = serde_json::Value::Null;
    assert!(details(&job).is_empty());
}
