use super::*;
use slint::Model;
use std::sync::{Arc, Mutex};

#[test]
fn particle_browser_shares_search_review_apply_cancel_and_software_screenshot() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (_directory, session, id) = crate::test_assets::particles();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(session));
    let token = lock(&shared).begin_particle_choice(id, false).unwrap()["token"]
        .as_u64()
        .unwrap();
    lock(&shared).choose_particle(token, "test_flame").unwrap();
    let controller = Controller::new(&shared).unwrap();
    controller.initialize(&mut lock(&shared), id).unwrap();
    assert_eq!(controller.token.get(), Some(token));
    assert_eq!(controller.window.get_value(), "test_flame");
    assert!(controller.window.get_can_apply());
    controller.window.set_query("flame fire".into());
    controller.window.invoke_search();
    assert_eq!(controller.window.get_rows().row_count(), 1);
    controller.window.set_query("no matching system".into());
    controller.window.invoke_search();
    assert_eq!(controller.window.get_rows().row_count(), 0);
    assert!(controller.window.get_can_apply());
    assert_eq!(lock(&shared).document().vmf(), &source);
    controller.window.show().unwrap();
    lock(&shared).choose_particle(token, "test_sparks").unwrap();
    controller.window.invoke_apply();
    assert!(controller.window.window().is_visible());
    assert_eq!(controller.window.get_value(), "test_sparks");
    assert_eq!(lock(&shared).document().vmf(), &source);
    controller.window.invoke_apply();
    assert!(!controller.window.window().is_visible());
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    controller.initialize(&mut lock(&shared), id).unwrap();
    controller.window.invoke_choose("test_flame".into());
    let replacement = lock(&shared).begin_particle_choice(id, false).unwrap()["token"]
        .as_u64()
        .unwrap();
    controller.window.invoke_cancel();
    assert!(lock(&shared).particle_choice(replacement).is_ok());
    controller.initialize(&mut lock(&shared), id).unwrap();
    controller.window.invoke_choose("test_flame".into());
    lock(&shared).cancel_particle_choice(replacement).unwrap();
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_can_apply());
    controller.window.invoke_cancel();
    assert_eq!(lock(&shared).document().vmf(), &source);
    if std::env::var_os("HAMMERR_UI_SNAPSHOTS").is_some()
        && let Ok(game) = editor_core::startup::mount_game(None)
    {
        let mut session = Session::new(Ok(game));
        session.set_definitions(Actor::User,source_formats::fgd::Catalog::parse("@PointClass = info_particle_system [ effect_name(string) : \"Particle System Name\" : \"\" ]").unwrap());
        let map_core::ObjectId::Entity(id) = session
            .create_entity(
                Actor::User,
                "info_particle_system",
                Some(glam::DVec3::ZERO),
                std::collections::BTreeMap::new(),
            )
            .unwrap()
        else {
            unreachable!()
        };
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.initialize(&mut lock(&shared), id).unwrap();
        controller.window.set_query("fire_medium_01".into());
        controller.window.invoke_search();
        controller.window.invoke_choose("fire_medium_01".into());
        assert!(
            controller.window.get_can_apply(),
            "{}",
            controller.window.get_message()
        );
        controller.window.show().unwrap();
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "particle-browser");
        controller.window.invoke_cancel();
    }
}
