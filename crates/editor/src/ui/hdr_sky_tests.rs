use super::*;

#[test]
fn hdr_sky_inline_controls_share_validation_restore_rejected_toggles_and_preserve_source() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let session = std::sync::Arc::new(std::sync::Mutex::new(Session::new(Err("no assets".into()))));
    let source = lock(&session).document().vmf().clone();
    let history = lock(&session).document().undo_labels().count();
    let camera = lock(&session).camera();
    let app = AppWindow::new().unwrap();
    bind_view_controls(&app, session.clone());
    refresh_view_controls(&app, &lock(&session));
    app.set_sky_preview(true);
    app.set_sky_channel(1);
    app.set_sky_exposure_ev("2".into());
    app.invoke_options_changed();
    assert_eq!(lock(&session).view().sky_channel, render::sky::Channel::Hdr);
    assert_eq!(lock(&session).view().sky_exposure_ev, 2.0);
    assert!(app.get_sky_preview_error().is_empty());
    let accepted = lock(&session).view();
    app.set_sky_exposure_ev("NaN".into());
    app.set_sky_preview(false);
    app.set_show_tools(false);
    app.set_show_leak(false);
    app.set_rope_preview(false);
    app.set_selection_outlines(false);
    app.invoke_options_changed();
    assert_eq!(lock(&session).view(), accepted);
    assert!(
        app.get_sky_preview_error()
            .contains("finite and between -8 and 8 EV")
    );
    assert!(app.get_sky_preview());
    assert_eq!(app.get_show_tools(), accepted.tool_textures);
    assert_eq!(app.get_show_leak(), accepted.leak_path);
    assert_eq!(app.get_rope_preview(), accepted.rope_preview);
    assert_eq!(app.get_selection_outlines(), accepted.selection_outlines);
    assert_eq!(app.get_sky_exposure_ev().as_str(), "2");
    app.set_sky_preview(false);
    app.set_sky_channel(0);
    app.invoke_options_changed();
    assert!(!lock(&session).view().sky_preview);
    assert_eq!(lock(&session).view().sky_channel, render::sky::Channel::Ldr);
    app.set_sky_preview(true);
    app.set_sky_channel(1);
    app.invoke_options_changed();
    app.show().unwrap();
    app.window().set_size(slint::PhysicalSize::new(1600, 960));
    super::tests::save_ui_snapshot(app.window(), "hdr-sky-preview-controls");
    assert_eq!(lock(&session).document().vmf(), &source);
    assert_eq!(lock(&session).document().undo_labels().count(), history);
    assert_eq!(lock(&session).camera(), camera);
    app.hide().unwrap();
}
