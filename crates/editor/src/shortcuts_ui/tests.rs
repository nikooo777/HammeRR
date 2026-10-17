use super::*;
use glam::DVec3;
use slint::{
    Model,
    platform::{PointerEventButton, WindowEvent},
};
use std::sync::{Arc, Mutex};

fn key_index(key: &str) -> i32 {
    keys().iter().position(|value| value == key).unwrap() as i32
}

fn software_backend() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
}

#[test]
fn attached_shortcut_forms_and_viewport_hints_observe_shared_activation_errors_and_reset() {
    software_backend();
    let directory = tempfile::tempdir().unwrap();
    let authority = editor_core::shortcuts::Authority::configured(Ok(directory.path().to_owned()));
    let first = Arc::new(Mutex::new(Session::with_keyboard_shortcuts(
        Err("no assets".into()),
        authority.clone(),
    )));
    let mut second_session = Session::with_keyboard_shortcuts(Err("no assets".into()), authority);
    second_session
        .create_block(
            Actor::User,
            DVec3::splat(1000.),
            DVec3::splat(1064.),
            "second",
            true,
        )
        .unwrap();
    let second = Arc::new(Mutex::new(second_session));
    let first_form = Controller::new(&first).unwrap();
    let second_form = Controller::new(&second).unwrap();
    first_form.show(&lock(&first)).unwrap();
    second_form.show(&lock(&second)).unwrap();
    first_form.window.invoke_choose("frame_selection".into());
    first_form.window.set_primary_key(key_index("F8"));
    first_form.window.set_primary_control(false);
    first_form.window.set_primary_shift(false);
    first_form.window.invoke_edited();
    first_form.window.invoke_save();
    second_form.refresh(&lock(&second));
    assert!(second_form.window.get_status().contains("another editor"));
    let app = AppWindow::new().unwrap();
    app.show().unwrap();
    app.window().set_size(slint::PhysicalSize::new(1600, 1000));
    app.window().take_snapshot().unwrap();
    refresh_hint(&app, &lock(&second));
    assert_eq!(app.get_frame_shortcut(), "F8");
    assert!(!app.get_shortcuts_error());
    let first_camera = lock(&first).camera();
    let second_camera = lock(&second).camera();
    assert!(route_viewport_key(
        &app,
        &second,
        ViewportKey {
            text: &key_text(Key::F8),
            down: true,
            control: false,
            shift: false,
            alt: false,
            meta: false
        },
        |_, _, _| false,
        |_, _| false
    ));
    assert_eq!(lock(&first).camera(), first_camera);
    assert_ne!(lock(&second).camera(), second_camera);
    std::fs::write(
        directory.path().join(editor_core::shortcuts::FILE_NAME),
        b"broken",
    )
    .unwrap();
    second_form.window.invoke_reload();
    first_form.refresh(&lock(&first));
    refresh_hint(&app, &lock(&first));
    assert!(app.get_shortcuts_error());
    assert_eq!(app.get_frame_shortcut(), "F8");
    assert!(
        first_form
            .window
            .get_status()
            .contains("Saved shortcuts unavailable")
    );
    first_form.window.invoke_reset();
    second_form.refresh(&lock(&second));
    refresh_hint(&app, &lock(&second));
    assert!(!app.get_shortcuts_error());
    assert_eq!(app.get_frame_shortcut(), "F");
    assert_eq!(lock(&second).shortcut_bindings(), Bindings::default());
}

fn dispatch_chord(app: &AppWindow, modifiers: &[Key], text: &str) {
    for key in modifiers {
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: key_text(*key).into(),
        });
    }
    app.window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.into() });
    app.window()
        .dispatch_event(WindowEvent::KeyReleased { text: text.into() });
    for key in modifiers.iter().rev() {
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: key_text(*key).into(),
        });
    }
}

