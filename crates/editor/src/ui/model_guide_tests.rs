use super::*;
use source_formats::fgd::Catalog;

#[test]
fn literal_model_guide_panel_reports_real_asset_and_failure_without_authoring() {
    let Ok(game) = editor_core::startup::mount_game(None) else {
        return;
    };
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Ok(game));
    let definition = "@PointClass size(4 4 4) studio(\"models/props_junk/wood_crate001a.mdl\") = helper_sample []";
    session.set_definitions(Actor::Agent, Catalog::parse(definition).unwrap());
    let ObjectId::Entity(id) = session
        .create_entity(
            Actor::Agent,
            "helper_sample",
            Some(DVec3::ZERO),
            BTreeMap::new(),
        )
        .unwrap()
    else {
        panic!()
    };
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera();
    let app = AppWindow::new().unwrap();
    refresh_panel(&app, &mut session);
    assert_eq!(
        app.get_entity_helper_status().as_str(),
        session.entity_helper_report(id).unwrap().status
    );
    assert!(
        app.get_entity_helper_status()
            .contains("FGD model guide: displayed")
    );
    assert!(
        app.get_entity_helper_status()
            .contains("studio(models/props_junk/wood_crate001a.mdl)")
    );
    assert!(
        app.get_entity_helper_status()
            .contains("flat editor mesh only, ordinary helper bounds and picking")
    );
    app.show().unwrap();
    app.window().set_size(slint::PhysicalSize::new(1600, 960));
    super::tests::scroll_inspector_to_control(&app, "Advanced properties");
    super::tests::save_ui_snapshot(app.window(), "literal-fgd-model-guide-inspector");
    session.set_definitions(
        Actor::Agent,
        Catalog::parse(&definition.replace("wood_crate001a.mdl", "missing_helper.mdl")).unwrap(),
    );
    refresh_panel(&app, &mut session);
    assert!(
        app.get_entity_helper_status()
            .contains("FGD model guide: unavailable")
    );
    assert!(app.get_entity_helper_status().contains("was not found"));
    assert!(app.get_has_fgd_model_guide());
    assert!(!app.get_can_inspect_model());
    let shared = std::sync::Arc::new(std::sync::Mutex::new(session));
    bind_model_guide_reload(&app, shared.clone());
    let serial = lock(&shared).scene_serial();
    app.invoke_reload_model_guides();
    assert!(lock(&shared).scene_serial() > serial);
    assert!(app.get_message().contains("Model assets reloaded"));
    let mut session = lock(&shared);
    assert_eq!(session.build_scene().stats.fgd_model_helpers, 0);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.camera(), camera);
    app.hide().unwrap();
}
