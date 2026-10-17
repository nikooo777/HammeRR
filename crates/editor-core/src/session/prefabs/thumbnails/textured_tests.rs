use super::*;
use crate::session::Actor;
use source_formats::gamefs::{Mount, MountedPath};

fn fixture() -> (Session, tempfile::TempDir, tempfile::TempDir) {
    let (mut session, library, assets, brush) = crate::test_assets::textured_prefabs();
    let stamp = session.prefab_report("", 0, 64, None)["snapshot"]
        .as_str()
        .unwrap()
        .to_owned();
    session
        .save_prefab(Actor::User, "Textured", &[brush], &stamp)
        .unwrap();
    (session, library, assets)
}

fn seed(session: &Session, style: ThumbnailStyle) -> Arc<RgbaImage> {
    let source = session.thumbnail_source("Textured").unwrap();
    let prepared = session.prepare_prefab_thumbnail(&source, style).unwrap();
    let image = Arc::new(RgbaImage {
        width: WIDTH,
        height: HEIGHT,
        pixels: vec![0; (WIDTH * HEIGHT * 4) as usize],
    });
    session
        .prefabs
        .thumbnails
        .cache
        .borrow_mut()
        .push_back(Cached {
            style,
            assets: prepared.assets,
            source: source.entry,
            text: source.text,
            epoch: (session.content_epoch, session.definitions_serial),
            thumbnail: Thumbnail {
                image: image.clone(),
                report: prepared.report,
            },
        });
    image
}

#[test]
fn textured_admission_is_headless_and_preserves_active_owned_placement_and_materials() {
    use sha2::{Digest, Sha256};
    let (mut session, _library, assets) = fixture();
    std::fs::write(
        assets.path().join("materials/test/prefab_base.vmt"),
        "LightmappedGeneric { $basetexture test/checker $bumpmap test/missing_normal }",
    )
    .unwrap();
    std::fs::write(
        assets.path().join("materials/test/checker.vmt"),
        "Patch { include \"materials/test/prefab_base.vmt\" insert { $surfaceprop concrete } }",
    )
    .unwrap();
    let mut view = session.view();
    view.normal_maps = true;
    session.set_view(view);
    let stamp = session.prefab_report("", 0, 64, None)["snapshot"]
        .as_str()
        .unwrap()
        .to_owned();
    session
        .preview_prefab(
            Actor::User,
            "Textured",
            glam::DVec3::X * 128.0,
            false,
            &stamp,
        )
        .unwrap();
    let document = session.document().vmf().clone();
    let draft = session.render_document().vmf().clone();
    let clipboard = session.clipboard_report();
    let history = session.history_snapshot();
    let camera = session.camera();
    let selected = session.selected_ids();
    let materials = (session.materials.len(), session.materials.generation());
    let serials = (
        session.scene_serial(),
        session.view_serial(),
        session.log_serial(),
    );
    let report = session.prefab_thumbnail_report_with_style("Textured", ThumbnailStyle::Textured);
    assert!(report["error"].is_null(), "{report}");
    assert_eq!(report["style"], "textured");
    assert_eq!(report["textures_loaded"], true);
    assert_eq!(report["normal_maps"], false);
    assert_eq!(report["authored_lighting"], false);
    assert_eq!(
        report["texture_assets"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(report["texture_assets"]["bindings"][0]["width"], 64);
    assert_eq!(report["texture_assets"]["bindings"][0]["height"], 32);
    assert_eq!(
        report["texture_assets"]["sources"][0]["sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    for source in report["texture_assets"]["sources"].as_array().unwrap() {
        let bytes = std::fs::read(assets.path().join(source["path"].as_str().unwrap())).unwrap();
        assert_eq!(source["sha256"], format!("{:x}", Sha256::digest(bytes)));
    }
    assert_eq!(report["graphics"]["initialized"], false);
    assert!(session.prefabs.thumbnails.cache.borrow().is_empty());
    assert_eq!(session.document().vmf(), &document);
    assert_eq!(session.render_document().vmf(), &draft);
    assert_eq!(session.clipboard_report(), clipboard);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.selected_ids(), selected);
    assert_eq!(
        (session.materials.len(), session.materials.generation()),
        materials
    );
    assert_eq!(
        (
            session.scene_serial(),
            session.view_serial(),
            session.log_serial()
        ),
        serials
    );
}

#[test]
fn textured_cache_rechecks_exact_bytes_and_winning_sources_without_erasing_flat() {
    let (mut session, library, assets) = fixture();
    let flat = seed(&session, ThumbnailStyle::Flat);
    let textured = seed(&session, ThumbnailStyle::Textured);
    assert!(!Arc::ptr_eq(&flat, &textured));
    assert!(Arc::ptr_eq(
        &textured,
        &session
            .cached_prefab_thumbnail_with_style("Textured", ThumbnailStyle::Textured)
            .unwrap()
            .unwrap()
            .image
    ));
    let path = assets.path().join("materials/test/checker.vtf");
    let modified = path.metadata().unwrap().modified().unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[80] ^= 0xff;
    std::fs::write(&path, &bytes).unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(path.metadata().unwrap().modified().unwrap(), modified);
    assert!(
        session
            .cached_prefab_thumbnail_with_style("Textured", ThumbnailStyle::Textured)
            .unwrap_err()
            .contains("source changed")
    );
    assert!(Arc::ptr_eq(
        &flat,
        &session
            .cached_prefab_thumbnail("Textured")
            .unwrap()
            .unwrap()
            .image
    ));
    seed(&session, ThumbnailStyle::Textured);
    let vmt = assets.path().join("materials/test/checker.vmt");
    let modified = vmt.metadata().unwrap().modified().unwrap();
    let bytes = std::fs::read(&vmt).unwrap();
    let mut changed = bytes.clone();
    changed.extend_from_slice(b"\n");
    std::fs::write(&vmt, &changed).unwrap();
    assert!(
        session
            .cached_prefab_thumbnail_with_style("Textured", ThumbnailStyle::Textured)
            .is_err()
    );
    std::fs::write(&vmt, bytes).unwrap();
    std::fs::File::open(&vmt)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    seed(&session, ThumbnailStyle::Textured);
    let higher = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(higher.path().join("materials/test")).unwrap();
    std::fs::copy(&vmt, higher.path().join("materials/test/checker.vmt")).unwrap();
    session.game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(higher.path().to_owned()),
        },
    );
    assert!(
        session
            .cached_prefab_thumbnail_with_style("Textured", ThumbnailStyle::Textured)
            .unwrap_err()
            .contains("source changed")
    );
    assert!(
        session
            .cached_prefab_thumbnail("Textured")
            .unwrap()
            .is_some()
    );
    assert!(!session.prefabs.thumbnails.renderer.initialized());
    seed(&session, ThumbnailStyle::Textured);
    let initial = session.thumbnail_source("Textured").unwrap();
    let path = library.path().join("Textured.vmf");
    std::fs::write(&path, format!("{}\n", initial.text)).unwrap();
    assert!(
        session
            .cached_thumbnail(&initial, ThumbnailStyle::Textured)
            .unwrap_err()
            .contains("Prefab changed")
    );
    seed(&session, ThumbnailStyle::Textured);
    let initial = session.thumbnail_source("Textured").unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        session
            .cached_thumbnail(&initial, ThumbnailStyle::Textured)
            .unwrap_err()
            .contains("no longer exists")
    );
    assert!(
        session
            .prefabs
            .thumbnails
            .cache
            .borrow()
            .iter()
            .all(|cached| cached.style != ThumbnailStyle::Textured)
    );
}

