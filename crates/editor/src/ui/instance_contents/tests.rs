use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn software_controls_share_visibility_reports_frame_clear_and_preserve_parent() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (_directory, game, parent, _child) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &parent, true).unwrap();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera();
    let request = session
        .begin_instance_contents_refresh(Actor::User)
        .unwrap();
    let result = request.run();
    session
        .finish_instance_contents_refresh(Actor::User, result)
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let app = AppWindow::new().unwrap();
    super::super::bind_view_controls(&app, shared.clone());
    super::super::refresh_view_controls(&app, &lock(&shared));
    assert!(!app.get_instance_contents());
    let (serials, mut displayed_key, mut draw_options) = {
        let session = lock(&shared);
        (
            (session.scene_serial(), session.view_serial()),
            session.render_scene_key(session.view()),
            session.view(),
        )
    };
    app.set_instance_contents(true);
    app.invoke_options_changed();
    assert!(lock(&shared).view().instance_contents);
    {
        let mut session = lock(&shared);
        displayed_key =
            super::super::prepare_scene_refresh(&mut session, displayed_key, &mut draw_options)
                .expect("Reference-only enable must refresh the production viewport route");
        assert!(draw_options.instance_contents);
        assert!(
            !session
                .build_scene_for_view(draw_options)
                .reference_lines
                .is_empty()
        );
        assert_eq!((session.scene_serial(), session.view_serial()), serials);
        assert!(
            super::super::prepare_scene_refresh(&mut session, displayed_key, &mut draw_options,)
                .is_none()
        );
    }
    for enabled in [false, true] {
        let mut session = lock(&shared);
        let view = session
            .resolve_view_update(session::ViewUpdate {
                instance_contents: Some(enabled),
                ..Default::default()
            })
            .unwrap();
        session.set_view(view);
        displayed_key =
            super::super::prepare_scene_refresh(&mut session, displayed_key, &mut draw_options)
                .expect("Shared visibility changes must update retained draw options");
        assert_eq!(draw_options.instance_contents, enabled);
        let mesh = session.build_scene_for_view(draw_options);
        assert_eq!(!mesh.reference_lines.is_empty(), enabled);
        assert_eq!(
            session.instance_contents_rendered_report()["displayed"],
            enabled
        );
        assert_eq!((session.scene_serial(), session.view_serial()), serials);
    }
    let controller = Controller::new(shared.clone()).unwrap();
    assert!(
        controller
            .window
            .get_summary()
            .contains("1 instance from 1 file")
    );
    assert!(controller.window.get_owners().contains("child.vmf"));
    assert!(controller.window.get_can_frame());
    controller.window.show().unwrap();
    controller
        .window
        .window()
        .set_size(slint::PhysicalSize::new(850, 690));
    crate::ui::tests::save_ui_snapshot(controller.window.window(), "instance-contents");
    controller.window.invoke_frame_contents();
    assert_ne!(lock(&shared).camera(), camera);
    lock(&shared).set_camera(camera);
    controller.window.invoke_clear_contents();
    assert!(!controller.window.get_can_frame());
    assert!(lock(&shared).view().instance_contents);
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.camera(), camera);
}

#[test]
fn software_controls_show_refresh_failure_without_relocking_the_session() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("No game assets required".into()));
    session
        .create_entity(
            Actor::User,
            "func_instance",
            Some(DVec3::ZERO),
            BTreeMap::from([("file".into(), "child.vmf".into())]),
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(shared.clone()).unwrap();
    controller.window.invoke_refresh_contents();
    assert!(
        controller
            .window
            .get_message()
            .contains("Save the parent VMF")
    );
    assert!(!controller.window.get_loading());
    assert!(!controller.window.get_can_frame());
}

