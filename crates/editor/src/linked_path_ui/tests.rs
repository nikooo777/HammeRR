use super::*;
use source_formats::fgd::Catalog;
use std::sync::{Arc, Mutex};

fn edit_coordinate(window: &LinkedPathWindow, index: usize, x: &str) {
    window.window().take_snapshot().unwrap();
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        window,
        &format!("Point {} X", index + 1),
    )
    .next()
    .unwrap()
    .set_accessible_value(x);
    assert_eq!(window.get_points().row_data(index).unwrap().x, x);
}

#[test]
fn linked_path_window_edits_orders_reviews_and_applies_owned_preview_once() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.set_definitions(
        Actor::User,
        Catalog::parse(
            r#"
@PointClass = path_corner : "Path corner" [
    targetname(target_source) : "Name"
    target(target_destination) : "Next point"
    speed(float) : "Speed" : 100
]
@PointClass = path_track : "Path track" [
    targetname(target_source) : "Name"
    target(target_destination) : "Next point"
]
"#,
        )
        .unwrap(),
    );
    let origin = session.placement_origin();
    let before = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    controller.show(&lock(&shared)).unwrap();
    assert_eq!(request(&controller.window).unwrap().points[0], origin);
    assert_eq!(controller.window.get_points().row_count(), 2);
    assert!(!controller.window.get_ready());
    controller.window.invoke_add_point();
    assert_eq!(controller.window.get_points().row_count(), 3);
    edit_coordinate(&controller.window, 2, "384.5");
    controller.window.invoke_move_point(2, -1);
    assert_eq!(request(&controller.window).unwrap().points[1].x, 384.5);
    controller.window.invoke_remove_point(2);
    assert_eq!(controller.window.get_points().row_count(), 2);
    controller.window.set_prefix("route".into());
    controller.window.set_closed(true);
    controller.window.invoke_edited();
    controller.window.invoke_preview();
    assert!(
        controller.window.get_ready(),
        "{}",
        controller.window.get_message()
    );
    assert!(
        controller
            .window
            .get_points()
            .row_data(0)
            .unwrap()
            .link
            .contains("route_001")
    );
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);

    edit_coordinate(&controller.window, 1, "400.25");
    assert!(!controller.window.get_ready());
    controller.window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &before);
    edit_coordinate(&controller.window, 1, "NaN");
    controller.window.invoke_preview();
    assert!(!controller.window.get_ready());
    assert!(controller.window.get_message().contains("finite"));
    edit_coordinate(&controller.window, 1, "400.25");
    controller.window.invoke_preview();
    assert!(
        controller.window.get_ready(),
        "{}",
        controller.window.get_message()
    );
    controller.window.invoke_cancel();
    assert!(lock(&shared).linked_path()["preview"].is_null());
    assert_eq!(lock(&shared).document().vmf(), &before);

    let foreign_token = {
        let mut session = lock(&shared);
        let snapshot = session.linked_path_stamp();
        let state = session
            .preview_linked_path(
                Actor::Agent,
                Request {
                    classname: "path_track".into(),
                    prefix: "train".into(),
                    points: vec![DVec3::ZERO, DVec3::new(128.0, 64.0, 32.0)],
                    closed: false,
                },
                &snapshot,
            )
            .unwrap();
        state["preview"]["token"].as_u64().unwrap()
    };
    controller.show(&lock(&shared)).unwrap();
    assert!(controller.window.get_can_review());
    assert!(!controller.window.get_ready());
    assert!(!controller.window.get_can_preview());
    controller.window.invoke_cancel();
    assert_eq!(
        lock(&shared).linked_path()["preview"]["token"],
        foreign_token
    );
    controller.show(&lock(&shared)).unwrap();
    controller.window.invoke_review();
    assert!(
        controller.window.get_ready(),
        "{}",
        controller.window.get_message()
    );
    assert_eq!(controller.window.get_class_index(), 1);
    assert_eq!(controller.window.get_prefix(), "train");
    controller.window.invoke_add_point();
    controller.window.set_closed(true);
    controller.window.invoke_edited();
    controller.window.invoke_preview();
    assert!(
        controller.window.get_ready(),
        "{}",
        controller.window.get_message()
    );
    crate::ui::tests::save_ui_snapshot(controller.window.window(), "linked-path");
    for label in [
        "Add point",
        "Remove",
        "Move up",
        "Move down",
        "Preview",
        "Apply",
        "Cancel",
    ] {
        crate::ui::tests::assert_dialog_button_visible(&controller.window, label);
    }
    controller.window.invoke_apply();
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    assert!(lock(&shared).linked_path()["preview"].is_null());
    assert!(!controller.window.window().is_visible());
    controller.window.invoke_apply();
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &before);

    controller.show(&lock(&shared)).unwrap();
    lock(&shared).new_map(Actor::Agent, true).unwrap();
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_editable());
    controller.window.invoke_preview();
    assert!(controller.window.get_message().contains("Map changed"));
    assert!(lock(&shared).linked_path()["preview"].is_null());

    controller.window.hide().unwrap();
    let ids = {
        let mut session = lock(&shared);
        let snapshot = session.linked_path_stamp();
        let state = session
            .preview_linked_path(
                Actor::Agent,
                Request {
                    classname: "path_track".into(),
                    prefix: "existing".into(),
                    points: vec![DVec3::ZERO, DVec3::X * 64.0, DVec3::X * 128.0],
                    closed: false,
                },
                &snapshot,
            )
            .unwrap();
        session
            .apply_linked_path(Actor::Agent, state["preview"]["token"].as_u64().unwrap())
            .unwrap();
        let ids = session
            .document()
            .vmf()
            .entities
            .iter()
            .map(|entity| entity.id)
            .collect::<Vec<_>>();
        session
            .set_entity_properties(
                Actor::Agent,
                ids[1],
                &[
                    ("custom_key".into(), Some("Keep me".into())),
                    ("spawnflags".into(), Some("1025".into())),
                ],
            )
            .unwrap();
        session
            .select(
                &[map_core::ObjectId::Entity(ids[1])],
                crate::session::SelectMode::Replace,
            )
            .unwrap();
        ids
    };
    let before = lock(&shared).document().vmf().clone();
    let history = lock(&shared).document().undo_labels().count();
    controller.show(&lock(&shared)).unwrap();
    assert!(controller.window.get_can_load());
    controller.window.invoke_load_selected();
    assert!(controller.window.get_existing());
    assert_eq!(controller.window.get_class_index(), 1);
    assert_eq!(
        controller.window.get_points().row_data(0).unwrap().name,
        "existing_001"
    );
    controller.window.invoke_add_point();
    controller.window.invoke_remove_point(0);
    assert_eq!(controller.window.get_points().row_count(), 3);
    controller.window.invoke_move_point(2, -1);
    assert_eq!(
        controller.window.get_points().row_data(1).unwrap().id,
        format!("entity:{}", ids[2])
    );
    edit_coordinate(&controller.window, 1, "160.5");
    controller.window.invoke_preview();
    assert!(
        controller.window.get_ready(),
        "{}",
        controller.window.get_message()
    );
    let expected = lock(&shared).render_document().vmf().clone();
    assert_eq!(lock(&shared).document().vmf(), &before);
    crate::ui::tests::save_ui_snapshot(controller.window.window(), "linked-path-existing");
    crate::ui::tests::assert_dialog_button_visible(&controller.window, "Load selected path");
    controller.window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &expected);
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    assert_eq!(
        lock(&shared)
            .document()
            .entity(ids[1])
            .unwrap()
            .properties
            .get("custom_key"),
        Some("Keep me")
    );
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &before);
    lock(&shared)
        .select(
            &[map_core::ObjectId::Entity(ids[1])],
            crate::session::SelectMode::Replace,
        )
        .unwrap();
    controller.show(&lock(&shared)).unwrap();
    controller.window.invoke_load_selected();
    lock(&shared)
        .set_entity_properties(
            Actor::Agent,
            ids[1],
            &[("custom_key".into(), Some("New data".into()))],
        )
        .unwrap();
    let changed = lock(&shared).document().vmf().clone();
    controller.window.invoke_preview();
    assert!(!controller.window.get_ready());
    assert_eq!(lock(&shared).document().vmf(), &changed);
    assert!(controller.window.get_message().contains("changed"));
    controller.window.invoke_load_selected();
    controller.window.invoke_preview();
    assert!(
        controller.window.get_ready(),
        "{}",
        controller.window.get_message()
    );
    controller.window.invoke_cancel();
    assert_eq!(lock(&shared).document().vmf(), &changed);
}
