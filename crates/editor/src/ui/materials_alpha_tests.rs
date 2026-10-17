use super::*;

#[test]
fn material_browser_color_alpha_callbacks_retain_source_drafts_and_explicit_failures() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = test_assets::alpha_game();
    let mut session = Session::new(Ok(game));
    session
        .begin_property_edit(session::PropertyTarget::World)
        .unwrap();
    let token = session.property_draft().unwrap().token;
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let shared = std::sync::Arc::new(std::sync::Mutex::new(session));
    let window = MaterialWindow::new().unwrap();
    bind_material_preview(&window, shared.clone());
    bind_material_reload(&window, shared.clone());
    bind_material_cancel(&window);
    window.set_material("test/checker".into());
    refresh_material_catalog(&window, &mut lock(&shared));
    window.invoke_preview_material();
    let color = window.get_preview().to_rgba8().unwrap();
    assert_eq!(window.get_preview_channel(), 0);
    window.show().unwrap();
    window.window().take_snapshot().unwrap();
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Alpha")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(window.get_preview_channel(), 1);
    assert!(window.get_available());
    assert!(window.get_detail().contains("materials/test/checker.vtf"));
    assert!(window.get_detail().contains("source alpha 0–255"));
    let alpha = window.get_preview().to_rgba8().unwrap();
    for (color, alpha) in color
        .as_bytes()
        .chunks_exact(4)
        .zip(alpha.as_bytes().chunks_exact(4))
    {
        assert_eq!(alpha, [color[3], color[3], color[3], 255]);
    }
    crate::ui::tests::save_ui_snapshot(window.window(), "material-alpha-preview");
    crate::ui::tests::assert_dialog_button_visible(&window, "Color");
    crate::ui::tests::assert_dialog_button_visible(&window, "Alpha");
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Color")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(
        window.get_preview().to_rgba8().unwrap().as_bytes(),
        color.as_bytes()
    );
    window.set_material("test/absent".into());
    window.invoke_preview_material();
    assert!(!window.get_available());
    assert!(window.get_detail().contains("Missing texture"));
    std::fs::write(
        directory.path().join("materials/test/broken.vmt"),
        "LightmappedGeneric { $basetexture }",
    )
    .unwrap();
    window.set_material("test/broken".into());
    window.set_preview_channel(1);
    window.invoke_preview_material();
    assert!(!window.get_available());
    assert_eq!(window.get_preview().size().width, 0);
    assert!(!window.get_detail().is_empty());
    window.invoke_cancel();
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.property_draft().unwrap().token, token);
}
