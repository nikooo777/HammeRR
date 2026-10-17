use super::*;
use slint::Model;
use std::sync::{Arc, Mutex};

#[test]
fn reload_saved_list_callback_recovers_a_conflict_and_preserves_the_owned_review() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.vmf");
    let second = directory.path().join("second.vmf");
    for path in [&first, &second] {
        std::fs::write(path, "world { id 1 classname worldspawn }").unwrap();
    }
    let mut state = Session::new(Err("no assets needed".into()));
    state.configure_recent_maps(Ok(directory.path().into()));
    state.open(Actor::User, &first, false).unwrap();
    state
        .preview_measurement(
            Actor::Agent,
            editor_core::session::measurements::Request {
                id: None,
                name: "Owned clearance".into(),
                start: glam::DVec3::ZERO,
                end: glam::DVec3::X,
            },
            &state.measurement_stamp(),
        )
        .unwrap();
    let owned = state.measurement_state()["draft"].clone();
    let document = state.document().vmf().clone();
    let camera = state.camera();
    let shared = Arc::new(Mutex::new(state));
    let window = RecentMapsWindow::new().unwrap();
    bind(&window, &shared);
    refresh(&window, &lock(&shared));
    window.show().unwrap();
    window.window().set_size(slint::PhysicalSize::new(800, 760));
    crate::ui::tests::assert_dialog_button_visible(&window, "Reload saved list");
    let mut other = Session::new(Err("no assets needed".into()));
    other.configure_recent_maps(Ok(directory.path().into()));
    other.open(Actor::Agent, &second, false).unwrap();
    let path = directory.path().join(crate::recent_maps::FILE_NAME);
    let saved = std::fs::read(&path).unwrap();
    window.set_confirm_clear(true);
    window.invoke_clear_list();
    assert!(!window.get_writable());
    assert!(window.get_warning().contains("Reload saved list"));
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    crate::ui::tests::save_ui_snapshot(window.window(), "recent-map-conflicts");
    window.invoke_refresh();
    assert!(window.get_writable());
    assert!(window.get_warning().is_empty());
    assert_eq!(window.get_rows().row_count(), 2);
    assert!(!window.get_confirm_clear());
    std::fs::write(&path, b"broken preferences").unwrap();
    window.invoke_refresh();
    assert!(!window.get_writable());
    assert_eq!(window.get_rows().row_count(), 2);
    std::fs::write(&path, &saved).unwrap();
    window.invoke_refresh();
    assert!(window.get_writable());
    let state = lock(&shared);
    assert_eq!(state.document().vmf(), &document);
    assert_eq!(state.camera(), camera);
    assert_eq!(state.measurement_state()["draft"], owned);
    assert_eq!(state.path(), Some(first.as_path()));
    assert_eq!(state.document().undo_labels().count(), 0);
    window.invoke_cancel();
}

#[test]
fn recent_map_callbacks_share_the_list_without_changing_the_other_document() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let authority =
        editor_core::recent_maps::Authority::configured(Ok(directory.path().to_owned()));
    let first = Arc::new(Mutex::new(Session::with_recent_maps(
        Err("no assets".into()),
        authority.clone(),
    )));
    let second = Arc::new(Mutex::new(Session::with_recent_maps(
        Err("no assets".into()),
        authority,
    )));
    let a = directory.path().join("a.vmf");
    let b = directory.path().join("b.vmf");
    for (session, path) in [(&first, &a), (&second, &b)] {
        std::fs::write(path, "world { id 1 classname worldspawn }").unwrap();
        lock(session).open(Actor::User, path, false).unwrap();
    }
    {
        let mut session = lock(&second);
        let stamp = session.measurement_stamp();
        session
            .preview_measurement(
                Actor::Agent,
                editor_core::session::measurements::Request {
                    id: None,
                    name: "Owned clearance".into(),
                    start: glam::DVec3::ZERO,
                    end: glam::DVec3::X,
                },
                &stamp,
            )
            .unwrap();
    }
    let source = lock(&second).document().vmf().clone();
    let history = lock(&second).history_snapshot();
    let draft = lock(&second).measurement_state()["draft"].clone();
    let camera = lock(&second).camera();
    let options = lock(&second).compile_options();
    let window = RecentMapsWindow::new().unwrap();
    bind(&window, &first);
    refresh(&window, &lock(&first));
    assert_eq!(window.get_rows().row_count(), 2);
    window.set_selected_path(b.display().to_string().into());
    window.invoke_remove_selected();
    assert_eq!(window.get_rows().row_count(), 1);
    assert_eq!(lock(&second).recent_maps_snapshot().entries.len(), 1);
    assert_eq!(lock(&second).path(), Some(b.as_path()));
    let revision = lock(&second).recent_maps_serial();
    let saved = std::fs::read(directory.path().join(crate::recent_maps::FILE_NAME)).unwrap();
    std::fs::write(
        directory.path().join(crate::recent_maps::FILE_NAME),
        b"broken",
    )
    .unwrap();
    window.invoke_refresh();
    assert!(!window.get_writable());
    assert!(lock(&second).recent_maps_serial() > revision);
    assert_eq!(lock(&second).recent_maps()["persistence"], "blocked");
    std::fs::write(directory.path().join(crate::recent_maps::FILE_NAME), saved).unwrap();
    window.invoke_refresh();
    assert!(window.get_writable());
    assert!(lock(&second).recent_maps()["error"].is_null());
    let session = lock(&second);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.measurement_state()["draft"], draft);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.compile_options(), options);
    window.invoke_cancel();
}
