use super::*;
use std::sync::{Arc, Mutex};

fn wait(controller: &Controller) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        controller.refresh();
        if !controller.window.get_busy() {
            return;
        }
        assert!(Instant::now() < deadline, "extraction worker timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn software_instance_extraction_callbacks_prepare_review_extract_cancel_and_undo() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game, parent, ids) = crate::test_assets::instance_extraction_map();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &parent, true).unwrap();
    session.select(&ids, SelectMode::Replace).unwrap();
    let original = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(shared.clone()).unwrap();
    controller.window.set_file("software.vmf".into());
    controller.window.set_pivot_mode(2);
    controller.window.set_pivot_x("12.375".into());
    controller.window.set_pivot_y("-6.625".into());
    controller.window.set_pivot_z("2.125".into());
    controller.window.invoke_prepare();
    wait(&controller);
    assert!(
        controller.window.get_ready(),
        "{}",
        controller.window.get_summary()
    );
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert!(!directory.path().join("software.vmf").exists());
    controller
        .window
        .window()
        .set_size(slint::PhysicalSize::new(760, 650));
    crate::ui::tests::assert_ui_renders(controller.window.window(), "instance-extraction");
    assert!(
        controller
            .window
            .get_summary()
            .contains("Custom · 12.375 -6.625 2.125")
    );
    controller.window.set_pivot_x("13.375".into());
    controller.window.invoke_extract();
    assert!(controller.window.get_message().contains("Pivot changed"));
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert!(!directory.path().join("software.vmf").exists());
    assert_eq!(controller.window.get_pivot_x(), "12.375");
    controller.window.invoke_extract();
    wait(&controller);
    assert!(directory.path().join("software.vmf").exists());
    assert_eq!(lock(&shared).document().undo_labels().count(), 1);
    let helper = lock(&shared)
        .document()
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("func_instance"))
        .cloned()
        .unwrap();
    assert_eq!(helper.origin(), Some(DVec3::new(12.375, -6.625, 2.125)));
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    lock(&shared).select(&ids, SelectMode::Replace).unwrap();
    let request = lock(&shared)
        .begin_instance_extraction(Actor::Agent, "agent.vmf")
        .unwrap();
    let completed = request.run();
    lock(&shared)
        .finish_instance_extraction(Actor::Agent, completed)
        .unwrap();
    controller.refresh();
    assert!(!controller.window.get_owned());
    controller.window.invoke_cancel();
    assert!(!controller.window.window().is_visible());
    assert!(lock(&shared).get_instance_extraction()["token"].is_number());
    controller.window.invoke_review();
    assert!(controller.window.get_owned());
    controller.window.invoke_cancel();
    assert!(lock(&shared).get_instance_extraction()["token"].is_null());
    assert!(!directory.path().join("agent.vmf").exists());
    controller.window.set_file("centre.vmf".into());
    controller.window.set_pivot_mode(1);
    controller.window.invoke_prepare();
    wait(&controller);
    assert!(
        controller.window.get_ready(),
        "{}",
        controller.window.get_message()
    );
    assert!(
        controller
            .window
            .get_summary()
            .contains("Selection centre · 60 48 16")
    );
    controller.window.invoke_cancel();
    assert!(!directory.path().join("centre.vmf").exists());
    controller.window.set_file("invalid-pivot.vmf".into());
    controller.window.set_pivot_mode(2);
    controller.window.set_pivot_x("NaN".into());
    controller.window.invoke_prepare();
    assert!(
        controller
            .window
            .get_message()
            .contains("finite X, Y and Z")
    );
    assert!(lock(&shared).get_instance_extraction()["token"].is_null());
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert!(!directory.path().join("invalid-pivot.vmf").exists());
}

#[test]
fn fixed_extraction_root_child_and_timer_reject_after_physical_cleanup_and_release_owners() {
    i_slint_backend_testing::init_no_event_loop();
    let (directory, game, parent, ids) = crate::test_assets::instance_extraction_map();
    let mut session = Session::new(Ok(game));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session.open(Actor::User, &parent, true).unwrap();
    session.select(&ids, SelectMode::Replace).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    let app = AppWindow::new().unwrap();
    bind_controller(&app, controller.clone());
    app.invoke_extract_instance();
    assert!(controller.window.window().is_visible());
    controller.window.set_file("admitted.vmf".into());
    controller.window.invoke_prepare();
    wait(&controller);
    assert!(
        controller.window.get_ready(),
        "{}",
        controller.window.get_summary()
    );
    controller.window.set_summary("refresh pending".into());
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    assert_ne!(controller.window.get_summary(), "refresh pending");
    let mut peer = Session::new(Err("peer assets unavailable".into()));
    peer.open(Actor::User, &parent, true).unwrap();
    let peer_source = peer.document().vmf().clone();
    let peer_history = peer.history_snapshot();
    document.shutdown().unwrap();
    let report = lock(&shared).get_instance_extraction();
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let camera = lock(&shared).camera();
    let token = controller.token.get();
    controller.window.invoke_prepare();
    controller.window.invoke_review();
    controller.window.invoke_extract();
    assert_eq!(controller.window.get_message(), "the document is closed");
    assert_eq!(controller.token.get(), token);
    controller.window.set_message("terminal message".into());
    controller.window.set_summary("terminal summary".into());
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    assert_eq!(controller.window.get_message(), "terminal message");
    assert_eq!(controller.window.get_summary(), "terminal summary");
    controller.window.invoke_cancel();
    assert!(!controller.window.window().is_visible());
    app.invoke_extract_instance();
    assert_eq!(app.get_message(), "the document is closed");
    assert!(!controller.window.window().is_visible());
    assert_eq!(lock(&shared).get_instance_extraction(), report);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(peer.document().vmf(), &peer_source);
    assert_eq!(peer.history_snapshot(), peer_history);
    assert!(!directory.path().join("admitted.vmf").exists());
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
    retained.invoke_prepare();
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    assert_eq!(retained.get_summary(), "terminal summary");
}
