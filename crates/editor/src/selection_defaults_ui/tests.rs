use super::*;
use crate::runtime::lock;
use editor_core::{saved_defaults::SavedDefaults, session::selection_filters::SelectionFilters};
use slint::Model;
use std::sync::{Arc, Mutex};

#[test]
fn selection_defaults_controls_share_saved_errors_and_apply_only_the_originating_document() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let defaults = SavedDefaults::configured(Ok(directory.path().into()));
    let first = Arc::new(Mutex::new(Session::with_saved_defaults(
        Err("no assets".into()),
        defaults.clone(),
    )));
    let second = Arc::new(Mutex::new(Session::with_saved_defaults(
        Err("no assets".into()),
        defaults,
    )));
    let first_document = crate::test_assets::document(first.clone());
    let second_document = crate::test_assets::document(second.clone());
    let left =
        Controller::new_with_access(&DocumentAccess::for_document(first_document.clone())).unwrap();
    let right = Controller::new_with_access(&DocumentAccess::for_document(second_document.clone()))
        .unwrap();
    let filters = SelectionFilters {
        world_brushes: false,
        ..Default::default()
    };
    lock(&first)
        .set_selection_filters(Actor::User, filters)
        .unwrap();
    left.show(&lock(&first)).unwrap();
    right.show(&lock(&second)).unwrap();
    left.window
        .window()
        .set_size(slint::PhysicalSize::new(760, 680));
    for label in [
        "Save current as defaults",
        "Apply saved defaults",
        "Reload defaults",
        "Reset saved defaults",
        "Close",
    ] {
        crate::ui::tests::assert_dialog_button_visible(&left.window, label);
    }
    crate::ui::tests::save_ui_snapshot(left.window.window(), "selection-defaults");
    left.window.invoke_save();
    assert_eq!(left.window.get_rows().row_data(0).unwrap().defaults, "Off");
    right.window.invoke_apply();
    assert!(right.window.get_message().contains("changed"));
    assert!(lock(&second).selection_filters().world_brushes);
    right.window.invoke_apply();
    assert_eq!(lock(&second).selection_filters(), filters);
    lock(&first)
        .set_selection_filters(Actor::User, SelectionFilters::default())
        .unwrap();
    let path = directory
        .path()
        .join(editor_core::selection_preferences::FILE_NAME);
    std::fs::write(&path, b"malformed selection defaults").unwrap();
    right.window.invoke_reload();
    assert!(!right.window.get_ready());
    assert!(!right.window.get_message().is_empty());
    left.refresh(&lock(&first));
    assert!(!left.window.get_ready());
    assert_eq!(
        left.window.get_rows().row_data(0).unwrap().defaults,
        "Unavailable"
    );
    right.window.invoke_save();
    assert!(right.window.get_message().contains("did not load"));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"malformed selection defaults"
    );
    right.window.invoke_reset();
    assert!(right.window.get_ready());
    assert_eq!(lock(&second).selection_filters(), filters);
    left.refresh(&lock(&first));
    assert!(left.window.get_ready());
    assert_eq!(left.window.get_rows().row_data(0).unwrap().defaults, "On");
    right.window.invoke_apply();
    assert_eq!(
        lock(&second).selection_filters(),
        SelectionFilters::default()
    );
    left.window.invoke_close_dialog();
    right.window.invoke_close_dialog();
    assert!(!left.window.window().is_visible() && !right.window.window().is_visible());
    let before = lock(&first).selection_preferences_state();
    let bytes = std::fs::read(&path).unwrap();
    first_document.shutdown().unwrap();
    left.window.invoke_reset();
    assert_eq!(left.window.get_message(), "the document is closed");
    assert_eq!(lock(&first).selection_preferences_state(), before);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    second_document.shutdown().unwrap();
}
