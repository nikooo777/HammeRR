use super::*;
use crate::session::Actor;
use std::sync::{Arc, Mutex};

#[test]
fn material_browser_source_window_reads_refreshes_reports_errors_and_closes_without_applying() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (root, game) = crate::test_assets::game();
    let path = root.path().join("materials/test/source.vmt");
    std::fs::write(&path, "UnlitGeneric { $basetexture test/checker }").unwrap();
    let mut session = Session::new(Ok(game));
    let id = session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(64.0),
            "test/checker",
            false,
        )
        .unwrap();
    session
        .preview_transform(
            Actor::Agent,
            &[id],
            map_core::edit::Transform {
                translation: glam::DVec3::X * 32.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let draft = session.render_document().vmf().clone();
    let snapshot = session.history_snapshot();
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let browser = MaterialWindow::new().unwrap();
    let source_window = bind(&browser, shared.clone());
    browser.set_material("test/source".into());
    browser.show().unwrap();
    browser.window().take_snapshot().unwrap();
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&browser, "Source…")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    let controller = source_window.borrow();
    let controller = controller.as_ref().unwrap();
    assert!(controller.window.window().is_visible());
    assert!(
        controller
            .window
            .get_provenance()
            .contains(&path.to_string_lossy().to_string())
    );
    assert!(controller.window.get_resolved().contains("unlitgeneric"));
    assert!(
        controller
            .window
            .get_source()
            .contains("$basetexture test/checker")
    );
    crate::ui::tests::save_ui_snapshot(controller.window.window(), "material-source");
    crate::ui::tests::assert_dialog_button_visible(&controller.window, "Refresh source");
    crate::ui::tests::assert_dialog_button_visible(&controller.window, "Close");
    std::fs::write(&path, "Patch { include materials/test/missing.vmt }").unwrap();
    controller.window.invoke_refresh_source();
    assert!(controller.window.get_status().contains("doesn't exist"));
    assert!(controller.window.get_source().contains("Patch"));
    assert!(controller.window.get_provenance().contains("Not found"));
    controller.window.invoke_close_dialog();
    assert!(!controller.window.window().is_visible());
    controller.show(&lock(&shared), "test/source").unwrap();
    assert!(controller.window.get_status().contains("doesn't exist"));
    controller.window.invoke_close_dialog();
    assert!(browser.window().is_visible());
    assert_eq!(browser.get_material(), "test/source");
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).render_document().vmf(), &draft);
    assert_eq!(lock(&shared).history_snapshot(), snapshot);
    assert_eq!(lock(&shared).camera(), camera);
    browser.hide().unwrap();
}
