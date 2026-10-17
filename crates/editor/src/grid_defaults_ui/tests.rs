use super::*;
use crate::runtime::lock;
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

#[test]
fn retained_defaults_dialogs_reject_drained_document_without_storage_or_peer_changes() {
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
    let peer = Arc::new(Mutex::new(Session::with_saved_defaults(
        Err("no assets needed".into()),
        defaults.clone(),
    )));
    let peer_document = crate::test_assets::document(peer.clone());
    let mut state = Session::with_saved_defaults(Err("no assets needed".into()), defaults);
    state
        .set_spatial(
            Actor::User,
            editor_core::spatial::Update {
                grid_size: Some(0.25),
                ..Default::default()
            },
        )
        .unwrap();
    state.set_view(render::ViewOptions {
        selection_outlines: false,
        ..state.view()
    });
    state.set_fly_speed(987.625).unwrap();
    let shared = Arc::new(Mutex::new(state));
    let weak_session = Arc::downgrade(&shared);
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let grid = Controller::new_with_access(&access).unwrap();
    let preview = crate::preview_defaults_ui::Controller::new_with_access(&access).unwrap();
    let navigation = crate::navigation_defaults_ui::Controller::new_with_access(&access).unwrap();
    grid.show(&lock(&shared)).unwrap();
    preview.show(&lock(&shared)).unwrap();
    navigation.show(&lock(&shared)).unwrap();
    grid.window.invoke_save();
    preview.window.invoke_save();
    navigation.window.invoke_save();
    access
        .with_session(|session| {
            session.set_spatial(
                Actor::User,
                editor_core::spatial::Update {
                    grid_size: Some(64.0),
                    ..Default::default()
                },
            )?;
            session.set_fly_speed(1200.0)?;
            session.set_view(render::ViewOptions {
                selection_outlines: true,
                ..session.view()
            });
            Ok(())
        })
        .unwrap();
    grid.refresh(&lock(&shared));
    preview.refresh(&lock(&shared));
    navigation.refresh(&lock(&shared));
    document.shutdown().unwrap();
    let paths = [
        editor_core::grid_preferences::FILE_NAME,
        editor_core::preview_preferences::FILE_NAME,
        editor_core::navigation_preferences::FILE_NAME,
    ]
    .map(|name| directory.path().join(name));
    std::fs::write(&paths[1], b"external malformed defaults").unwrap();
    let bytes = paths.each_ref().map(|path| std::fs::read(path).unwrap());
    let before = defaults_state(&lock(&shared));
    let peer_before = defaults_state(&lock(&peer));
    let display = (
        grid.window.get_current(),
        preview.window.get_status(),
        navigation.window.get_current(),
    );
    grid.window.invoke_apply();
    preview.window.invoke_reset();
    navigation.window.invoke_save();
    preview.window.invoke_reload();
    assert_eq!(grid.window.get_message(), "the document is closed");
    assert_eq!(preview.window.get_message(), "the document is closed");
    assert_eq!(navigation.window.get_message(), "the document is closed");
    assert_eq!(defaults_state(&lock(&shared)), before);
    assert_eq!(defaults_state(&lock(&peer)), peer_before);
    assert_eq!(
        paths.each_ref().map(|path| std::fs::read(path).unwrap()),
        bytes
    );
    assert_eq!(
        (
            grid.window.get_current(),
            preview.window.get_status(),
            navigation.window.get_current(),
        ),
        display
    );
    assert_eq!(
        peer_document.lifecycle(),
        crate::runtime::DocumentLifecycle::Live
    );
    grid.window.invoke_close_dialog();
    preview.window.invoke_close_dialog();
    navigation.window.invoke_close_dialog();
    assert!(!grid.window.window().is_visible());
    assert!(!preview.window.window().is_visible());
    assert!(!navigation.window.window().is_visible());
    drop(access);
    drop(document);
    drop(shared);
    assert!(weak_session.upgrade().is_none());
    grid.window.invoke_save();
    assert_eq!(grid.window.get_message(), "the document is closed");
    peer_document.shutdown().unwrap();
}

fn defaults_state(session: &Session) -> (Value, render::Camera) {
    (
        serde_json::json!({
            "grid": session.grid_preferences_state(),
            "preview": session.preview_preferences_state(),
            "navigation": session.navigation_preferences_state(),
            "history": session.history_snapshot(),
            "log": session.log_serial(),
        }),
        session.camera(),
    )
}