#[test]
fn textured_failures_never_substitute_flat_or_initialize_graphics() {
    let (mut session, _library, assets) = fixture();
    seed(&session, ThumbnailStyle::Flat);
    let path = assets.path().join("materials/test/checker.vmt");
    for material in [
        "LightmappedGeneric { $basetexture test/missing }",
        "LightmappedGeneric { $basetexture test/checker Proxies { AnimatedTexture {} } }",
        "LightmappedGeneric { $basetexture test/checker $basetexturetransform \"scale 2 1\" }",
        "LightmappedGeneric { $basetexture test/checker %tooltexture test/other }",
        "LightmappedGeneric { $basetexture test/checker $detail test/other }",
        "LightmappedGeneric { $basetexture test/checker $translucent 1 }",
        "WorldVertexTransition { $basetexture test/checker $basetexture2 test/checker }",
    ] {
        std::fs::write(&path, material).unwrap();
        let report =
            session.prefab_thumbnail_report_with_style("Textured", ThumbnailStyle::Textured);
        assert_eq!(report["style"], "textured");
        assert!(report["error"].is_string(), "{report}");
        assert!(
            session
                .prefab_thumbnail_with_style("Textured", true, ThumbnailStyle::Textured)
                .is_err()
        );
        assert!(
            session
                .cached_prefab_thumbnail("Textured")
                .unwrap()
                .is_some()
        );
        assert!(!session.prefabs.thumbnails.renderer.initialized());
    }
    std::fs::write(&path, "UnlitGeneric { $basetexture test/large }").unwrap();
    let large = std::fs::File::create(assets.path().join("materials/test/large.vtf")).unwrap();
    large.set_len(16 * 1024 * 1024 + 1).unwrap();
    assert!(
        session.prefab_thumbnail_report_with_style("Textured", ThumbnailStyle::Textured)["error"]
            .is_string()
    );
    std::fs::write(&path, "VertexLitGeneric { $basetexture test/checker }").unwrap();
    assert!(
        session.prefab_thumbnail_report_with_style("Textured", ThumbnailStyle::Textured)["error"]
            .is_null()
    );
    seed(&session, ThumbnailStyle::Textured);
    session.reload_material_assets(Actor::User);
    assert!(
        session
            .cached_prefab_thumbnail("Textured")
            .unwrap()
            .is_none()
    );
    assert!(
        session
            .cached_prefab_thumbnail_with_style("Textured", ThumbnailStyle::Textured)
            .unwrap()
            .is_none()
    );
}

