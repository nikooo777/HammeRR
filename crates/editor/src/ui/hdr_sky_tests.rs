use super::*;

#[test]
fn resource_visibility_controls_share_defaults_and_preserve_pending_source_reviews() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets".into()));
    session
        .begin_property_edit(editor_core::session::PropertyTarget::World)
        .unwrap();
    let token = session.property_draft().unwrap().token;
    let source = session.document().vmf().clone();
    let serial = session.scene_serial();
    let shared = std::sync::Arc::new(std::sync::Mutex::new(session));
    let app = AppWindow::new().unwrap();
    bind_view_controls(&app, shared.clone());
    refresh_view_controls(&app, &lock(&shared));
    assert!(app.get_entity_images() && app.get_entity_model_guides());
    for enabled in [false, false, true] {
        app.set_entity_images(enabled);
        app.set_entity_model_guides(enabled);
        app.invoke_options_changed();
        let session = lock(&shared);
        assert_eq!(session.view().entity_images, enabled);
        assert_eq!(session.view().entity_model_guides, enabled);
        assert_eq!(session.scene_serial(), serial);
        assert_eq!(session.property_draft().unwrap().token, token);
        assert_eq!(session.document().vmf(), &source);
    }
    {
        let mut session = lock(&shared);
        let view = render::ViewOptions {
            entity_images: false,
            ..session.view()
        };
        session.set_view(view);
        refresh_view_controls(&app, &session);
    }
    assert!(!app.get_entity_images() && app.get_entity_model_guides());
}

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
    app.set_entity_images(false);
    app.set_entity_model_guides(false);
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
    assert_eq!(app.get_entity_images(), accepted.entity_images);
    assert_eq!(app.get_entity_model_guides(), accepted.entity_model_guides);
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
