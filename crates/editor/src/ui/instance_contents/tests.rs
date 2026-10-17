use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn software_controls_share_visibility_reports_frame_clear_and_preserve_parent() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (_directory, game, parent, _child) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &parent, true).unwrap();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera();
    let request = session
        .begin_instance_contents_refresh(Actor::User)
        .unwrap();
    let result = request.run();
    session
        .finish_instance_contents_refresh(Actor::User, result)
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let app = AppWindow::new().unwrap();
    super::super::bind_view_controls(&app, shared.clone());
    super::super::refresh_view_controls(&app, &lock(&shared));
    assert!(!app.get_instance_contents());
    let (serials, mut displayed_key, mut draw_options) = {
        let session = lock(&shared);
        (
            (session.scene_serial(), session.view_serial()),
            session.render_scene_key(session.view()),
            session.view(),
        )
    };
    app.set_instance_contents(true);
    app.invoke_options_changed();
    assert!(lock(&shared).view().instance_contents);
    {
        let mut session = lock(&shared);
        displayed_key =
            super::super::prepare_scene_refresh(&mut session, displayed_key, &mut draw_options)
                .expect("Reference-only enable must refresh the production viewport route");
        assert!(draw_options.instance_contents);
        assert!(
            !session
                .build_scene_for_view(draw_options)
                .reference_lines
                .is_empty()
        );
        assert_eq!((session.scene_serial(), session.view_serial()), serials);
        assert!(
            super::super::prepare_scene_refresh(&mut session, displayed_key, &mut draw_options,)
                .is_none()
        );
    }
    for enabled in [false, true] {
        let mut session = lock(&shared);
        let view = session
            .resolve_view_update(session::ViewUpdate {
                instance_contents: Some(enabled),
                ..Default::default()
            })
            .unwrap();
        session.set_view(view);
        displayed_key =
            super::super::prepare_scene_refresh(&mut session, displayed_key, &mut draw_options)
                .expect("Shared visibility changes must update retained draw options");
        assert_eq!(draw_options.instance_contents, enabled);
        let mesh = session.build_scene_for_view(draw_options);
        assert_eq!(!mesh.reference_lines.is_empty(), enabled);
        assert_eq!(
            session.instance_contents_rendered_report()["displayed"],
            enabled
        );
        assert_eq!((session.scene_serial(), session.view_serial()), serials);
    }
    let controller = Controller::new(shared.clone()).unwrap();
    assert!(
        controller
            .window
            .get_summary()
            .contains("1 instance from 1 file")
    );
    assert!(controller.window.get_owners().contains("child.vmf"));
    assert!(controller.window.get_can_frame());
    controller.window.show().unwrap();
    controller
        .window
        .window()
        .set_size(slint::PhysicalSize::new(850, 690));
    crate::ui::tests::save_ui_snapshot(controller.window.window(), "instance-contents");
    controller.window.invoke_frame_contents();
    assert_ne!(lock(&shared).camera(), camera);
    lock(&shared).set_camera(camera);
    controller.window.invoke_clear_contents();
    assert!(!controller.window.get_can_frame());
    assert!(lock(&shared).view().instance_contents);
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.camera(), camera);
}

#[test]
fn software_controls_show_refresh_failure_without_relocking_the_session() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("No game assets required".into()));
    session
        .create_entity(
            Actor::User,
            "func_instance",
            Some(DVec3::ZERO),
            BTreeMap::from([("file".into(), "child.vmf".into())]),
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(shared.clone()).unwrap();
    controller.window.invoke_refresh_contents();
    assert!(
        controller
            .window
            .get_message()
            .contains("Save the parent VMF")
    );
    assert!(!controller.window.get_loading());
    assert!(!controller.window.get_can_frame());
}
