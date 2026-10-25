use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn surface_pivot_ui_arms_cancels_picks_and_releases_foreign_or_stale_input() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session
        .create_block(
            Actor::User,
            DVec3::new(-128.0, -128.0, -16.0),
            DVec3::new(128.0, 128.0, 0.0),
            "brick/test",
            false,
        )
        .unwrap();
    session
        .set_transform_tool(
            Actor::User,
            TransformTool {
                mode: Some(render::handles::Mode::Rotate),
                pivot: Some(DVec3::new(1.25, 2.5, 3.75)),
                ..Default::default()
            },
        )
        .unwrap();
    let camera = render::Camera::planar(
        render::Projection::Top,
        glam::Vec3::new(3.125, -5.25, 0.0),
        300.0,
    );
    session.set_camera(camera);
    session
        .set_spatial(
            Actor::User,
            editor_core::spatial::Update {
                grid_size: Some(0.25),
                snap: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let selection = session.selected_ids();
    let shared = Arc::new(Mutex::new(session));
    let app = AppWindow::new().unwrap();
    bind_pivot_pick_controls(&app, shared.clone());
    refresh_panel(&app, &mut lock(&shared));
    refresh_interaction(&app, &lock(&shared));
    app.set_show_transform(true);
    app.show().unwrap();
    app.window().set_size(slint::PhysicalSize::new(1600, 1000));
    refresh_pivot_pick_controls(&app, &lock(&shared), camera);
    crate::ui::tests::scroll_inspector_to_control(&app, "Pick pivot…");
    crate::ui::tests::assert_dialog_button_visible(&app, "Pick pivot…");
    app.invoke_pick_transform_pivot();
    assert!(app.get_pivot_picking());
    assert!(app.get_can_cancel_pivot_pick());
    assert!(app.get_pivot_pick_notice().contains("Click a visible"));
    assert!(!app.get_can_snap_pivot());
    app.invoke_snap_transform_pivot_to_grid();
    assert!(lock(&shared).transform_pivot_pick_owned(Actor::User));
    crate::ui::tests::assert_dialog_button_visible(&app, "Cancel pivot pick");
    crate::ui::tests::save_ui_snapshot(app.window(), "surface-picked-transform-pivot");
    app.invoke_cancel_transform_pivot_pick();
    assert!(!app.get_pivot_picking());
    assert_eq!(
        lock(&shared).transform_tool().pivot,
        Some(DVec3::new(1.25, 2.5, 3.75))
    );
    app.invoke_pick_transform_pivot();
    let size = viewport_size(&app);
    let pointer = Pointer {
        position: DVec2::new(f64::from(size.0) * 0.5, f64::from(size.1) * 0.5),
        size,
    };
    assert!(pick_pivot_pixel(&app, &mut lock(&shared), camera, pointer));
    assert!(!app.get_pivot_picking());
    let pivot = lock(&shared).transform_tool().pivot.unwrap();
    assert!(pivot.abs_diff_eq(DVec3::new(3.125, -5.25, 0.0), 1e-5));
    assert_eq!(app.get_transform_pivot_mode(), 2);
    assert_eq!(app.get_pivot_x().as_str(), pivot.x.to_string().as_str());
    assert!(app.get_pivot_pick_source().contains("Picked solid:"));
    assert!(app.get_can_snap_pivot());
    crate::ui::tests::assert_dialog_button_visible(&app, "Snap pivot to grid");
    app.invoke_snap_transform_pivot_to_grid();
    let snapped = DVec3::new(3.25, -5.25, 0.0);
    assert!(
        lock(&shared)
            .transform_tool()
            .pivot
            .unwrap()
            .abs_diff_eq(snapped, 1e-5)
    );
    assert_eq!(app.get_transform_pivot_mode(), 2);
    assert_eq!(app.get_pivot_x(), "3.25");
    assert!(app.get_pivot_pick_source().is_empty());
    assert!(app.get_message().contains("on grid 0.25"));
    crate::ui::tests::save_ui_snapshot(app.window(), "grid-snapped-transform-pivot");
    {
        let mut session = lock(&shared);
        let snapshot = session.transform_pivot_pick_stamp();
        session
            .begin_transform_pivot_pick(Actor::Agent, camera, size, &snapshot)
            .unwrap();
        refresh_pivot_pick_controls(&app, &session, camera);
        assert!(!app.get_pivot_picking() && !app.get_can_cancel_pivot_pick());
        assert!(!pick_pivot_pixel(&app, &mut session, camera, pointer));
        let token = session.transform_pivot_pick_state().picking.unwrap().token;
        session
            .cancel_transform_pivot_pick(Actor::Agent, token)
            .unwrap();
    }
    app.invoke_pick_transform_pivot();
    {
        let mut session = lock(&shared);
        session.set_camera(camera);
        refresh_pivot_pick_controls(&app, &session, camera);
        assert!(!app.get_pivot_picking() && app.get_can_cancel_pivot_pick());
        assert!(!pick_pivot_pixel(&app, &mut session, camera, pointer));
    }
    app.invoke_cancel_transform_pivot_pick();
    assert!(!lock(&shared).transform_pivot_picking());
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    assert_eq!(lock(&shared).selected_ids(), selection);
    assert_eq!(lock(&shared).camera(), camera);
    app.hide().unwrap();
}
