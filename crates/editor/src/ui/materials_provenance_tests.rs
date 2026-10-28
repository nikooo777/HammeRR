use super::*;
use source_formats::gamefs::{Mount, MountedPath};

#[test]
fn material_source_controls_share_winner_filters_and_keep_cached_preview_origin_unavailable() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, mut game) = test_assets::alpha_game();
    let higher = directory.path().join("override");
    std::fs::create_dir_all(higher.join("materials/test")).unwrap();
    std::fs::create_dir_all(directory.path().join("materials/maps/demo")).unwrap();
    std::fs::write(
        directory
            .path()
            .join("materials/maps/demo/checker_patch.vmt"),
        "LightmappedGeneric {}",
    )
    .unwrap();
    game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(higher.clone()),
        },
    );
    let mut session = Session::new(Ok(game));
    session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(64.0),
            "test/checker",
            false,
        )
        .unwrap();
    session.build_scene();
    session
        .begin_property_edit(session::PropertyTarget::World)
        .unwrap();
    let token = session.property_draft().unwrap().token;
    let original = session.document().vmf().clone();
    let history = session.history_snapshot();
    let shared = std::sync::Arc::new(std::sync::Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let window = MaterialWindow::new().unwrap();
    bind_material_preview_with_access(&window, &access);
    bind_material_search_with_access(&window, &access, || true);
    window.set_filter("checker".into());
    window.set_material("test/checker".into());
    window.invoke_search();
    assert_eq!(window.get_total(), 2);
    assert_eq!(window.get_source_ids().row_count(), 6);
    window.set_exclude_generated_map_materials(true);
    window.invoke_search();
    assert_eq!(window.get_total(), 1);
    window.set_source_index(5);
    window.invoke_search();
    assert_eq!(window.get_total(), 1);
    window.invoke_preview_material();
    let pixels = window.get_preview().to_rgba8().unwrap();
    assert!(window.get_detail().contains("Cached display"));
    std::fs::write(
        higher.join("materials/test/checker.vmt"),
        "LightmappedGeneric { $basetexture test/missing }",
    )
    .unwrap();
    window.invoke_search();
    assert_eq!(window.get_total(), 0);
    window.set_source_index(4);
    window.invoke_search();
    assert_eq!(window.get_total(), 1);
    let tile = window
        .get_rows()
        .row_data(0)
        .unwrap()
        .tiles
        .row_data(0)
        .unwrap();
    assert!(tile.source.contains(higher.to_str().unwrap()));
    window.invoke_preview_material();
    assert_eq!(
        window.get_preview().to_rgba8().unwrap().as_bytes(),
        pixels.as_bytes()
    );
    assert!(
        window
            .get_detail()
            .contains("Preview physical origin unavailable")
    );
    assert!(
        window
            .get_detail()
            .contains("Current VMT lookup: directory")
    );
    assert!(window.get_detail().contains(higher.to_str().unwrap()));
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(lock(&shared).property_draft().unwrap().token, token);
}
