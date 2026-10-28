use super::*;
use source_formats::gamefs::{Mount, MountedPath};

fn image(minor: u32, format: u32, pixels: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0u8; 80];
    bytes[..4].copy_from_slice(b"VTF\0");
    bytes[4..8].copy_from_slice(&7u32.to_le_bytes());
    bytes[8..12].copy_from_slice(&minor.to_le_bytes());
    bytes[12..16].copy_from_slice(&80u32.to_le_bytes());
    bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&1u16.to_le_bytes());
    bytes[24..26].copy_from_slice(&1u16.to_le_bytes());
    bytes[52..56].copy_from_slice(&format.to_le_bytes());
    bytes[56] = 1;
    bytes[57..61].copy_from_slice(&u32::MAX.to_le_bytes());
    if minor >= 2 {
        bytes[63..65].copy_from_slice(&1u16.to_le_bytes());
    }
    bytes.extend_from_slice(pixels);
    bytes
}
fn fixture() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    let fs = GameFs {
        game_dir: directory.path().into(),
        base_dir: directory.path().into(),
        mounts: vec![MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.path().into()),
        }],
    };
    std::fs::write(directory.path().join("materials/icon.vmt"), "Sprite { $basetexture icon $spriteorientation vp_parallel $spriteorigin \"[0.5 0.5]\" $no_fullbright 1 }").unwrap();
    std::fs::write(
        directory.path().join("materials/icon.vtf"),
        image(2, 0, &[200, 100, 50, 0, 10, 20, 30, 255]),
    )
    .unwrap();
    (directory, fs)
}

