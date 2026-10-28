use super::*;
use crate::runtime::lock;
use slint::Model;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn click(window: &Sky3dWindow, label: &str) {
    window.window().take_snapshot().unwrap();
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(window, label)
        .next()
        .unwrap()
        .invoke_accessible_default_action();
}

#[test]
fn software_sky3d_controls_choose_camera_toggle_prepare_and_show_unavailable_input() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("fixture content unavailable".into()));
    let camera = session
        .create_entity(
            Actor::User,
            "sky_camera",
            Some(glam::DVec3::new(64.0, 32.0, 16.0)),
            [
                ("scale".into(), "16".into()),
                ("targetname".into(), "miniature".into()),
            ]
            .into(),
        )
        .unwrap();
    session
        .create_entity(
            Actor::User,
            "sky_camera",
            Some(glam::DVec3::ZERO),
            [("scale".into(), "0".into())].into(),
        )
        .unwrap();
    let mut beyond_list = camera;
    for index in 0..66 {
        beyond_list = session
            .create_entity(
                Actor::User,
                "sky_camera",
                Some(glam::DVec3::new(index as f64, 0.0, 0.0)),
                [
                    ("scale".into(), "16".into()),
                    ("targetname".into(), format!("extra_{index}")),
                ]
                .into(),
            )
            .unwrap();
    }
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let selection = session.selected_ids();
    let view_camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let controller =
        Controller::new_with_access(&DocumentAccess::unmanaged(shared.clone())).unwrap();
    let window = &controller.window;
    controller.show(&mut lock(&shared)).unwrap();
    window.window().set_size(slint::PhysicalSize::new(820, 760));
    assert_eq!(window.get_camera_index(), 0);
    assert!(!window.get_preview_enabled());
    assert!(!window.get_can_prepare());
    assert_eq!(window.get_cameras().row_count(), 65);
    click(window, "Enable 3D sky preview");
    assert!(
        !lock(&shared).sky3d_preview_state()["enabled"]
            .as_bool()
            .unwrap()
    );
    assert!(!window.get_message().is_empty());
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(window, "3D sky camera")
        .next()
        .unwrap()
        .mock_single_click(slint::platform::PointerEventButton::Left);
    for text in [
        slint::platform::Key::DownArrow,
        slint::platform::Key::Return,
    ] {
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: text.into() });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyReleased { text: text.into() });
    }
    assert_eq!(window.get_camera_index(), 1);
    assert!(window.get_detail().contains("miniature"));
    assert!(window.get_can_prepare());
    assert_eq!(
        lock(&shared).sky3d_preview_state()["camera_entity"],
        crate::identity::id_string(camera)
    );
    click(window, "Enable 3D sky preview");
    assert_eq!(lock(&shared).sky3d_preview_state()["enabled"], true);
    click(window, "Prepare 3D sky");
    let deadline = Instant::now() + Duration::from_secs(5);
    while lock(&shared).sky3d_preview_state()["worker_active"] == true {
        assert!(Instant::now() < deadline, "preparation did not retire");
        std::thread::sleep(Duration::from_millis(5));
    }
    click(window, "Refresh status");
    let report = lock(&shared).sky3d_preview_state();
    assert_eq!(report["ready"], false);
    assert_ne!(report["status"], "ready");
    assert!(!window.get_diagnostics().is_empty());
    crate::ui::tests::assert_ui_renders(window.window(), "sky3d-preview-unavailable");
    for label in [
        "Refresh status",
        "Prepare 3D sky",
        "Cancel preparation",
        "Close",
    ] {
        crate::ui::tests::assert_dialog_button_visible(window, label);
    }
    click(window, "Enable 3D sky preview");
    assert_eq!(lock(&shared).sky3d_preview_state()["enabled"], false);
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(window, "Camera entity ID")
        .find(|element| {
            element.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::TextInput)
        })
        .unwrap()
        .set_accessible_value(crate::identity::id_string(beyond_list));
    assert_eq!(
        window.get_camera_entity(),
        crate::identity::id_string(beyond_list)
    );
    click(window, "Use camera");
    assert_eq!(
        lock(&shared).sky3d_preview_state()["camera_entity"],
        crate::identity::id_string(beyond_list)
    );
    assert_eq!(window.get_camera_index(), 65);
    assert_eq!(window.get_cameras().row_count(), 66);
    assert!(window.get_detail().contains("extra_65"));
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(window, "Camera entity ID")
        .find(|element| {
            element.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::TextInput)
        })
        .unwrap()
        .set_accessible_value("solid:1");
    assert_eq!(window.get_camera_entity(), "solid:1");
    click(window, "Use camera");
    assert!(!window.get_message().is_empty());
    assert_eq!(
        lock(&shared).sky3d_preview_state()["camera_entity"],
        crate::identity::id_string(beyond_list)
    );
    click(window, "Close");
    assert!(!window.window().is_visible());
    let state = lock(&shared);
    assert_eq!(state.document().vmf(), &source);
    assert_eq!(state.history_snapshot(), history);
    assert_eq!(state.selected_ids(), selection);
    assert_eq!(state.camera(), view_camera);
}
