use super::*;
use map_core::surface::{FaceId, SurfaceEdit};
use slint::Model;
use source_formats::fgd::Catalog;
use std::sync::{Arc, Mutex};

#[test]
#[ignore = "requires a GPU adapter for the real Ui viewport"]
fn child_callbacks_work_then_release_ui_session_and_cached_resources() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = crate::test_assets::game();
    let sky_directory = directory.path().join("materials/skybox");
    std::fs::create_dir_all(&sky_directory).unwrap();
    for suffix in render::sky::SUFFIXES {
        std::fs::write(
            sky_directory.join(format!("lifecycle{suffix}.vmt")),
            "UnlitGeneric { $basetexture test/checker }",
        )
        .unwrap();
    }
    let mut vmf = map_core::Document::empty().vmf().clone();
    vmf.world.properties.set("skyname", "lifecycle");
    let source = directory.path().join("lifecycle.vmf");
    let source_text = vmf.write(&Default::default());
    std::fs::write(&source, &source_text).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &source, false).unwrap();
    let settings_directory = directory.path().join("settings");
    session.configure_recovery(Ok(settings_directory.clone()));
    session.set_definitions(
        Actor::User,
        Catalog::parse(
            "@PointClass = lifecycle [ rendercolor(color255) : \"Color\" : \"255 255 255\" ]",
        )
        .unwrap(),
    );
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "test/checker",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let face = FaceId {
        solid,
        side: session.document().solid(solid).unwrap().sides[0].id,
    };
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    let sky = session.skybox_preview().unwrap();
    assert!(sky.ready(), "{}", sky.summary());
    let weak_sky = Arc::downgrade(&sky);
    drop(sky);
    let shared = Arc::new(Mutex::new(session));
    let weak_session = Arc::downgrade(&shared);
    let app = AppWindow::new().unwrap();
    let ctx = GpuContext::new().unwrap();
    let ui = Rc::new(RefCell::new(Ui::new(shared.clone(), ctx.clone())));
    let weak_ui = Rc::downgrade(&ui);
    {
        let mut session = lock(&shared);
        let mesh = session.build_scene();
        ui.borrow_mut()
            .viewport
            .set_scene(&ctx, &mesh, session.materials());
    }

    show_surface_window(&app, &ui);
    let surface = ui.borrow().surface_window.as_ref().unwrap().clone_strong();
    surface.set_shift_u("23".into());
    surface.invoke_preview_edit();
    assert!(lock(&shared).has_face_preview());
    surface.invoke_apply();
    assert!(!lock(&shared).has_face_preview());
    assert_eq!(
        map_core::surface::side(lock(&shared).document(), face)
            .unwrap()
            .u_axis
            .offset,
        23.0,
    );
    surface.invoke_browse();
    let material = ui.borrow().material_window.as_ref().unwrap().clone_strong();
    material.set_filter("checker".into());
    material.invoke_search();
    assert!(material.get_total() > 0);
    material.set_material("test/checker".into());
    material.set_favorite(false);
    material.invoke_toggle_favorite();
    assert!(lock(&shared).is_material_favorite("test/checker"));
    material.invoke_use_material();
    assert!(!material.window().is_visible());
    surface.invoke_fit_atlas();
    let atlas = ui.borrow().atlas.as_ref().unwrap().window.clone_strong();
    assert!(atlas.get_loaded());
    atlas.invoke_browse();
    assert!(material.window().is_visible());

    show_entity_window(&app, &ui, false);
    let entity = ui.borrow().entity_window.as_ref().unwrap().clone_strong();
    entity.set_filter("lifecycle".into());
    entity.invoke_filter_changed();
    assert_eq!(entity.get_classes().row_count(), 1);
    entity.set_classname("lifecycle".into());
    entity.invoke_class_selected();
    assert!(entity.get_definition().contains("lifecycle"));
    entity.invoke_create();
    let entity_id = single_selected_entity(&lock(&shared)).unwrap();
    show_color_window(&app, &ui, "rendercolor");
    let color = ui.borrow().color_window.as_ref().unwrap().clone_strong();
    assert!(color.window().is_visible());
    color.set_red(42);
    color.set_green(84);
    color.set_blue(126);
    color.invoke_apply();
    assert_eq!(
        lock(&shared)
            .document()
            .entity(entity_id)
            .unwrap()
            .properties
            .get("rendercolor"),
        Some("42 84 126"),
    );

    lock(&shared)
        .select_faces(&[face], SelectMode::Replace)
        .unwrap();
    show_surface_window(&app, &ui);
    surface.invoke_browse();
    {
        let mut session = lock(&shared);
        session
            .edit_faces(
                Actor::Agent,
                &[face],
                SurfaceEdit {
                    shift_u: Some(99.0),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
        session
            .begin_property_edit(session::PropertyTarget::World)
            .unwrap();
    }
    let (document, rendered, history, selection, faces, camera, view, preview, property_token) = {
        let session = lock(&shared);
        (
            session.document().vmf().clone(),
            session.render_document().vmf().clone(),
            session.history_snapshot(),
            session.selected_ids(),
            session.selected_faces(),
            session.camera(),
            session.view(),
            session.face_preview_token().unwrap(),
            session.property_draft().unwrap().token,
        )
    };
    let favorites_path = settings_directory.join(material_favorites::FILE_NAME);
    let favorites = std::fs::read(&favorites_path).unwrap();
    let weak_surface = surface.as_weak();
    let weak_material = material.as_weak();
    let weak_atlas = atlas.as_weak();
    let weak_entity = entity.as_weak();
    let weak_color = color.as_weak();
    drop(ui);
    assert!(weak_ui.upgrade().is_none());

    surface.set_shift_u("1000".into());
    surface.invoke_preview_edit();
    surface.invoke_apply();
    surface.invoke_cancel();
    surface.invoke_browse();
    surface.invoke_fit_atlas();
    surface
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!surface.window().is_visible());
    material.set_total(12345);
    material.invoke_search();
    assert_eq!(material.get_total(), 12345);
    material.set_material("test/checker".into());
    material.set_favorite(true);
    material.invoke_toggle_favorite();
    material.invoke_use_material();
    atlas.invoke_browse();
    entity.set_definition("retained child".into());
    entity.invoke_filter_changed();
    entity.invoke_class_selected();
    assert_eq!(entity.get_definition().as_str(), "retained child");
    entity.invoke_create();
    entity.invoke_pick_surface();
    color.set_red(1);
    color.invoke_apply();
    {
        let session = lock(&shared);
        assert_eq!(session.document().vmf(), &document);
        assert_eq!(session.render_document().vmf(), &rendered);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.selected_ids(), selection);
        assert_eq!(session.selected_faces(), faces);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.view(), view);
        assert_eq!(session.face_preview_token(), Some(preview));
        assert_eq!(session.property_draft().unwrap().token, property_token);
        assert!(session.is_material_favorite("test/checker"));
    }
    assert_eq!(std::fs::read(&favorites_path).unwrap(), favorites);
    assert_eq!(std::fs::read_to_string(&source).unwrap(), source_text);
    for window in [
        surface.window(),
        material.window(),
        atlas.window(),
        entity.window(),
        color.window(),
    ] {
        window.hide().unwrap();
    }
    drop((surface, material, atlas, entity, color, app));
    drop(shared);
    assert!(weak_surface.upgrade().is_none());
    assert!(weak_material.upgrade().is_none());
    assert!(weak_atlas.upgrade().is_none());
    assert!(weak_entity.upgrade().is_none());
    assert!(weak_color.upgrade().is_none());
    assert!(weak_session.upgrade().is_none());
    assert!(weak_sky.upgrade().is_none());
}