#[test]
fn textured_mip_decode_budget_and_alpha_fail_before_graphics() {
    let (_session, _library, assets) = fixture();
    let mut bytes = std::fs::read(assets.path().join("materials/test/checker.vtf")).unwrap();
    let supported = source_formats::vtf::Vtf::parse(bytes.clone()).unwrap();
    textures::validate_texture(&supported, &mut 0).unwrap();
    for flags in [
        0x1, 0x4, 0x8, 0x80, 0x800, 0x4000, 0x8000, 0x10000, 0x08000000, 0x20000000,
    ] {
        let mut unsupported = supported.clone();
        unsupported.flags = flags;
        assert!(
            textures::validate_texture(&unsupported, &mut 0)
                .unwrap_err()
                .contains("static 2D LDR")
        );
    }
    bytes[83] = 0;
    let texture = source_formats::vtf::Vtf::parse(bytes).unwrap();
    let mut decoded = 64 * 1024 * 1024 - 1;
    assert!(
        textures::validate_texture(&texture, &mut decoded)
            .unwrap_err()
            .contains("64 MiB")
    );
    assert!(
        textures::validate_texture(&texture, &mut 0)
            .unwrap_err()
            .contains("opaque")
    );
}

#[test]
#[ignore = "requires a graphics adapter for actual textured prefab thumbnail validation"]
fn textured_gpu_images_cache_refresh_and_errors_preserve_owned_editor_state() {
    let (mut session, library, assets) = fixture();
    let stamp = session.prefab_report("", 0, 64, None)["snapshot"]
        .as_str()
        .unwrap()
        .to_owned();
    session
        .preview_prefab(
            Actor::Agent,
            "Textured",
            glam::DVec3::X * 128.0,
            false,
            &stamp,
        )
        .unwrap();
    let original = session.document().vmf().clone();
    let draft = session.render_document().vmf().clone();
    let clipboard = session.clipboard_report();
    let history = session.history_snapshot();
    let camera = session.camera();
    let flat = session.prefab_thumbnail("Textured", false).unwrap();
    let first = session
        .prefab_thumbnail_with_style("Textured", false, ThumbnailStyle::Textured)
        .unwrap();
    assert_ne!(first.image.pixels, flat.image.pixels);
    let cached = session
        .prefab_thumbnail_with_style("Textured", false, ThumbnailStyle::Textured)
        .unwrap();
    assert!(Arc::ptr_eq(&first.image, &cached.image));
    let refreshed = session
        .prefab_thumbnail_with_style("Textured", true, ThumbnailStyle::Textured)
        .unwrap();
    assert!(!Arc::ptr_eq(&cached.image, &refreshed.image));
    assert_eq!(first.image.pixels, refreshed.image.pixels);
    let path = assets.path().join("materials/test/checker.vtf");
    let mut bytes = std::fs::read(&path).unwrap();
    for pixel in bytes[80..].chunks_exact_mut(4) {
        pixel.copy_from_slice(&[220, 45, 40, 255]);
    }
    std::fs::write(&path, &bytes).unwrap();
    assert!(
        session
            .prefab_thumbnail_with_style("Textured", false, ThumbnailStyle::Textured)
            .is_err()
    );
    assert!(Arc::ptr_eq(
        &flat.image,
        &session.prefab_thumbnail("Textured", false).unwrap().image
    ));
    let changed = session
        .prefab_thumbnail_with_style("Textured", true, ThumbnailStyle::Textured)
        .unwrap();
    assert_ne!(changed.image.pixels, first.image.pixels);
    for index in 0..MAX_CACHE + 2 {
        let name = format!("Texture cache {index}");
        let source = session.thumbnail_source("Textured").unwrap();
        std::fs::write(library.path().join(format!("{name}.vmf")), source.text).unwrap();
        session
            .prefab_thumbnail_with_style(
                &name,
                false,
                if index % 2 == 0 {
                    ThumbnailStyle::Flat
                } else {
                    ThumbnailStyle::Textured
                },
            )
            .unwrap();
    }
    assert_eq!(session.prefabs.thumbnails.cache.borrow().len(), MAX_CACHE);
    assert_eq!(session.document().vmf(), &original);
    assert_eq!(session.render_document().vmf(), &draft);
    assert_eq!(session.clipboard_report(), clipboard);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
}