#[test]
fn shortcut_controls_save_atomically_report_conflicts_and_preserve_text_field_editing() {
    software_backend();
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().to_owned()));
    let id = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "test/material",
            false,
        )
        .unwrap();
    let before = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    controller.show(&lock(&shared)).unwrap();
    controller
        .window
        .window()
        .set_size(slint::PhysicalSize::new(900, 900));
    assert_eq!(controller.window.get_commands().row_count(), 5);
    controller.window.set_primary_key(key_index("Y"));
    controller.window.set_primary_control(true);
    controller.window.invoke_edited();
    assert!(!controller.window.get_can_save());
    assert!(
        controller
            .window
            .get_status()
            .contains("assigned more than once")
    );
    controller.window.set_primary_key(key_index("F6"));
    controller.window.set_primary_control(false);
    controller.window.set_primary_shift(true);
    controller.window.invoke_edited();
    assert!(controller.window.get_can_save());
    assert_eq!(lock(&shared).shortcut_bindings(), Bindings::default());
    controller.window.invoke_save();
    assert_eq!(
        lock(&shared).resolve_editor_shortcut("F6", false, true, false, false),
        Some(Command::Undo)
    );
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert!(!controller.window.get_can_save());
    for label in [
        "Save shortcuts",
        "Cancel",
        "Reload saved",
        "Reset saved shortcuts",
    ] {
        crate::ui::tests::assert_dialog_button_visible(&controller.window, label);
    }
    crate::ui::tests::save_ui_snapshot(controller.window.window(), "keyboard-shortcuts");
    controller.window.set_primary_key(key_index("F7"));
    controller.window.invoke_edited();
    {
        let mut session = lock(&shared);
        let snapshot = session.shortcuts_stamp();
        session
            .change_keyboard_shortcuts(Actor::Agent, &snapshot, "reset", None)
            .unwrap();
    }
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_can_save());
    assert!(controller.window.get_status().contains("another editor"));
    controller.window.invoke_save();
    assert_eq!(lock(&shared).shortcut_bindings(), Bindings::default());
    controller.window.invoke_reload();
    assert_eq!(controller.window.get_primary_key(), key_index("Z"));
    std::fs::write(
        directory.path().join(editor_core::shortcuts::FILE_NAME),
        b"malformed",
    )
    .unwrap();
    controller.window.invoke_reload();
    assert!(!controller.window.get_can_save());
    assert!(
        controller
            .window
            .get_status()
            .contains("Saved shortcuts unavailable")
    );
    assert_eq!(lock(&shared).shortcut_bindings(), Bindings::default());
    controller.window.invoke_reset();
    assert!(lock(&shared).shortcuts_error_message().is_none());
    controller.window.invoke_cancel();
    assert!(!controller.window.window().is_visible());

    let app = AppWindow::new().unwrap();
    app.on_key({
        let (app, shared) = (app.as_weak(), shared.clone());
        move |text, down, control, shift, alt, meta| {
            route_viewport_key(
                &app.unwrap(),
                &shared,
                ViewportKey {
                    text: &text,
                    down,
                    control,
                    shift,
                    alt,
                    meta,
                },
                |_, _, _| false,
                |_, _| false,
            )
        }
    });
    app.show().unwrap();
    app.window().set_size(slint::PhysicalSize::new(1600, 1000));
    app.window().take_snapshot().unwrap();
    let grid = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &app,
        "Viewport grid size",
    )
    .next()
    .unwrap();
    grid.mock_single_click(PointerEventButton::Left);
    grid.set_accessible_value("16");
    grid.set_accessible_selection_offsets(0, 2);
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Delete.into(),
    });
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Delete.into(),
    });
    assert_eq!(app.get_grid_size(), "");
    app.window()
        .dispatch_event(WindowEvent::KeyPressed { text: "3".into() });
    app.window()
        .dispatch_event(WindowEvent::KeyReleased { text: "3".into() });
    app.window()
        .dispatch_event(WindowEvent::KeyPressed { text: "2".into() });
    app.window()
        .dispatch_event(WindowEvent::KeyReleased { text: "2".into() });
    assert_eq!(app.get_grid_size(), "32");
    for (down, text) in [
        (true, key_text(Key::Control)),
        (true, "z".into()),
        (false, "z".into()),
        (false, key_text(Key::Control)),
    ] {
        app.window().dispatch_event(if down {
            WindowEvent::KeyPressed { text: text.into() }
        } else {
            WindowEvent::KeyReleased { text: text.into() }
        });
    }
    assert_eq!(lock(&shared).document().vmf(), &before);
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: slint::LogicalPosition::new(600.0, 400.0),
        button: PointerEventButton::Left,
    });
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: slint::LogicalPosition::new(600.0, 400.0),
        button: PointerEventButton::Left,
    });
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Delete.into(),
    });
    assert!(!lock(&shared).document().contains(id));
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert!(!app.invoke_key("z".into(), true, true, false, true, false));
    assert_eq!(lock(&shared).document().vmf(), &before);
    app.hide().unwrap();
}

