use std::sync::{Arc, Mutex};

use source_formats::keyvalues::WriteOptions;

use super::*;
use crate::compile_ui::Controller;
use crate::ui::tests::{assert_dialog_button_visible, save_ui_snapshot};

fn scroll_to_packing_control(window: &CompileWindow) {
    let size = window.window().size();
    for _ in 0..32 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
        let control = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            window,
            "Pack static prop assets",
        )
        .next()
        .unwrap();
        let top = control.absolute_position().y;
        if top >= 0.0 && top + control.size().height <= size.height as f32 - 64.0 {
            assert_dialog_button_visible(window, "Pack static prop assets");
            return;
        }
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(
                    size.width as f32 * 0.5,
                    size.height as f32 * 0.5,
                ),
                delta_x: 0.0,
                delta_y: -160.0,
            });
    }
    panic!("Static-prop packing control cannot be scrolled into view");
}

#[test]
fn static_prop_packing_checkbox_uses_shared_settings_recipes_recent_maps_and_active_job_guards() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = crate::test_assets::game();
    let vmf = crate::test_assets::room();
    let first = directory.path().join("static-prop-packing-room.vmf");
    let second = directory.path().join("other-room.vmf");
    std::fs::write(&first, vmf.write(&WriteOptions::default())).unwrap();
    std::fs::write(&second, vmf.write(&WriteOptions::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.configure_recent_maps(Ok(directory.path().join("preferences")));
    session.configure_build_recipes(Ok(directory.path().join("preferences")));
    session.open(Actor::User, &first, false).unwrap();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    controller.show(&lock(&shared)).unwrap();
    assert!(controller.window.get_lighting_enabled());
    assert!(!controller.window.get_pack_static_prop_assets());
    assert_eq!(controller.window.get_selected_lighting(), "Fullbright");
    assert!(
        controller
            .window
            .get_static_prop_packing_description()
            .contains("DX90 or newer")
    );
    scroll_to_packing_control(&controller.window);
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &controller.window,
        "Pack static prop assets",
    )
    .next()
    .unwrap()
    .invoke_accessible_default_action();
    assert!(lock(&shared).compile_options().pack_static_prop_assets);
    assert!(!lock(&shared).compile_options().pack_sky_assets);
    assert!(controller.window.get_pack_static_prop_assets());
    controller.refresh(&lock(&shared));
    save_ui_snapshot(controller.window.window(), "build-pack-static-prop-assets");
    for lighting in ["direct_ldr", "direct_ldr_hdr", "fullbright"] {
        lock(&shared)
            .update_build_settings(
                Actor::Agent,
                BuildSettingsUpdate {
                    lighting: Some(lighting),
                    ..Default::default()
                },
            )
            .unwrap();
        controller.refresh(&lock(&shared));
        assert!(controller.window.get_pack_static_prop_assets());
        assert!(controller.window.get_lighting_enabled());
    }
    controller.window.set_recipe_name("Static props".into());
    controller.window.invoke_save_recipe();
    controller.refresh(&lock(&shared));
    controller
        .window
        .invoke_change_pack_static_prop_assets(false);
    assert!(!controller.window.get_pack_static_prop_assets());
    controller.window.invoke_apply_recipe();
    assert!(controller.window.get_pack_static_prop_assets());
    let request = lock(&shared)
        .begin_compile(
            Actor::User,
            Some(&directory.path().join("cancelled-static-props.bsp")),
        )
        .unwrap();
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_lighting_enabled());
    controller
        .window
        .invoke_change_pack_static_prop_assets(false);
    assert!(controller.window.get_pack_static_prop_assets());
    assert!(lock(&shared).compile_options().pack_static_prop_assets);
    assert_eq!(
        lock(&shared).compile_status()["job"]["settings"]["pack_static_prop_assets"],
        true
    );
    controller.window.invoke_cancel_build();
    let result = request.run();
    assert!(result.is_err());
    lock(&shared).finish_compile(Actor::User, &result);
    lock(&shared).open(Actor::User, &second, false).unwrap();
    controller
        .window
        .invoke_change_pack_static_prop_assets(false);
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_pack_static_prop_assets());
    let recent = crate::RecentMapsWindow::new().unwrap();
    crate::recent_maps_ui::bind(&recent, &shared);
    crate::recent_maps_ui::refresh(&recent, &lock(&shared));
    recent.invoke_select_map(first.to_string_lossy().to_string().into());
    assert!(
        recent
            .get_selected_detail()
            .contains("static-prop packing on")
    );
    recent.invoke_open_selected();
    controller.refresh(&lock(&shared));
    assert_eq!(lock(&shared).path(), Some(first.as_path()));
    assert!(controller.window.get_pack_static_prop_assets());
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    assert_eq!(lock(&shared).camera(), camera);
}

#[test]
fn static_prop_packing_details_require_current_published_complete_stats_and_keep_zero_models() {
    let mut job = serde_json::json!({
        "status":"completed","stale":false,
        "result":{"published":true,"stats":{"static_prop_assets":{
            "models":2,"files":8,"asset_bytes":1024
        }}}
    });
    assert_eq!(
        details(&job),
        "\nStatic-prop assets: 2 models · 8 files · 1024 bytes"
    );
    for status in ["pending", "running", "failed", "cancelled"] {
        job["status"] = status.into();
        assert!(details(&job).is_empty());
    }
    job["status"] = "completed".into();
    job["stale"] = true.into();
    assert!(details(&job).is_empty());
    job["stale"] = false.into();
    job["result"]["published"] = false.into();
    assert!(details(&job).is_empty());
    job["result"]["published"] = true.into();
    job["result"]["stats"]["static_prop_assets"] = serde_json::json!({
        "models":0,"files":0,"asset_bytes":0
    });
    assert_eq!(
        details(&job),
        "\nStatic-prop assets: 0 models · 0 files · 0 bytes"
    );
    for field in ["models", "files", "asset_bytes"] {
        let mut missing = job.clone();
        missing["result"]["stats"]["static_prop_assets"][field] = serde_json::Value::Null;
        assert!(details(&missing).is_empty());
    }
}
