use super::*;

#[test]
fn instance_dependency_build_details_share_counts_hashes_and_stale_launch_status() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, mut game, source, child) = editor_core::test_assets::instance_map(false);
    let grandchild = directory.path().join("leaf.vmf");
    std::fs::copy(&child, &grandchild).unwrap();
    let mut assembly = map_core::Document::empty();
    assembly
        .edit("nested brush references", |tx| {
            for x in [0.0, 64.0] {
                map_core::edit::create_entity(
                    tx,
                    "func_instance",
                    Some(glam::DVec3::new(x, 0.0, 0.0)),
                    &[("file".into(), "leaf.vmf".into())],
                    Vec::new(),
                )?;
            }
            Ok(())
        })
        .unwrap();
    std::fs::write(&child, assembly.vmf().write(&Default::default())).unwrap();
    let materials = directory.path().join("materials/dev");
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::write(
        materials.join("dev_measuregeneric01.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    game.mounts.push(source_formats::gamefs::MountedPath {
        ids: vec!["game".into()],
        mount: source_formats::gamefs::Mount::Directory(directory.path().to_owned()),
    });
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &source, true).unwrap();
    let before = session.document().vmf().clone();
    let history = session.history_snapshot();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    assert!(result.as_ref().unwrap().output.bsp.is_some());
    session.finish_compile(Actor::User, &result);
    let shared = std::sync::Arc::new(std::sync::Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    controller.refresh(&lock(&shared));
    assert!(controller.window.get_launch_ready());
    assert!(controller.window.get_details().contains(
        "Instances: 3 · Files: 2 · World brushes: 2 · Detail brushes: 2 · Nesting depth: 2"
    ));
    assert!(controller.window.get_details().contains("child.vmf"));
    assert!(controller.window.get_details().contains("leaf.vmf"));
    assert!(controller.window.get_details().contains("SHA256"));
    assert!(
        !controller
            .window
            .get_details()
            .contains("Instance terrain patches:")
    );
    assert!(
        !controller
            .window
            .get_details()
            .contains("Authored grid vertices:")
    );
    assert!(
        !controller
            .window
            .get_details()
            .contains("Expanded static lights:")
    );
    controller.window.show().unwrap();
    controller.window.window().take_snapshot().unwrap();
    controller
        .window
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(360.0, 450.0),
            delta_x: 0.0,
            delta_y: -10_000.0,
        });
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
    crate::ui::tests::save_ui_snapshot(controller.window.window(), "build-instance-dependencies");
    std::fs::remove_file(&grandchild).unwrap();
    controller.refresh(&lock(&shared));
    assert!(!controller.window.get_launch_ready());
    assert!(controller.window.get_summary().contains("dependencies"));
    assert!(
        controller
            .window
            .get_details()
            .contains("instance dependencies changed")
    );
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(lock(&shared).history_snapshot(), history);
}