#[test]
fn viewport_key_router_guards_fixed_commands_navigation_and_keeps_releases() {
    software_backend();
    let mut session = Session::new(Err("no assets needed".into()));
    let id = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "test/material",
            false,
        )
        .unwrap();
    let before = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let app = AppWindow::new().unwrap();
    let fixed = Rc::new(RefCell::new(Vec::new()));
    let navigation = Rc::new(RefCell::new(Vec::new()));
    app.on_key({
        let (app, shared, fixed, navigation) = (
            app.as_weak(),
            shared.clone(),
            fixed.clone(),
            navigation.clone(),
        );
        move |text, down, control, shift, alt, meta| {
            route_viewport_key(
                &app.unwrap(),
                &shared,
                ViewportKey {
                    text: &text,
                    down,
                    control,
                    shift,
                    alt,
                    meta,
                },
                |text, control, shift| {
                    assert!(shared.try_lock().is_ok());
                    fixed.borrow_mut().push((text.to_owned(), control, shift));
                    (control && text.eq_ignore_ascii_case("s"))
                        || text == key_text(Key::UpArrow)
                        || text == key_text(Key::Escape)
                },
                |text, down| {
                    assert!(shared.try_lock().is_ok());
                    navigation.borrow_mut().push((text.to_owned(), down));
                    text == "w" || text == key_text(Key::Shift)
                },
            )
        }
    });

    let up = key_text(Key::UpArrow);
    for (text, control, alt, meta) in [
        ("s", true, true, false),
        ("s", true, false, true),
        (up.as_str(), false, true, false),
        (up.as_str(), false, false, true),
        ("w", false, true, false),
        ("w", false, false, true),
        ("z", true, true, false),
        ("z", true, false, true),
    ] {
        assert!(!app.invoke_key(text.into(), true, control, false, alt, meta));
    }
    assert!(fixed.borrow().is_empty());
    assert!(navigation.borrow().is_empty());
    assert_eq!(lock(&shared).document().vmf(), &before);

    for shift in [false, true] {
        assert!(!app.invoke_key("w".into(), true, true, shift, false, false));
    }
    assert!(navigation.borrow().is_empty());
    fixed.borrow_mut().clear();
    assert!(app.invoke_key("s".into(), true, true, true, false, false));
    assert!(app.invoke_key(up.clone().into(), true, false, true, false, false));
    assert!(app.invoke_key(
        key_text(Key::Escape).into(),
        true,
        false,
        true,
        false,
        false
    ));
    assert_eq!(
        *fixed.borrow(),
        vec![
            ("s".into(), true, true),
            (up, false, true),
            (key_text(Key::Escape), false, true),
        ]
    );
    assert!(navigation.borrow().is_empty());
    fixed.borrow_mut().clear();

    assert!(app.invoke_key(
        key_text(Key::Delete).into(),
        true,
        false,
        false,
        false,
        false
    ));
    assert!(!lock(&shared).document().contains(id));
    assert!(app.invoke_key("z".into(), true, true, false, false, false));
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert!(fixed.borrow().is_empty());
    assert!(navigation.borrow().is_empty());

    for shift in [false, true] {
        assert!(app.invoke_key("w".into(), true, false, shift, false, false));
        assert!(app.invoke_key("w".into(), false, true, shift, true, true));
    }
    assert_eq!(
        *navigation.borrow(),
        vec![
            ("w".into(), true),
            ("w".into(), false),
            ("w".into(), true),
            ("w".into(), false),
        ]
    );
    assert_eq!(fixed.borrow().len(), 2);

    let opens = Rc::new(std::cell::Cell::new(0));
    app.on_open_map({
        let opens = opens.clone();
        move || opens.set(opens.get() + 1)
    });
    app.show().unwrap();
    app.window().set_size(slint::PhysicalSize::new(1600, 1000));
    app.window().take_snapshot().unwrap();
    for down in [true, false] {
        let position = slint::LogicalPosition::new(600.0, 400.0);
        let button = PointerEventButton::Left;
        app.window().dispatch_event(if down {
            WindowEvent::PointerPressed { position, button }
        } else {
            WindowEvent::PointerReleased { position, button }
        });
    }
    dispatch_chord(&app, &[Key::Control, Key::Alt], "o");
    dispatch_chord(&app, &[Key::Control, Key::Meta], "o");
    assert_eq!(opens.get(), 0);
    dispatch_chord(&app, &[Key::Control], "o");
    assert_eq!(opens.get(), 1);
    assert_eq!(lock(&shared).document().vmf(), &before);
    app.hide().unwrap();
}