#[test]
fn fixed_contents_root_child_and_timer_reject_after_real_drain_and_release_owners() {
    i_slint_backend_testing::init_no_event_loop();
    let (directory, game, parent, _child) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session.open(Actor::User, &parent, true).unwrap();
    let request = session
        .begin_instance_contents_refresh(Actor::User)
        .unwrap();
    session
        .finish_instance_contents_refresh(Actor::User, request.run())
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    let app = AppWindow::new().unwrap();
    bind_controller(&app, controller.clone());
    app.invoke_inspect_instance_contents();
    assert!(controller.window.window().is_visible());
    let camera = lock(&shared).camera();
    controller.window.invoke_frame_contents();
    assert_ne!(lock(&shared).camera(), camera);
    controller.window.set_summary("refresh pending".into());
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    assert_ne!(controller.window.get_summary(), "refresh pending");
    controller.window.hide().unwrap();
    controller.window.set_summary("hidden unchanged".into());
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    assert_eq!(controller.window.get_summary(), "hidden unchanged");
    let mut peer = Session::new(Err("peer assets unavailable".into()));
    peer.open(Actor::User, &parent, true).unwrap();
    let peer_source = peer.document().vmf().clone();
    let peer_history = peer.history_snapshot();
    document.shutdown().unwrap();
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let camera = lock(&shared).camera();
    let state = lock(&shared).inspect_instance_contents(0, 64).unwrap();
    let offset = controller.offset.get();
    let token = controller.token.get();
    app.invoke_inspect_instance_contents();
    assert_eq!(app.get_message(), "the document is closed");
    assert!(!controller.window.window().is_visible());
    controller.window.invoke_refresh_contents();
    controller.window.invoke_cancel_refresh();
    controller.window.invoke_clear_contents();
    controller.window.invoke_frame_contents();
    controller.window.invoke_next();
    controller.window.invoke_previous();
    assert_eq!(controller.window.get_message(), "the document is closed");
    assert_eq!(controller.offset.get(), offset);
    assert_eq!(controller.token.get(), token);
    controller.window.show().unwrap();
    controller.window.set_message("terminal message".into());
    controller.window.set_summary("terminal summary".into());
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    assert_eq!(controller.window.get_message(), "terminal message");
    assert_eq!(controller.window.get_summary(), "terminal summary");
    controller.window.invoke_close_dialog();
    assert!(!controller.window.window().is_visible());
    assert_eq!(
        lock(&shared).inspect_instance_contents(0, 64).unwrap(),
        state
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(peer.document().vmf(), &peer_source);
    assert_eq!(peer.history_snapshot(), peer_history);
    let retained = controller.window.clone_strong();
    let weak_controller = Rc::downgrade(&controller);
    let weak_session = Arc::downgrade(&shared);
    drop(controller);
    drop(app);
    assert!(weak_controller.upgrade().is_none());
    drop(access);
    drop(shared);
    drop(document);
    assert!(weak_session.upgrade().is_none());
    retained.invoke_refresh_contents();
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    assert_eq!(retained.get_summary(), "terminal summary");
}

#[test]
fn instance_binders_preserve_failed_shutdown_error_and_timer_messages_without_retry() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let recovery = directory.path().join("recovery");
    let mut session = Session::new(Err("no assets required".into()));
    session.configure_recovery(Ok(recovery.clone()));
    session
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(16.0), "test", false)
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let app = AppWindow::new().unwrap();
    let _contents = bind_with_access(&app, &access);
    let _reference = super::super::instance_reference::bind_with_access(&app, &access);
    let _extraction = super::super::instance_extraction::bind_with_access(&app, &access);
    let controller = Controller::new_with_access(&access).unwrap();
    controller.window.show().unwrap();
    std::fs::write(&recovery, b"not a directory").unwrap();
    let error = document.shutdown().unwrap_err();
    let expected = format!("document shutdown failed: {error}");
    app.invoke_inspect_instance_contents();
    assert_eq!(app.get_message(), expected);
    app.invoke_create_instance_reference();
    assert_eq!(app.get_message(), expected);
    app.invoke_edit_instance_reference();
    assert_eq!(app.get_message(), expected);
    app.invoke_extract_instance();
    assert_eq!(app.get_message(), expected);
    controller.window.invoke_frame_contents();
    assert_eq!(controller.window.get_message(), expected);
    controller
        .window
        .set_message("preserve failed status".into());
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    assert_eq!(controller.window.get_message(), "preserve failed status");
    controller.window.invoke_close_dialog();
    assert!(!controller.window.window().is_visible());
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    assert_eq!(document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
}