fn linear(value: u8) -> f64 {
    let value = f64::from(value) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn encoded(value: f64) -> u8 {
    let value = if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn render_icons(
    context: &crate::GpuContext,
    renderer: &mut crate::Renderer,
    mesh: &crate::SceneMesh,
    materials: &crate::MaterialLibrary,
    camera: &crate::Camera,
) -> Vec<u8> {
    let gpu = renderer.upload(context, mesh, materials).unwrap();
    renderer.render_to_rgba(
        context,
        &gpu,
        camera,
        &crate::ViewOptions {
            grid_visible: false,
            ..Default::default()
        },
        65,
        65,
    )
}

#[test]
#[ignore = "requires an offscreen GPU; run explicitly with --ignored"]
fn additive_sprite_rgb_matches_independent_blend_reference_and_mixed_camera_order() {
    let (directory, fs) = fixture();
    let context = crate::GpuContext::new().unwrap();
    let mut renderer = crate::Renderer::new(&context);
    let mut materials = crate::MaterialLibrary::default();
    let document = map_core::Document::empty();
    let mut mesh =
        crate::SceneMesh::build(document.vmf(), &fs, &mut materials, &Default::default());
    let top = crate::Camera::looking_at(glam::Vec3::Z * 96.0, glam::Vec3::ZERO);
    let bottom = crate::Camera::looking_at(glam::Vec3::NEG_Z * 96.0, glam::Vec3::ZERO);
    let options = crate::ViewOptions {
        grid_visible: false,
        ..Default::default()
    };
    let center = (32 * 65 + 32) * 4;
    let clear = render_icons(&context, &mut renderer, &mesh, &materials, &top);
    std::fs::write(
        directory.path().join("materials/icon.vtf"),
        image(2, 0, &[0, 0, 0, 255, 0, 0, 0, 255]),
    )
    .unwrap();
    let library = Library::default();
    let black = library.load(&fs, "icon");
    mesh.icons = vec![Instance {
        center: [0.0; 3],
        world_size: [32.0; 2],
        asset: black.clone(),
        blend: Blend::Alpha,
    }];
    let raw = render_icons(&context, &mut renderer, &mesh, &materials, &top);
    assert_eq!(&raw[center..center + 3], &[0; 3]);
    mesh.icons[0].blend = Blend::Additive;
    assert_eq!(
        render_icons(&context, &mut renderer, &mesh, &materials, &top),
        clear
    );
    std::fs::write(
        directory.path().join("materials/icon.vtf"),
        image(2, 0, &[128, 64, 32, 128, 128, 64, 32, 128]),
    )
    .unwrap();
    let asset = Library::default().load(&fs, "icon");
    mesh.icons = vec![
        Instance {
            center: [0.0, 0.0, 1.0],
            world_size: [32.0; 2],
            asset: asset.clone(),
            blend: Blend::Alpha,
        },
        Instance {
            center: [0.0, 0.0, -1.0],
            world_size: [32.0; 2],
            asset: asset.clone(),
            blend: Blend::Additive,
        },
    ];
    let a = 128.0 / 255.0;
    let gpu = renderer.upload(&context, &mesh, &materials).unwrap();
    for (camera, far_additive) in [(&top, true), (&bottom, false)] {
        let pixels = renderer.render_to_rgba(&context, &gpu, camera, &options, 65, 65);
        for (channel, (source, background)) in [128, 64, 32]
            .into_iter()
            .zip([0.012, 0.014, 0.018])
            .enumerate()
        {
            let source = linear(source);
            let expected = if far_additive {
                (background + source * a) * (1.0 - a) + source * a
            } else {
                background * (1.0 - a) + source * a + source * a
            };
            assert!(
                pixels[center + channel].abs_diff(encoded(expected)) <= 2,
                "camera{far_additive} channel{channel}: {} expected{}",
                pixels[center + channel],
                encoded(expected)
            );
        }
        assert_eq!(pixels[center + 3], 255);
    }
    mesh.icons[0].center = [0.0; 3];
    mesh.icons[1].center = [0.0; 3];
    let tied = render_icons(&context, &mut renderer, &mesh, &materials, &top);
    mesh.icons.reverse();
    let reversed = render_icons(&context, &mut renderer, &mesh, &materials, &top);
    assert_ne!(&tied[center..center + 3], &reversed[center..center + 3]);
    assert!(Arc::ptr_eq(&mesh.icons[0].asset, &mesh.icons[1].asset));
    let mut occluder = map_core::Document::empty();
    occluder
        .edit("occluder", |tx| {
            map_core::edit::create_block(
                tx,
                glam::DVec3::new(-16.0, -16.0, 1.0),
                glam::DVec3::new(16.0, 16.0, 2.0),
                "icon",
            )
        })
        .unwrap();
    let mut blocked =
        crate::SceneMesh::build(occluder.vmf(), &fs, &mut materials, &Default::default());
    let baseline = render_icons(&context, &mut renderer, &blocked, &materials, &top);
    blocked.icons = mesh.icons;
    let hidden = render_icons(&context, &mut renderer, &blocked, &materials, &top);
    assert_eq!(&baseline[center..center + 4], &hidden[center..center + 4]);
}
#[test]
fn static_rgba_icon_retains_alpha_aspect_and_exact_winning_sources() {
    let (directory, fs) = fixture();
    let library = Library::default();
    let asset = library.load(&fs, "icon");
    assert_eq!(asset.report.status, "ready");
    assert_eq!(asset.report.world_size, [32.0, 16.0]);
    assert!(
        matches!(&asset.report.source, Some(Source::Directory { file, .. }) if file == &directory.path().join("materials/icon.vmt").display().to_string())
    );
    assert!(
        matches!(&asset.report.texture_source, Some(Source::Directory { file, .. }) if file == &directory.path().join("materials/icon.vtf").display().to_string())
    );
    let pixels = asset
        .material
        .as_ref()
        .unwrap()
        .base
        .as_ref()
        .unwrap()
        .decode_rgba8(0, 0, 0)
        .unwrap();
    assert_eq!(pixels.pixels, [200, 100, 50, 0, 10, 20, 30, 255]);
    assert!(Arc::ptr_eq(&asset, &library.load(&fs, "ICON")));
}

#[test]
fn normalized_image_assets_share_cache_without_collapsing_nested_materials_paths() {
    let (directory, fs) = fixture();
    let nested = directory.path().join("materials/materials");
    std::fs::create_dir(&nested).unwrap();
    std::fs::copy(
        directory.path().join("materials/icon.vmt"),
        nested.join("icon.vmt"),
    )
    .unwrap();
    let library = Library::default();
    let icon = library.load(&fs, "icon");
    for alias in ["icon.vmt", "materials/icon.vmt", "MATERIALS\\ICON.VMT"] {
        assert!(Arc::ptr_eq(&icon, &library.load(&fs, alias)));
    }
    assert!(Arc::ptr_eq(
        &icon,
        &library.load_material_path(&fs, "materials/icon.vmt")
    ));
    let distinct = library.load_material_path(&fs, "materials/materials/icon.vmt");
    assert_eq!(distinct.report.status, "ready");
    assert_eq!(distinct.report.material, "materials/materials/icon.vmt");
    assert!(!Arc::ptr_eq(&icon, &distinct));
    assert_eq!(library.0.borrow().entries.len(), 2);
    assert!(library.load(&fs, "sprites/icon.spr").material.is_none());
}
#[test]
fn legacy_no_depth_and_alpha_only_are_color_images_but_uv_data_is_not() {
    let (directory, fs) = fixture();
    for minor in [0, 1, 2] {
        std::fs::write(
            directory.path().join("materials/icon.vtf"),
            image(minor, 8, &[0, 255]),
        )
        .unwrap();
        assert_eq!(Library::default().load(&fs, "icon").report.status, "ready");
    }
    std::fs::write(
        directory.path().join("materials/icon.vtf"),
        image(2, 22, &[0; 4]),
    )
    .unwrap();
    let asset = Library::default().load(&fs, "icon");
    assert!(asset.material.is_none());
    assert!(asset.report.error.as_ref().unwrap().contains("LDR"));
}
#[test]
fn malformed_winner_never_falls_through_and_reload_clears_failure() {
    let (directory, mut fs) = fixture();
    let override_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(override_dir.path().join("materials")).unwrap();
    let path = override_dir.path().join("materials/icon.vmt");
    std::fs::write(&path, "Sprite { $basetexture icon").unwrap();
    fs.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(override_dir.path().into()),
        },
    );
    let mut library = crate::MaterialLibrary::default();
    let invalid = library.icons.load(&fs, "icon");
    assert!(invalid.material.is_none());
    assert!(
        matches!(&invalid.report.source, Some(Source::Directory { file, .. }) if file == &path.display().to_string())
    );
    std::fs::remove_file(path).unwrap();
    assert!(Arc::ptr_eq(&invalid, &library.icons.load(&fs, "icon")));
    library.reload(&fs);
    let ready = library.icons.load(&fs, "icon");
    assert_eq!(ready.report.status, "ready");
    assert!(
        matches!(&ready.report.source, Some(Source::Directory { mount, .. }) if mount == &directory.path().display().to_string())
    );
}
#[test]
fn admission_rejects_unknown_appearance_proxies_animation_and_dimension_bombs() {
    let (directory, fs) = fixture();
    for material in [
        "UnlitGeneric { $basetexture icon }",
        "Sprite { $basetexture icon $spriteorientation vp_parallel $spriteorigin \"[0.5 0.5]\" $color red }",
        "Sprite { Proxies { AnimatedTexture {} } }",
        "Sprite { $basetexture icon $spriteorientation oriented $spriteorigin \"[0.5 0.5]\" }",
        "Sprite { $basetexture icon $spriteorientation vp_parallel $spriteorigin \"[0 0]\" }",
    ] {
        std::fs::write(directory.path().join("materials/icon.vmt"), material).unwrap();
        assert!(
            Library::default().load(&fs, "icon").material.is_none(),
            "{material}"
        );
    }
    std::fs::write(
        directory.path().join("materials/icon.vmt"),
        "Sprite { $basetexture icon $spriteorientation vp_parallel $spriteorigin \"[0.5 0.5]\" }",
    )
    .unwrap();
    for (offset, value, size) in [(16, 513u16, 80 + 513 * 4), (24, 2, 96), (63, 2, 96)] {
        let mut bytes = image(2, 0, &[0; 8]);
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        bytes.resize(size, 0);
        assert!(Vtf::parse(bytes.clone()).is_ok());
        std::fs::write(directory.path().join("materials/icon.vtf"), bytes).unwrap();
        let asset = Library::default().load(&fs, "icon");
        assert!(asset.material.is_none());
        assert!(
            asset
                .report
                .error
                .as_ref()
                .unwrap()
                .contains("single-frame single-face depth-one LDR")
        );
    }
    let mut zero_depth = image(2, 0, &[0; 8]);
    zero_depth[63..65].copy_from_slice(&0u16.to_le_bytes());
    assert!(Vtf::parse(zero_depth.clone()).is_ok());
    std::fs::write(directory.path().join("materials/icon.vtf"), zero_depth).unwrap();
    assert!(
        Library::default()
            .load(&fs, "icon")
            .report
            .error
            .as_ref()
            .unwrap()
            .contains("zero frames or depth")
    );
}
#[test]
fn failed_admissions_count_toward_bounded_cache_without_path_normalization_escape() {
    let (_directory, fs) = fixture();
    let library = Library::default();
    assert!(library.load(&fs, "../icon").material.is_none());
    assert!(library.load(&fs, "/icon").material.is_none());
    for index in 0..MAX_ASSETS - 2 {
        assert!(
            library
                .load(&fs, &format!("missing{index}"))
                .material
                .is_none()
        );
    }
    let exhausted = library.load(&fs, "icon");
    assert!(
        exhausted
            .report
            .error
            .as_ref()
            .unwrap()
            .contains("cache limit")
    );
    assert_eq!(library.0.borrow().entries.len(), MAX_ASSETS);
}
