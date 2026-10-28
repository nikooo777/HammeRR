use std::sync::{Arc, Mutex};

use glam::DVec3;

use super::*;

#[test]
fn point_entity_placement_controls_pick_review_apply_cancel_and_reject_replacement() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (session, _) = crate::test_assets::placement();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    controller.initialize(&lock(&shared), "info_target");
    controller
        .window
        .window()
        .set_size(slint::PhysicalSize::new(610, 660));
    controller.window.show().unwrap();
    controller.window.set_snap(true);
    controller.window.set_align(true);
    controller.window.set_offset("4".into());
    controller.window.invoke_invalidate();
    controller.window.invoke_pick();
    assert!(!controller.window.window().is_visible());
    assert_eq!(
        lock(&shared).get_entity_placement()["draft"]["picking"],
        true
    );
    assert!(
        controller
            .pick_at_ray(
                &mut lock(&shared),
                Ray {
                    origin: DVec3::splat(1000.0),
                    direction: DVec3::Z
                }
            )
            .is_err()
    );
    assert_eq!(
        lock(&shared).get_entity_placement()["draft"]["picking"],
        true
    );
    controller
        .pick_at_ray(
            &mut lock(&shared),
            Ray {
                origin: DVec3::new(32.0, 0.0, 64.0),
                direction: DVec3::Y,
            },
        )
        .unwrap();
    controller.window.show().unwrap();
    assert!(controller.window.get_ready());
    assert!(controller.window.get_summary().contains("Bounds minimum"));
    assert!(
        controller
            .window
            .get_summary()
            .contains("Source angles · 0 0 90")
    );
    assert!(!controller.window.get_summary().contains("light pitch"));
    assert!(
        controller
            .window
            .get_summary()
            .contains("Contact offset · 4.000")
    );
    crate::ui::tests::assert_ui_renders(controller.window.window(), "entity-placement-controls");
    for label in ["Pick surface", "Apply entity", "Cancel"] {
        crate::ui::tests::assert_dialog_button_visible(&controller.window, label);
    }
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    let old_token = lock(&shared).get_entity_placement()["draft"]["token"]
        .as_u64()
        .unwrap();
    lock(&shared).refresh_model_assets(Actor::Agent);
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_ready());
    controller.window.invoke_pick();
    assert_ne!(
        lock(&shared).get_entity_placement()["draft"]["token"],
        old_token
    );
    controller
        .pick_at_ray(
            &mut lock(&shared),
            Ray {
                origin: DVec3::new(32.0, 0.0, 64.0),
                direction: DVec3::Y,
            },
        )
        .unwrap();
    controller.window.set_offset("8".into());
    controller.window.invoke_invalidate();
    controller.window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &source);
    controller.window.invoke_pick();
    controller
        .pick_at_ray(
            &mut lock(&shared),
            Ray {
                origin: DVec3::new(32.0, 0.0, 64.0),
                direction: DVec3::Y,
            },
        )
        .unwrap();
    controller.window.invoke_apply();
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    assert!(lock(&shared).get_entity_placement()["draft"].is_null());
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);

    let token = {
        let mut session = lock(&shared);
        let snapshot = session.entity_placement_stamp();
        let report = session
            .begin_entity_placement(
                Actor::Agent,
                Request {
                    classname: "light".into(),
                    properties: BTreeMap::new(),
                    offset: 2.0,
                    align: false,
                    snap: false,
                },
                &snapshot,
            )
            .unwrap();
        let token = report["draft"]["token"].as_u64().unwrap();
        session
            .pick_entity_placement(
                Actor::Agent,
                token,
                Ray {
                    origin: DVec3::new(32.0, 0.0, 64.0),
                    direction: DVec3::NEG_Z,
                },
            )
            .unwrap();
        token
    };
    lock(&shared).refresh_model_assets(Actor::Agent);
    controller.initialize(&lock(&shared), "info_target");
    assert_eq!(controller.window.get_classname(), "light");
    assert!(!controller.window.get_owned());
    controller.window.invoke_apply();
    assert_eq!(lock(&shared).document().vmf(), &source);
    controller.window.invoke_review();
    assert!(controller.window.get_owned());
    let human_token = lock(&shared).get_entity_placement()["draft"]["token"]
        .as_u64()
        .unwrap();
    assert_ne!(token, human_token);
    assert!(
        lock(&shared)
            .cancel_entity_placement(Actor::Agent, token)
            .is_err()
    );
    controller.window.invoke_cancel();
    assert!(lock(&shared).get_entity_placement()["draft"].is_null());
    assert_eq!(lock(&shared).document().vmf(), &source);

    controller.initialize(&lock(&shared), "info_target");
    controller.window.invoke_pick();
    controller
        .pick_at_ray(
            &mut lock(&shared),
            Ray {
                origin: DVec3::new(32.0, 0.0, 64.0),
                direction: DVec3::NEG_Z,
            },
        )
        .unwrap();
    let id = lock(&shared).document().objects().next().unwrap();
    lock(&shared)
        .preview_transform(
            Actor::Agent,
            &[id],
            map_core::edit::Transform {
                translation: DVec3::X,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let replacement = lock(&shared).render_document().vmf().clone();
    controller.window.invoke_cancel();
    assert_eq!(lock(&shared).render_document().vmf(), &replacement);
}
