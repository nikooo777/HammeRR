use super::*;
use std::sync::{Arc, Mutex};

fn wait_for_collapse(controller: &Controller, shared: &SharedSession) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while lock(shared).get_instance_collapse()["worker_active"] == true {
        assert!(
            std::time::Instant::now() < deadline,
            "collapse worker did not finish"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    controller.refresh_admitted(shared);
}

#[test]
fn software_instance_collapse_callbacks_preview_cancel_apply_and_undo_the_shared_draft() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (_directory, _game, parent, child) = crate::test_assets::instance_map(false);
    let (_assets, game) = crate::test_assets::model_lighting::game();
    let mut content =
        source_formats::vmf::Vmf::parse(&std::fs::read_to_string(&child).unwrap()).unwrap();
    let mut light = source_formats::vmf::Entity::new(1000, "light");
    light.properties.set("origin", "0 0 96");
    light.properties.set("_light", "255 160 64 240");
    content.entities.push(light);
    let mut cube = source_formats::vmf::Entity::new(1001, "env_cubemap");
    cube.properties.set("origin", "0 0 80");
    cube.properties.set("cubemapsize", "1");
    cube.properties
        .set("sides", content.world.solids[0].sides[0].id.to_string());
    content.entities.push(cube);
    let mut prop = source_formats::vmf::Entity::new(1002, "prop_static");
    prop.properties.set("origin", "16 8 32");
    prop.properties.set("model", "models/test/receiver.mdl");
    prop.properties.set("solid", "0");
    prop.properties.set("skin", "1");
    content.entities.push(prop);
    std::fs::write(child, content.write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &parent, true).unwrap();
    let owner = session
        .document()
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("func_instance"))
        .unwrap()
        .id;
    let source = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(shared.clone()).unwrap();
    controller.open(Some(owner));
    assert!(controller.window.get_can_collapse());
    assert_eq!(controller.window.get_collapse_scope(), 0);
    controller.window.invoke_preview_collapse();
    wait_for_collapse(&controller, &shared);
    assert!(controller.window.get_collapse_active());
    assert!(controller.window.get_ready());
    assert!(controller.window.get_contents().contains("1 point lights"));
    assert!(controller.window.get_contents().contains("1 cubemaps"));
    assert!(controller.window.get_contents().contains("1 static props"));
    assert_eq!(lock(&shared).document().vmf(), &source);
    controller
        .window
        .window()
        .set_size(slint::PhysicalSize::new(800, 950));
    crate::ui::tests::assert_ui_renders(controller.window.window(), "instance-collapse-review");
    controller.window.invoke_cancel();
    assert_eq!(lock(&shared).render_document().vmf(), &source);
    let retained = lock(&shared)
        .edit(Actor::User, "retain unselected reference", |tx| {
            let id = tx.new_object_id()?;
            let mut reference = source_formats::vmf::Entity::new(id, "func_instance");
            reference.properties.set("file", "missing-unselected.vmf");
            tx.add_entity(reference.clone())?;
            Ok(reference)
        })
        .unwrap();
    lock(&shared)
        .select(&[ObjectId::Entity(owner)], SelectMode::Replace)
        .unwrap();
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).document().undo_labels().count();
    controller.open(Some(owner));
    controller.window.set_collapse_scope(1);
    controller.window.invoke_preview_collapse();
    wait_for_collapse(&controller, &shared);
    assert_eq!(
        lock(&shared).get_instance_collapse()["scope"],
        "selected_leading_references"
    );
    assert!(
        controller
            .window
            .get_contents()
            .contains("Selected leading references")
    );
    crate::ui::tests::assert_ui_renders(
        controller.window.window(),
        "instance-collapse-selected-review",
    );
    let shown = lock(&shared).render_document().vmf().clone();
    assert_eq!(
        shown
            .entities
            .iter()
            .filter(|entity| entity.classname() == Some("light"))
            .count(),
        1
    );
    assert_eq!(
        shown
            .entities
            .iter()
            .find(|entity| entity.id == retained.id),
        Some(&retained)
    );
    controller.window.set_collapse_scope(0);
    controller.window.invoke_apply();
    assert!(controller.window.get_message().contains("scope changed"));
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(controller.window.get_collapse_scope(), 1);
    controller.window.invoke_apply();
    wait_for_collapse(&controller, &shared);
    assert_eq!(lock(&shared).document().vmf(), &shown);
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    controller.stop_timer();
}

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
    crate::ui::tests::assert_ui_renders(controller.window.window(), "instance-reference");
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
