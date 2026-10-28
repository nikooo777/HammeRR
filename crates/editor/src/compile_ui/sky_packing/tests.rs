use std::sync::{Arc, Mutex};
use std::time::Duration;

use source_formats::keyvalues::WriteOptions;

use super::*;
use crate::compile_ui::Controller;
use crate::session::Actor;
use crate::ui::tests::{assert_dialog_button_visible, assert_ui_renders};

#[test]
fn sky_packing_controls_share_callbacks_restores_job_guards_and_result_statistics() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = crate::test_assets::game();
    let skybox = directory.path().join("materials/skybox");
    std::fs::create_dir_all(&skybox).unwrap();
    for suffix in ["rt", "bk", "lf", "ft", "up", "dn"] {
        std::fs::write(
            skybox.join(format!("ui_pack_sky{suffix}.vmt")),
            "UnlitGeneric { $basetexture test/checker }",
        )
        .unwrap();
    }
    let mut vmf = crate::test_assets::room();
    vmf.world.properties.set("skyname", "ui_pack_sky");
    for solid in &mut vmf.world.solids {
        for side in &mut solid.sides {
            side.material = "test/checker".into();
        }
    }
    let first = directory.path().join("packed-sky-room.vmf");
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
    assert!(lock(&shared).compile_status()["job"].is_null());
    assert!(controller.window.get_lighting_enabled());
    assert!(!controller.window.get_pack_sky_assets());
    assert_eq!(controller.window.get_selected_lighting(), "Fullbright");
    assert!(
        !controller
            .window
            .get_details()
            .contains("Packed sky assets:")
    );
    assert_dialog_button_visible(&controller.window, "Pack sky assets");
    assert_ui_renders(controller.window.window(), "build-pack-sky-default");
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &controller.window,
        "Pack sky assets",
    )
    .next()
    .unwrap()
    .invoke_accessible_default_action();
    assert!(lock(&shared).compile_options().pack_sky_assets);
    assert!(controller.window.get_pack_sky_assets());
    assert!(
        controller
            .window
            .get_message()
            .contains("unsupported assets stop")
    );
    controller
        .window
        .set_recipe_name("Portable sky check".into());
    controller.window.invoke_save_recipe();
    controller.refresh(&lock(&shared));
    assert_eq!(
        controller.window.get_selected_recipe(),
        "Portable sky check"
    );
    controller.window.invoke_change_pack_sky_assets(false);
    assert!(!controller.window.get_pack_sky_assets());
    controller.window.invoke_apply_recipe();
    assert!(controller.window.get_pack_sky_assets());
    controller.refresh(&lock(&shared));
    assert_ui_renders(controller.window.window(), "build-pack-sky-controls");
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    assert_eq!(lock(&shared).camera(), camera);
    let request = lock(&shared)
        .begin_compile(
            Actor::User,
            Some(&directory.path().join("cancelled-sky.bsp")),
        )
        .unwrap();
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_lighting_enabled());
    controller.window.invoke_change_pack_sky_assets(false);
    assert!(controller.window.get_pack_sky_assets());
    assert!(lock(&shared).compile_options().pack_sky_assets);
    assert!(controller.window.get_message().contains("compil"));
    assert_eq!(
        lock(&shared).compile_status()["job"]["settings"]["pack_sky_assets"],
        true
    );
    assert!(
        !controller
            .window
            .get_details()
            .contains("Packed sky assets:")
    );
    assert_ui_renders(controller.window.window(), "build-pack-sky-active");
    controller.window.invoke_cancel_build();
    let result = request.run();
    assert!(result.is_err());
    lock(&shared).finish_compile(Actor::User, &result);
    controller.refresh(&lock(&shared));
    assert!(controller.window.get_lighting_enabled());
    assert!(
        !controller
            .window
            .get_details()
            .contains("Packed sky assets:")
    );
    lock(&shared).open(Actor::User, &second, false).unwrap();
    controller.window.invoke_change_pack_sky_assets(false);
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_pack_sky_assets());
    let recent = crate::RecentMapsWindow::new().unwrap();
    crate::recent_maps_ui::bind(&recent, &shared);
    crate::recent_maps_ui::refresh(&recent, &lock(&shared));
    recent.invoke_select_map(first.to_string_lossy().to_string().into());
    assert!(recent.get_selected_detail().contains("sky packing on"));
    recent.invoke_open_selected();
    controller.refresh(&lock(&shared));
    assert_eq!(lock(&shared).path(), Some(first.as_path()));
    assert!(controller.window.get_pack_sky_assets());
    let request = lock(&shared)
        .begin_compile(
            Actor::User,
            Some(&directory.path().join("packed-sky-room.bsp")),
        )
        .unwrap();
    let result = request.run();
    assert!(result.is_ok(), "{:?}", result.as_ref().err());
    lock(&shared).finish_compile(Actor::User, &result);
    controller.refresh(&lock(&shared));
    let status = lock(&shared).compile_status();
    assert_eq!(status["job"]["status"], "completed");
    assert_eq!(status["job"]["result"]["stats"]["sky_assets"]["files"], 7);
    assert!(
        controller
            .window
            .get_details()
            .contains("Packed sky assets: ui_pack_sky · LDR · 7 files")
    );
    controller.window.invoke_change_pack_sky_assets(false);
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_pack_sky_assets());
    assert!(
        controller
            .window
            .get_details()
            .contains("Packed sky assets: ui_pack_sky")
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    controller.window.invoke_change_pack_sky_assets(true);
    controller.refresh(&lock(&shared));
    let size = controller.window.window().size();
    controller
        .window
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(size.width as f32 - 20.0, 250.0),
            delta_x: 0.0,
            delta_y: -10000.0,
        });
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    assert_ui_renders(controller.window.window(), "build-pack-sky-details");
    let output = directory.path().join("packed-sky-room.bsp");
    let published = std::fs::read(&output).unwrap();
    let request = lock(&shared)
        .begin_compile(Actor::User, Some(&output))
        .unwrap();
    let result = request.run();
    assert!(result.is_ok(), "{:?}", result.as_ref().err());
    assert!(result.as_ref().unwrap().output.stats.sky_assets.is_some());
    let mut external_source = std::fs::read_to_string(&first).unwrap();
    external_source.push_str("\n// source changed after worker preparation\n");
    std::fs::write(&first, external_source).unwrap();
    lock(&shared).finish_compile(Actor::User, &result);
    controller.refresh(&lock(&shared));
    let status = lock(&shared).compile_status();
    assert_eq!(status["job"]["status"], "failed");
    assert_eq!(status["job"]["result"]["published"], false);
    assert_eq!(status["job"]["result"]["stats"]["sky_assets"]["files"], 7);
    assert!(status["job"]["result"]["error"].as_str().is_some());
    assert_eq!(std::fs::read(&output).unwrap(), published);
    assert!(
        !controller
            .window
            .get_details()
            .contains("Packed sky assets:")
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
}
