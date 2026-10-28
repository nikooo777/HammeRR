use super::*;
use slint::Model;
use source_formats::fgd::Catalog;

#[test]
fn solid_class_picker_and_shared_tie_operation_accept_visclusters() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("authoring needs no mounted assets".into()));
    session.set_definitions(
        Actor::User,
        Catalog::parse("@SolidClass color(180 180 0) = func_viscluster []").unwrap(),
    );
    let brush = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "tools/toolstrigger",
            false,
        )
        .unwrap();
    let window = EntityWindow::new().unwrap();
    window.set_brush_mode(true);
    window.set_filter("viscluster".into());
    window.set_classname("func_viscluster".into());
    refresh_entity_window(&window, &session);
    assert_eq!(window.get_classes().row_count(), 1);
    assert_eq!(
        window.get_classes().row_data(0).unwrap().classname,
        "func_viscluster"
    );
    assert!(window.get_definition().contains("func_viscluster (solid)"));
    window.show().unwrap();
    super::tests::assert_dialog_button_visible(&window, "Tie selected brushes to entity");
    let entity = session
        .tie_to_entity(
            Actor::User,
            &[brush],
            &window.get_classname(),
            BTreeMap::new(),
        )
        .unwrap();
    let ObjectId::Entity(id) = entity else {
        unreachable!()
    };
    assert_eq!(
        session.document().entity(id).unwrap().classname(),
        Some("func_viscluster")
    );
    assert_eq!(session.document().entity(id).unwrap().solids.len(), 1);
    session.undo(Actor::User).unwrap();
    assert!(session.document().entity(id).is_none());
    assert!(
        session
            .document()
            .solid(match brush {
                ObjectId::Solid(id) => id,
                _ => unreachable!(),
            })
            .is_some()
    );

    let (directory, game) = crate::test_assets::game();
    let materials = directory.path().join("materials/dev");
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::write(
        materials.join("dev_measuregeneric01.vmt"),
        "LightmappedGeneric { $basetexture test/checker }",
    )
    .unwrap();
    let path = directory.path().join("visclusters.vmf");
    let mut document = map_core::Document::new(crate::test_assets::room());
    document
        .edit("visibility doorway", |tx| {
            for (mins, maxs) in [
                ([-8.0, -256.0, 0.0], [8.0, -64.0, 256.0]),
                ([-8.0, 64.0, 0.0], [8.0, 256.0, 256.0]),
            ] {
                map_core::edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "test/checker",
                )?;
            }
            Ok(())
        })
        .unwrap();
    std::fs::write(&path, document.vmf().write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &path, true).unwrap();
    let brush = session
        .create_block(
            Actor::User,
            DVec3::new(-250.0, -250.0, 8.0),
            DVec3::new(250.0, 250.0, 248.0),
            "test/checker",
            false,
        )
        .unwrap();
    session
        .tie_to_entity(Actor::User, &[brush], "func_viscluster", BTreeMap::new())
        .unwrap();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    session.finish_compile(Actor::User, &result);
    let result = result.unwrap();
    assert!(result.output.bsp.is_some(), "{:?}", result.output.report);
    assert_eq!(result.output.stats.viscluster_groups, 1);
    let shared = std::sync::Arc::new(std::sync::Mutex::new(session));
    let controller = crate::compile_ui::Controller::new(&shared).unwrap();
    controller.show(&lock(&shared)).unwrap();
    let details = controller.window.get_details();
    assert!(details.contains("emitted clusters"));
    assert!(details.contains("solved leaf clusters"));
    assert!(details.contains("1 helpers · 1 effective groups"));
    controller.window.window().take_snapshot().unwrap();
    controller
        .window
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(360.0, 450.0),
            delta_x: 0.0,
            delta_y: -10000.0,
        });
    super::tests::assert_ui_renders(controller.window.window(), "build-visclusters");
}
