use super::*;
use crate::runtime::lock;
use std::sync::{Arc, Mutex};

fn click(window: &ColorCorrectionWindow, label: &str) {
    window.window().take_snapshot().unwrap();
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(window, label)
        .next()
        .unwrap()
        .invoke_accessible_default_action();
}

#[test]
fn software_controls_load_apply_reject_invalid_weight_and_clear_the_shared_lut() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/color-correction-tests");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("ui-")
        .tempdir_in(root)
        .unwrap();
    let path = directory.path().join("preview.raw");
    std::fs::write(&path, vec![37; render::color_correction::RAW_BYTES]).unwrap();
    let shared = Arc::new(Mutex::new(Session::new(Err(
        "fixture content unavailable".into()
    ))));
    let before = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let controller =
        Controller::new_with_access(&DocumentAccess::unmanaged(shared.clone())).unwrap();
    controller.show(&mut lock(&shared)).unwrap();
    lock(&shared).load_color_correction(&path).unwrap();
    let unreviewed = lock(&shared).color_correction_state();
    controller.window.set_preview_enabled(true);
    controller.window.set_weight("0.125".into());
    click(&controller.window, "Apply preview");
    assert!(controller.window.get_message().contains("review"));
    assert_eq!(lock(&shared).color_correction_state(), unreviewed);
    assert!(!controller.window.get_preview_enabled());
    controller.window.set_source(path.to_str().unwrap().into());
    click(&controller.window, "Load LUT");
    assert!(controller.window.get_message().is_empty());
    assert_eq!(lock(&shared).color_correction_state()["ready"], true);
    assert!(!controller.window.get_preview_enabled());
    controller.window.set_preview_enabled(true);
    controller.window.set_weight("0.25".into());
    click(&controller.window, "Apply preview");
    assert_eq!(lock(&shared).color_correction_state()["weight"], 0.25);
    assert_eq!(lock(&shared).color_correction_state()["enabled"], true);
    lock(&shared).load_color_correction(&path).unwrap();
    click(&controller.window, "Apply preview");
    assert!(!controller.window.get_message().is_empty());
    assert_eq!(lock(&shared).color_correction_state()["enabled"], false);
    controller.window.set_preview_enabled(true);
    controller.window.set_weight("0.25".into());
    click(&controller.window, "Apply preview");
    assert!(controller.window.get_message().is_empty());
    controller.window.set_weight("NaN".into());
    click(&controller.window, "Apply preview");
    assert!(!controller.window.get_message().is_empty());
    assert_eq!(lock(&shared).color_correction_state()["weight"], 0.25);
    std::fs::remove_file(&path).unwrap();
    click(&controller.window, "Apply preview");
    assert!(!controller.window.get_message().is_empty());
    assert_eq!(lock(&shared).color_correction_state()["ready"], false);
    assert!(lock(&shared).color_correction_for_render().is_err());
    click(&controller.window, "Clear LUT");
    assert_eq!(lock(&shared).color_correction_state()["enabled"], false);
    assert!(controller.window.get_source().is_empty());
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(lock(&shared).history_snapshot(), history);
    click(&controller.window, "Close");
    assert!(!controller.window.window().is_visible());
}
