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
