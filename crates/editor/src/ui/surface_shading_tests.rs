use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn directional_surface_shading_control_shares_session_state_and_atomic_validation() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("view controls need no assets".into()));
    let object = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "test/material",
            false,
        )
        .unwrap();
    session
        .preview_transform(
            Actor::User,
            &[object],
            map_core::edit::Transform {
                translation: DVec3::X * 8.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let preview = session.render_document().vmf().clone();
    let history = session.history_snapshot();
    let shared = Arc::new(Mutex::new(session));
    let app = AppWindow::new().unwrap();
    bind_view_controls(&app, shared.clone());
    refresh_view_controls(&app, &lock(&shared));
    assert!(!app.get_surface_shading());
    app.set_surface_shading(true);
    app.invoke_options_changed();
    assert!(lock(&shared).view().surface_shading);
    app.set_surface_shading(false);
    app.set_lighting_exposure_ev("9".into());
    app.invoke_options_changed();
    assert!(lock(&shared).view().surface_shading);
    assert!(app.get_surface_shading());
    {
        let mut session = lock(&shared);
        let view = session
            .resolve_view_update(editor_core::session::ViewUpdate {
                surface_shading: Some(false),
                ..Default::default()
            })
            .unwrap();
        session.set_view(view);
    }
    refresh_view_controls(&app, &lock(&shared));
    assert!(!app.get_surface_shading());
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.render_document().vmf(), &preview);
    assert_eq!(session.history_snapshot(), history);
}

#[test]
fn viewport_image_covers_allocated_bounds_at_scale_1_25_without_a_bottom_strip() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let app = AppWindow::new().unwrap();
    app.show().unwrap();
    let size = slint::PhysicalSize::new(1920, 1080);
    app.window().set_size(size);
    app.window()
        .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 1.25 });
    // The testing adapter caches physical size but emits resize events at scale 1.
    app.window()
        .dispatch_event(slint::platform::WindowEvent::Resized {
            size: size.to_logical(app.window().scale_factor()),
        });
    assert_eq!(app.window().size(), size);
    app.window().take_snapshot().unwrap();
    let mut image = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(1, 1);
    image.make_mut_bytes().copy_from_slice(&[28, 186, 71, 255]);
    app.set_frame(slint::Image::from_rgba8(image));
    let snapshot = app.window().take_snapshot().unwrap();
    assert_eq!((snapshot.width(), snapshot.height()), (1920, 1080));
    assert_eq!(app.window().scale_factor(), 1.25);
    let viewport =
        i_slint_backend_testing::ElementHandle::find_by_element_id(&app, "AppWindow::viewport")
            .next()
            .unwrap();
    let origin = viewport.absolute_position();
    let extent = viewport.size();
    assert!((extent.width - app.get_viewport_width()).abs() < 0.01);
    assert!((extent.height - app.get_viewport_height()).abs() < 0.01);
    let x = ((origin.x + extent.width - 4.0) * 1.25).floor() as usize;
    let first = ((origin.y + extent.height - 48.0) * 1.25).ceil() as usize;
    let last = ((origin.y + extent.height) * 1.25).floor() as usize - 2;
    for y in first..last {
        let offset = (y * snapshot.width() as usize + x) * 4;
        assert_eq!(
            &snapshot.as_bytes()[offset..offset + 4],
            &[28, 186, 71, 255],
            "row {y}"
        );
    }
    super::tests::save_ui_snapshot(app.window(), "viewport-scale-1-25");
}
