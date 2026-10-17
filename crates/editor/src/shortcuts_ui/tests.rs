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

#[test]
fn shortcut_controls_save_atomically_report_conflicts_and_preserve_text_field_editing() {
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
    assert_eq!(lock(&shared).shortcut_bindings(), &Bindings::default());
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
    assert_eq!(lock(&shared).shortcut_bindings(), &Bindings::default());
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
    assert_eq!(lock(&shared).shortcut_bindings(), &Bindings::default());
    controller.window.invoke_reset();
    assert!(lock(&shared).shortcuts_error_message().is_none());
    controller.window.invoke_cancel();
    assert!(!controller.window.window().is_visible());

    let app = AppWindow::new().unwrap();
    app.on_key({
        let (app, shared) = (app.as_weak(), shared.clone());
        move |text, down, control, shift, alt, meta| {
            down && dispatch_key(
                &app.unwrap(),
                &mut lock(&shared),
                &text,
                control,
                shift,
                alt,
                meta,
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
