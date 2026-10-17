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
        .set_size(slint::PhysicalSize::new(760, 530));
    crate::ui::tests::save_ui_snapshot(controller.window.window(), "instance-extraction");
    controller.window.invoke_extract();
    wait(&controller);
    assert!(directory.path().join("software.vmf").exists());
    assert_eq!(lock(&shared).document().undo_labels().count(), 1);
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
    assert!(lock(&shared).get_instance_extraction()["token"].is_number());
    controller.window.invoke_review();
    assert!(controller.window.get_owned());
    controller.window.invoke_cancel();
    assert!(lock(&shared).get_instance_extraction()["token"].is_null());
    assert!(!directory.path().join("agent.vmf").exists());
}
