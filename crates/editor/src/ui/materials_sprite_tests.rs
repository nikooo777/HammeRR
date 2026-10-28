use std::sync::{Arc, Mutex};

use source_formats::fgd::Catalog;

use super::*;

#[test]
fn sprite_name_browse_callbacks_preserve_cancel_and_commit_full_virtual_identity() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = test_assets::game();
    for name in [
        "sprites/glow01",
        "root",
        "nested/foo",
        "materials/foo",
        "folder.with.dots/foo",
    ] {
        let path = directory.path().join(format!("materials/{name}.vmt"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "Sprite { $basetexture test/checker $spriteorientation vp_parallel $spriteorigin \"[0.5 0.5]\" }").unwrap();
    }
    let mut session = Session::new(Ok(game));
    session.set_definitions(Actor::Agent, Catalog::parse(r#"@PointClass size(4 4 4) sprite() = env_sprite [ model(sprite) : "Sprite Name" : "sprites/glow01.spr" scale(string) : "Scale" : "1" ]"#).unwrap());
    let original = "SPRITES\\Glow01.SPR";
    let ObjectId::Entity(id) = session
        .create_entity(
            Actor::Agent,
            "env_sprite",
            Some(DVec3::ZERO),
            BTreeMap::from([("model".into(), original.into())]),
        )
        .unwrap()
    else {
        panic!()
    };
    let before = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera();
    let selection = session.selected_ids();
    let generation = session.document_generation();
    let shared = Arc::new(Mutex::new(session));
    let window = MaterialWindow::new().unwrap();
    window.set_sprite_resource(true);
    bind_material_cancel(&window);
    bind_entity_material_choice(&window, shared.clone(), (id, "model".into(), generation));
    window.set_material(
        editor_core::sprite_resource::picker_name(original)
            .unwrap()
            .into(),
    );
    refresh_material_catalog(&window, &mut lock(&shared));
    refresh_material_window(&window, &mut lock(&shared));
    window.show().unwrap();
    crate::ui::tests::assert_ui_renders(window.window(), "sprite-name-material-picker");
    assert_eq!(window.get_material(), "sprites/glow01");
    assert!(window.get_available());
    assert_ne!(window.get_preview().size().width, 0);
    window.invoke_cancel();
    {
        let session = lock(&shared);
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.document().undo_labels().count(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.selected_ids(), selection);
    }

    for name in [
        "nested/foo",
        "root",
        "materials/foo",
        "folder.with.dots/foo",
    ] {
        window.set_material(name.into());
        refresh_material_window(&window, &mut lock(&shared));
        assert!(window.get_available());
        window.invoke_use_material();
        let mut session = lock(&shared);
        assert_eq!(
            session
                .document()
                .entity(id)
                .unwrap()
                .properties
                .get("model"),
            Some(format!("materials/{name}.vmt").as_str())
        );
        assert_eq!(session.document().undo_labels().count(), history + 1);
        session.undo(Actor::Agent).unwrap();
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.selected_ids(), selection);
    }
    for invalid in ["nested/foo.bar", "root.vmt", "../root"] {
        window.set_material(invalid.into());
        window.invoke_use_material();
        assert!(!window.get_detail().is_empty());
        assert_eq!(lock(&shared).document().vmf(), &before);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
    }
    window.set_material("root".into());
    lock(&shared).new_map(Actor::Agent, true).unwrap();
    let replaced = lock(&shared).document().vmf().clone();
    window.invoke_use_material();
    assert!(window.get_detail().contains("Map changed"));
    assert_eq!(lock(&shared).document().vmf(), &replaced);
}

#[test]
fn ordinary_material_entity_choice_retains_its_catalog_spelling() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (_directory, game) = test_assets::game();
    let mut session = Session::new(Ok(game));
    let ObjectId::Entity(id) = session
        .create_entity(
            Actor::Agent,
            "info_target",
            Some(DVec3::ZERO),
            BTreeMap::new(),
        )
        .unwrap()
    else {
        panic!()
    };
    let generation = session.document_generation();
    let shared = Arc::new(Mutex::new(session));
    let window = MaterialWindow::new().unwrap();
    window.set_material("test/checker".into());
    refresh_material_window(&window, &mut lock(&shared));
    bind_entity_material_choice(&window, shared.clone(), (id, "material".into(), generation));
    window.invoke_use_material();
    assert_eq!(
        lock(&shared)
            .document()
            .entity(id)
            .unwrap()
            .properties
            .get("material"),
        Some("test/checker")
    );
}
