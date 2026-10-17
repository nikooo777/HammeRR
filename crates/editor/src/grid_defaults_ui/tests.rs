use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn grid_defaults_window_saves_applies_retries_and_resets_through_shared_session_callbacks() {
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
        .set_spatial(
            Actor::Agent,
            editor_core::spatial::Update {
                grid_size: Some(0.25),
                snap: Some(false),
                grid_visible: Some(false),
                elevation: Some(17.5),
                depth: Some(33.25),
                ..Default::default()
            },
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    let window = &controller.window;
    controller.show(&lock(&shared)).unwrap();
    assert!(window.get_ready());
    assert!(window.get_current().contains("0.25 units"));
    window.invoke_save();
    assert!(window.get_message().contains("saved"));
    assert!(window.get_defaults().contains("0.25 units"));
    crate::ui::tests::save_ui_snapshot(window.window(), "grid-defaults");
    for label in [
        "Save current as defaults",
        "Apply saved defaults",
        "Reload defaults",
        "Reset saved defaults",
        "Close",
    ] {
        crate::ui::tests::assert_dialog_button_visible(window, label);
    }
    lock(&shared)
        .set_spatial(
            Actor::Agent,
            editor_core::spatial::Update {
                grid_size: Some(64.0),
                ..Default::default()
            },
        )
        .unwrap();
    window.invoke_apply();
    assert!(window.get_message().contains("changed"));
    assert_eq!(lock(&shared).spatial().grid_size, 64.0);
    window.invoke_apply();
    assert_eq!(lock(&shared).spatial().grid_size, 0.25);
    assert_eq!(lock(&shared).spatial().elevation, 17.5);
    assert_eq!(lock(&shared).spatial().depth, 33.25);
    std::fs::write(
        directory
            .path()
            .join(editor_core::grid_preferences::FILE_NAME),
        b"bad preferences",
    )
    .unwrap();
    window.invoke_reload();
    assert!(!window.get_ready());
    assert!(!window.get_status().is_empty());
    window.invoke_save();
    assert!(window.get_message().contains("did not load"));
    assert_eq!(
        std::fs::read(
            directory
                .path()
                .join(editor_core::grid_preferences::FILE_NAME)
        )
        .unwrap(),
        b"bad preferences"
    );
    crate::ui::tests::save_ui_snapshot(window.window(), "grid-defaults-error");
    window.invoke_reset();
    assert!(window.get_ready());
    assert_eq!(lock(&shared).spatial().grid_size, 0.25);
    window.invoke_apply();
    assert_eq!(lock(&shared).spatial().grid_size, 16.0);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), 0);
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
}

#[test]
fn shared_saved_grid_errors_refresh_toolbar_and_form_without_invalidating_an_owned_pick() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let defaults =
        editor_core::saved_defaults::SavedDefaults::configured(Ok(directory.path().into()));
    let mut peer = Session::with_saved_defaults(Err("no assets needed".into()), defaults.clone());
    let mut session = Session::with_saved_defaults(Err("no assets needed".into()), defaults);
    session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(64.0),
            "tools/toolsnodraw",
            true,
        )
        .unwrap();
    let draft = session
        .preview_measurement(
            Actor::Agent,
            editor_core::session::measurements::Request {
                id: None,
                name: "Retained grid review".into(),
                start: glam::DVec3::new(-64.0, 0.0, 0.0),
                end: glam::DVec3::X,
            },
            &session.measurement_stamp(),
        )
        .unwrap();
    session
        .begin_measurement_pick(
            Actor::Agent,
            draft["draft"]["token"].as_u64().unwrap(),
            editor_core::session::measurements::Endpoint::End,
            session.camera(),
            (128, 128),
            &session.measurement_pick_stamp(),
        )
        .unwrap();
    let before = session.measurement_state();
    let serials = (
        session.view_serial(),
        session.scene_serial(),
        session.camera_serial(),
    );
    let current = session.spatial();
    let source = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let app = AppWindow::new().unwrap();
    let controller = Controller::new(&shared).unwrap();
    controller.show(&lock(&shared)).unwrap();
    refresh_status(&app, &lock(&shared));
    assert!(!app.get_grid_defaults_error());
    std::fs::write(
        directory
            .path()
            .join(editor_core::grid_preferences::FILE_NAME),
        b"reviewed malformed grid defaults",
    )
    .unwrap();
    let stamp = peer.grid_preferences_stamp();
    assert!(
        peer.change_grid_preferences(Actor::Agent, "reload", &stamp)
            .is_err()
    );
    controller.window.invoke_save();
    assert!(controller.window.get_message().contains("changed"));
    assert!(!controller.window.get_ready());
    refresh_status(&app, &lock(&shared));
    assert!(app.get_grid_defaults_error());
    let stamp = peer.grid_preferences_stamp();
    peer.change_grid_preferences(Actor::Agent, "reset", &stamp)
        .unwrap();
    controller.refresh(&lock(&shared));
    refresh_status(&app, &lock(&shared));
    assert!(controller.window.get_ready());
    assert!(!app.get_grid_defaults_error());
    controller.window.invoke_apply();
    assert!(controller.window.get_message().contains("Finish or cancel"));
    let session = lock(&shared);
    assert_eq!(session.measurement_state(), before);
    assert_eq!(session.spatial(), current);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(
        (
            session.view_serial(),
            session.scene_serial(),
            session.camera_serial()
        ),
        serials
    );
    assert_eq!(
        session.measurement_state()["draft"]["picking"]["valid"],
        true
    );
    controller.window.invoke_close_dialog();
}
