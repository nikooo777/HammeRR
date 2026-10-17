use super::*;
use crate::runtime::lock;
use slint::Model;
use std::sync::{Arc, Mutex};

#[test]
fn preview_defaults_window_compares_saves_applies_and_recovers_through_session_callbacks() {
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
    session.configure_recovery(Ok(directory.path().into()));
    let view = session.view();
    session.set_view(render::ViewOptions {
        selection_outlines: false,
        entity_images: false,
        entity_model_guides: true,
        rope_preview: true,
        sky_preview: true,
        sky_channel: render::sky::Channel::Hdr,
        sky_exposure_ev: 1.5,
        ..view
    });
    let source = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    let window = &controller.window;
    controller.show(&lock(&shared)).unwrap();
    window.window().set_size(slint::PhysicalSize::new(760, 760));
    assert!(window.get_ready());
    assert_eq!(window.get_rows().row_count(), 5);
    let outlines = window.get_rows().row_data(0).unwrap();
    assert_eq!(outlines.label, "Selection outlines");
    assert_eq!(outlines.current, "Off");
    assert_eq!(outlines.defaults, "On");
    crate::ui::tests::save_ui_snapshot(window.window(), "preview-defaults");
    for label in [
        "Save current as defaults",
        "Apply saved defaults",
        "Reload defaults",
        "Reset saved defaults",
        "Close",
    ] {
        crate::ui::tests::assert_dialog_button_visible(window, label);
    }
    window.invoke_save();
    assert!(window.get_message().contains("saved"));
    assert_eq!(window.get_rows().row_data(0).unwrap().defaults, "Off");
    {
        let mut session = lock(&shared);
        let view = session.view();
        session.set_view(render::ViewOptions {
            selection_outlines: true,
            ..view
        });
    }
    window.invoke_apply();
    assert!(window.get_message().contains("changed"));
    assert!(lock(&shared).view().selection_outlines);
    window.invoke_apply();
    assert!(!lock(&shared).view().selection_outlines);
    assert_eq!(lock(&shared).view().sky_channel, render::sky::Channel::Hdr);
    assert_eq!(lock(&shared).view().sky_exposure_ev, 1.5);
    {
        let mut session = lock(&shared);
        session
            .preview_block(glam::DVec3::ZERO, glam::DVec3::splat(32.0))
            .unwrap();
    }
    window.invoke_apply();
    assert!(window.get_message().contains("active preview"));
    assert!(lock(&shared).block_preview().is_some());
    lock(&shared).cancel_block_preview();
    let path = directory
        .path()
        .join(editor_core::preview_preferences::FILE_NAME);
    std::fs::write(&path, b"bad preferences").unwrap();
    window.invoke_reload();
    assert!(!window.get_ready());
    assert!(!window.get_status().is_empty());
    assert_eq!(
        window.get_rows().row_data(0).unwrap().defaults,
        "Unavailable"
    );
    crate::ui::tests::save_ui_snapshot(window.window(), "preview-defaults-error");
    window.invoke_save();
    assert!(window.get_message().contains("did not load"));
    assert_eq!(std::fs::read(path).unwrap(), b"bad preferences");
    window.invoke_reset();
    assert!(window.get_ready());
    assert!(!lock(&shared).view().selection_outlines);
    window.invoke_apply();
    assert_eq!(
        editor_core::preview_preferences::Preview::from(lock(&shared).view()),
        editor_core::preview_preferences::Preview::default()
    );
    assert_eq!(lock(&shared).view().sky_channel, render::sky::Channel::Hdr);
    assert_eq!(lock(&shared).view().sky_exposure_ev, 1.5);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), 0);
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
}
