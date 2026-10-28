use super::*;
use render::Projection;
use std::sync::{Arc, Mutex};

#[test]
fn speed_input_wheel_routing_and_defaults_callbacks_preserve_owned_preview() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().into()));
    session
        .preview_block(glam::DVec3::ZERO, glam::DVec3::splat(32.0))
        .unwrap();
    let preview = session.block_preview();
    let source = session.document().vmf().clone();
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let app = AppWindow::new().unwrap();
    bind_speed(&app, &shared);
    app.show().unwrap();
    app.window().set_size(slint::PhysicalSize::new(1600, 960));
    crate::ui::tests::assert_dialog_button_visible(&app, "Fly speed");
    crate::ui::tests::assert_dialog_button_visible(&app, "Navigation defaults…");
    assert_eq!(app.get_fly_speed(), "600");
    app.set_fly_speed("987.625".into());
    app.invoke_fly_speed_changed();
    assert_eq!(lock(&shared).fly_speed(), 987.625);
    for invalid in ["garbage", "NaN", "24", "20001"] {
        app.set_fly_speed(invalid.into());
        app.invoke_fly_speed_changed();
        assert!(!app.get_message().is_empty());
        assert_eq!(app.get_fly_speed(), "987.625");
        assert_eq!(lock(&shared).fly_speed(), 987.625);
    }
    for projection in [Projection::Top, Projection::Front, Projection::Side] {
        assert!(!wheel_speed(&mut lock(&shared), projection, true, 240.0).unwrap());
    }
    assert!(!wheel_speed(&mut lock(&shared), Projection::Perspective, false, 240.0).unwrap());
    assert_eq!(lock(&shared).fly_speed(), 987.625);
    assert!(wheel_speed(&mut lock(&shared), Projection::Perspective, true, 240.0).unwrap());
    assert_eq!(lock(&shared).fly_speed(), 1975.25);
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(lock(&shared).block_preview(), preview);
    let path = directory
        .path()
        .join(editor_core::navigation_preferences::FILE_NAME);
    assert!(!path.exists());
    let controller = Controller::new(&shared).unwrap();
    controller.show(&lock(&shared)).unwrap();
    let window = &controller.window;
    window.window().set_size(slint::PhysicalSize::new(760, 720));
    assert_eq!(window.get_current(), "1975.25 units/s");
    for label in [
        "Save current as defaults",
        "Apply saved defaults",
        "Reload defaults",
        "Reset saved defaults",
        "Close",
    ] {
        crate::ui::tests::assert_dialog_button_visible(window, label);
    }
    crate::ui::tests::assert_ui_renders(window.window(), "navigation-defaults");
    window.invoke_save();
    assert_eq!(window.get_saved(), "1975.25 units/s");
    lock(&shared).set_fly_speed(600.0).unwrap();
    window.invoke_apply();
    assert!(window.get_message().contains("changed"));
    assert_eq!(lock(&shared).fly_speed(), 600.0);
    window.invoke_apply();
    assert_eq!(lock(&shared).fly_speed(), 1975.25);
    std::fs::write(&path, b"broken defaults").unwrap();
    window.invoke_reload();
    assert!(!window.get_ready());
    assert!(!window.get_status().is_empty());
    window.invoke_save();
    assert!(window.get_message().contains("did not load"));
    assert_eq!(std::fs::read(&path).unwrap(), b"broken defaults");
    window.invoke_reset();
    assert!(window.get_ready());
    assert_eq!(lock(&shared).fly_speed(), 1975.25);
    window.invoke_apply();
    assert_eq!(lock(&shared).fly_speed(), 600.0);
    let session = lock(&shared);
    assert_eq!(session.block_preview(), preview);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), 0);
    assert_eq!(session.camera(), camera);
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
    app.hide().unwrap();
}
