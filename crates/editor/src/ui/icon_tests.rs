use super::*;
use source_formats::fgd::Catalog;

#[test]
fn fgd_icon_panel_shares_asset_provenance_and_failure_without_authoring() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = editor_core::test_assets::game();
    let material = directory.path().join("materials/test/icon.vmt");
    std::fs::write(&material, "Sprite { $basetexture test/checker $spriteorientation vp_parallel $spriteorigin \"[0.5 0.5]\" $no_fullbright 1 }").unwrap();
    let mut session = Session::new(Ok(game));
    session.set_definitions(
        Actor::Agent,
        Catalog::parse("@PointClass size(4 4 4) iconsprite(\"test/icon\") = icon_sample []")
            .unwrap(),
    );
    let ObjectId::Entity(id) = session
        .create_entity(
            Actor::Agent,
            "icon_sample",
            Some(DVec3::ZERO),
            BTreeMap::new(),
        )
        .unwrap()
    else {
        panic!()
    };
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera;
    let app = AppWindow::new().unwrap();
    refresh_panel(&app, &mut session);
    assert_eq!(
        app.get_entity_helper_status().as_str(),
        session.entity_helper_report(id).unwrap().status
    );
    assert!(
        app.get_entity_helper_status()
            .contains("FGD icon sprite: displayed")
    );
    assert!(
        app.get_entity_helper_status()
            .contains("live-camera unlit RGBA; box bounds retained")
    );
    assert!(app.get_entity_helper_status().contains("Icon VMT source:"));
    assert!(app.get_entity_helper_status().contains("Icon VTF source:"));
    app.show().unwrap();
    app.window().set_size(slint::PhysicalSize::new(1600, 960));
    super::tests::save_ui_snapshot(app.window(), "fgd-icon-sprite-inspector");
    std::fs::write(&material, "Sprite { invalid }").unwrap();
    session.reload_material_assets(Actor::Agent);
    refresh_panel(&app, &mut session);
    assert!(app.get_entity_helper_status().contains("unavailable_icon"));
    assert!(
        app.get_entity_helper_status()
            .contains("complete bounded KeyValues")
    );
    assert!(session.build_scene().icons.is_empty());
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.camera, camera);
    app.hide().unwrap();
}
