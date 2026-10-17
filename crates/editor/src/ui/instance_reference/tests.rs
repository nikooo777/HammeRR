use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn software_instance_reference_callbacks_share_creation_edits_and_expire_picker_results() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (_directory, game, parent, child) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &parent, true).unwrap();
    let source = session.document().vmf().clone();
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(shared.clone()).unwrap();
    controller.open(None);
    controller.complete_file(
        controller.token.get().unwrap(),
        controller.generation.get(),
        controller.edits.get(),
        controller.picker.get(),
        &child,
    );
    assert!(controller.window.get_ready());
    controller.window.set_file("child.vmf".into());
    controller.window.invoke_edited(0);
    controller.window.set_origin_x("80".into());
    controller.window.set_origin_y("16".into());
    controller.window.set_origin_z("8".into());
    controller.window.invoke_edited(1);
    controller.window.set_yaw("90".into());
    controller.window.invoke_edited(2);
    controller.window.set_origin_x("".into());
    controller.window.invoke_edited(1);
    assert_eq!(controller.window.get_origin_x(), "");
    assert_eq!(controller.window.get_origin_y(), "16");
    assert_eq!(controller.window.get_origin_z(), "8");
    assert!(!controller.window.get_ready());
    controller.window.set_origin_x("80".into());
    controller.window.invoke_edited(1);
    controller.window.invoke_check_file();
    assert!(controller.window.get_ready());
    assert!(
        controller
            .window
            .get_status()
            .contains("File present; contents unchecked")
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    controller
        .window
        .window()
        .set_size(slint::PhysicalSize::new(760, 690));
    crate::ui::tests::save_ui_snapshot(controller.window.window(), "instance-reference");
    let token = controller.token.get().unwrap();
    let generation = controller.generation.get();
    let edits = controller.edits.get();
    let picker = controller.picker.get();
    controller.window.set_file("typed.vmf".into());
    controller.window.invoke_edited(0);
    controller.complete_file(token, generation, edits, picker, &child);
    assert_eq!(controller.window.get_file(), "typed.vmf");
    assert!(controller.window.get_message().contains("expired"));
    controller.picker.set(picker.wrapping_add(1));
    controller.complete_file(token, generation, controller.edits.get(), picker, &child);
    assert_eq!(controller.window.get_file(), "typed.vmf");
    controller.complete_file(
        token,
        generation,
        controller.edits.get(),
        controller.picker.get(),
        &child,
    );
    assert_eq!(controller.window.get_file(), "child.vmf");
    controller.window.set_origin_x("9".repeat(513).into());
    controller.window.invoke_edited(1);
    assert_eq!(controller.window.get_origin_x(), "80");
    assert_eq!(controller.window.get_origin_y(), "16");
    assert_eq!(controller.window.get_origin_z(), "8");
    assert!(
        controller
            .window
            .get_message()
            .contains("rejected inputs were restored")
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    controller.window.invoke_apply();
    assert_eq!(lock(&shared).document().undo_labels().count(), 1);
    let ObjectId::Entity(id) = lock(&shared).selected_ids()[0] else {
        unreachable!()
    };
    assert_eq!(
        lock(&shared).document().entity(id).unwrap().origin(),
        Some(DVec3::new(80.0, 16.0, 8.0))
    );
    assert_eq!(lock(&shared).camera(), camera);
    controller.open(Some(id));
    let old = (
        controller.token.get().unwrap(),
        controller.generation.get(),
        controller.edits.get(),
        controller.picker.get(),
    );
    controller.window.invoke_cancel();
    controller.open(Some(id));
    controller.window.set_file("replacement.vmf".into());
    controller.window.invoke_edited(0);
    controller.complete_file(old.0, old.1, old.2, old.3, &child);
    assert_eq!(controller.window.get_file(), "replacement.vmf");
    controller.window.invoke_cancel();
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
}

#[test]
fn fixed_reference_root_child_picker_and_timer_reject_terminal_actions_without_retargeting() {
    i_slint_backend_testing::init_no_event_loop();
    let (directory, game, parent, child) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session.open(Actor::User, &parent, true).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    let app = AppWindow::new().unwrap();
    bind_controller(&app, controller.clone());
    app.invoke_create_instance_reference();
    assert!(controller.window.window().is_visible());
    let token = controller.token.get().unwrap();
    controller.complete_file(
        token,
        controller.generation.get(),
        controller.edits.get(),
        controller.picker.get(),
        &child,
    );
    assert_eq!(controller.window.get_file(), "child.vmf");
    controller.window.set_status("refresh pending".into());
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    assert_ne!(controller.window.get_status(), "refresh pending");
    let mut peer = Session::new(Err("peer assets unavailable".into()));
    peer.open(Actor::User, &parent, true).unwrap();
    peer.begin_instance_reference(Actor::User, None).unwrap();
    let peer_form = peer.get_instance_reference();
    let peer_source = peer.document().vmf().clone();
    document.shutdown().unwrap();
    let form = lock(&shared).get_instance_reference();
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let camera = lock(&shared).camera();
    let counters = (
        controller.generation.get(),
        controller.edits.get(),
        controller.picker.get(),
    );
    let file = controller.window.get_file();
    controller.window.invoke_edited(0);
    controller.window.invoke_check_file();
    controller.window.invoke_review();
    controller.window.invoke_refresh_contents();
    controller.window.invoke_apply();
    controller.window.invoke_browse();
    controller.complete_file(
        token,
        counters.0,
        counters.1,
        counters.2,
        &directory.path().join("never-read.vmf"),
    );
    assert_eq!(controller.window.get_message(), "the document is closed");
    assert_eq!(controller.window.get_file(), file);
    assert_eq!(
        (
            controller.generation.get(),
            controller.edits.get(),
            controller.picker.get()
        ),
        counters
    );
    controller.window.set_message("terminal message".into());
    controller.window.set_status("terminal form".into());
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    assert_eq!(controller.window.get_message(), "terminal message");
    assert_eq!(controller.window.get_status(), "terminal form");
    controller.window.invoke_cancel();
    assert!(!controller.window.window().is_visible());
    app.invoke_create_instance_reference();
    app.invoke_edit_instance_reference();
    assert_eq!(app.get_message(), "the document is closed");
    assert!(!controller.window.window().is_visible());
    assert_eq!(lock(&shared).get_instance_reference(), form);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(peer.get_instance_reference(), peer_form);
    assert_eq!(peer.document().vmf(), &peer_source);
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
    retained.invoke_apply();
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    assert_eq!(retained.get_status(), "terminal form");
}
