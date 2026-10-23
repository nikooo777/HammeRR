use super::*;
use crate::runtime::lock;
use editor_core::{saved_defaults::SavedDefaults, session::TransformTool};
use glam::DVec3;
use std::sync::{Arc, Mutex};

#[test]
fn pivot_defaults_controls_share_errors_preserve_peers_and_reject_retired_callbacks() {
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
    let point = DVec3::new(1.125, -2.375, 3.0625);
    lock(&first)
        .set_transform_tool(
            Actor::User,
            TransformTool {
                pivot: Some(point),
                ..Default::default()
            },
        )
        .unwrap();
    left.show(&lock(&first)).unwrap();
    right.show(&lock(&second)).unwrap();
    left.window
        .window()
        .set_size(slint::PhysicalSize::new(760, 760));
    for label in [
        "Save current as defaults",
        "Apply saved defaults",
        "Reload defaults",
        "Reset saved defaults",
        "Close",
    ] {
        crate::ui::tests::assert_dialog_button_visible(&left.window, label);
    }
    left.window.invoke_save();
    assert!(left.window.get_defaults().contains("1.125"));
    assert_eq!(lock(&second).transform_tool().pivot, None);
    right.window.invoke_apply();
    assert!(right.window.get_message().contains("changed"));
    right.window.invoke_apply();
    assert_eq!(lock(&second).transform_tool().pivot, Some(point));
    lock(&first)
        .set_transform_tool(Actor::User, TransformTool::default())
        .unwrap();
    left.refresh(&lock(&first));
    assert!(left.window.get_current().contains("Selection center"));
    assert_eq!(lock(&second).transform_tool().pivot, Some(point));
    crate::ui::tests::save_ui_snapshot(left.window.window(), "transform-pivot-defaults");
    let path = directory
        .path()
        .join(editor_core::pivot_preferences::FILE_NAME);
    std::fs::write(&path, b"malformed").unwrap();
    left.window.invoke_reload();
    assert!(!left.window.get_ready());
    assert!(!left.window.get_status().is_empty());
    right.refresh(&lock(&second));
    assert!(!right.window.get_ready());
    left.window.invoke_save();
    assert!(left.window.get_message().contains("did not load"));
    assert_eq!(std::fs::read(&path).unwrap(), b"malformed");
    left.window.invoke_reset();
    assert!(left.window.get_ready());
    assert_eq!(lock(&second).transform_tool().pivot, Some(point));
    right.refresh(&lock(&second));
    right.window.invoke_apply();
    assert_eq!(lock(&second).transform_tool().pivot, None);
    let bytes = std::fs::read(&path).unwrap();
    first_document.shutdown().unwrap();
    left.window.invoke_reset();
    assert!(!left.window.get_message().is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    left.window.invoke_close_dialog();
    assert!(!left.window.window().is_visible());
    second_document.shutdown().unwrap();
}
