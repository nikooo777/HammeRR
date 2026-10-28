use super::*;
use crate::{MaterialLibrary, SceneMesh};
use map_core::{Document, edit};
use source_formats::gamefs::{Mount, MountedPath};

fn texture(width: u16, pixels: &[[u8; 4]]) -> Vec<u8> {
    let mut bytes = vec![0; 80];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (at, value) in [(4, 7u32), (8, 2), (12, 80), (57, u32::MAX)] {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[16..18].copy_from_slice(&width.to_le_bytes());
    for at in [18, 24, 63] {
        bytes[at..at + 2].copy_from_slice(&1u16.to_le_bytes());
    }
    bytes[56] = 1;
    for pixel in pixels {
        bytes.extend(pixel);
    }
    bytes
}

fn fixture() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    std::fs::write(
        directory.path().join("materials/base.vtf"),
        texture(1, &[[255; 4]]),
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/normal.vtf"),
        texture(1, &[[255, 128, 128, 255]]),
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/test.vmt"),
        "LightmappedGeneric { $basetexture base $bumpmap normal }",
    )
    .unwrap();
    let fs = GameFs {
        game_dir: directory.path().into(),
        base_dir: directory.path().into(),
        mounts: vec![MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.path().into()),
        }],
    };
    (directory, fs)
}

fn document() -> Document {
    let mut document = Document::empty();
    document
        .edit("block", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-32.0, -32.0, -8.0),
                DVec3::new(32.0, 32.0, 0.0),
                "test",
            )
        })
        .unwrap();
    document
}

fn mesh(
    document: &Document,
    fs: &GameFs,
    materials: &mut MaterialLibrary,
    enabled: bool,
) -> SceneMesh {
    SceneMesh::build_with_normal_maps_filtered(
        document.vmf(),
        fs,
        materials,
        (&Default::default(), &Default::default()),
        &[],
        (&[], &Default::default()),
        (&Default::default(), &[]),
        None,
        enabled,
        |_| true,
    )
}

#[test]
fn normal_admission_is_lazy_bounded_and_reloads_failures_and_pixels() {
    let (directory, fs) = fixture();
    let document = document();
    let mut materials = MaterialLibrary::default();
    let initial = mesh(&document, &fs, &mut materials, false);
    let id = materials.load(&fs, "test");
    assert!(materials.get(id).normal.is_none());
    assert!(
        initial
            .vertices
            .iter()
            .all(|vertex| vertex.tangent == [0.0; 4])
    );
    std::fs::remove_file(directory.path().join("materials/normal.vtf")).unwrap();
    let unsupported = mesh(&document, &fs, &mut materials, true);
    assert_eq!(unsupported.normal_maps.rejected_faces, 6);
    assert_eq!(unsupported.vertices, initial.vertices);
    std::fs::write(
        directory.path().join("materials/normal.vtf"),
        texture(1, &[[128, 128, 255, 255]]),
    )
    .unwrap();
    assert_eq!(
        mesh(&document, &fs, &mut materials, true)
            .normal_maps
            .rejected_faces,
        6
    );
    materials.reload(&fs);
    let admitted = mesh(&document, &fs, &mut materials, true);
    assert_eq!(admitted.normal_maps.applied_faces, 6);
    assert_eq!(admitted.indices, initial.indices);
    assert_eq!((admitted.mins, admitted.maxs), (initial.mins, initial.maxs));
    assert_eq!(admitted.normal_maps.materials[0].dimensions, Some([1, 1]));
    assert!(admitted.normal_maps.materials[0].source.is_some());
    assert!(admitted.normal_maps.materials[0].texture_source.is_some());
    let generation = materials.generation();
    assert_eq!(
        mesh(&document, &fs, &mut materials, true)
            .normal_maps
            .applied_faces,
        6
    );
    assert_eq!(materials.generation(), generation);
    let mut cache = Library {
        bytes: MAX_CACHE_BYTES,
        ..Default::default()
    };
    assert!(
        cache
            .load(&fs, "test")
            .report
            .error
            .as_ref()
            .unwrap()
            .contains("64 MiB")
    );
    let asset = Library::default().load(&fs, "test");
    for index in 0..MAX_ASSETS {
        cache
            .entries
            .insert(format!("reserved{index}"), asset.clone());
    }
    assert!(!cache.load(&fs, "not-read").retained);
}

#[test]
fn normal_admission_rejects_new_material_settings_until_the_cached_base_is_reloaded() {
    let (directory, fs) = fixture();
    let document = document();
    let mut materials = MaterialLibrary::default();
    let initial = mesh(&document, &fs, &mut materials, false);
    let original = Material::load(&fs, "test").unwrap().unwrap();
    assert!(materials.find("test").unwrap().matches_material(&original));
    std::fs::write(directory.path().join("materials/test.vmt"),"LightmappedGeneric { $basetexture base $bumpmap normal $bumptransform \"translate .5 0\" }").unwrap();
    let changed = Material::load(&fs, "test").unwrap().unwrap();
    assert!(!materials.find("test").unwrap().matches_material(&changed));
    let rejected = mesh(&document, &fs, &mut materials, true);
    assert_eq!(rejected.vertices, initial.vertices);
    assert_eq!(rejected.normal_maps.rejected_faces, 6);
    assert!(rejected.normal_maps.errors[0].contains("settings changed"));
    materials.reload(&fs);
    let admitted = mesh(&document, &fs, &mut materials, true);
    assert_eq!(admitted.normal_maps.applied_faces, 6);
    assert_eq!(
        admitted.normal_maps.materials[0].transform,
        [1.0, 1.0, 0.5, 0.0]
    );
}

#[test]
fn normal_material_contract_rejects_unsupported_controls_without_discarding_base() {
    let (directory, fs) = fixture();
    for extra in [
        "$ssbump 1",
        "$bumpmap2 normal",
        "$bumpframe 1",
        "$bumptransform \"rotate 1\"",
        "$bumptransform \"scale -1 1\"",
        "$bumptransform \"scale 0 1\"",
        "$bumptransform \"scale NaN 1\"",
        "$bumptransform \"scale 2048 1\"",
        "$bumptransform \"translate 0 inf\"",
        "$bumptransform \"scale 1 1 scale 2 2\"",
        "$basetexturetransform \"translate 1 0\"",
        "%tooltexture base",
        "$detail base",
        "Proxies { AnimatedTexture {} }",
    ] {
        std::fs::write(
            directory.path().join("materials/test.vmt"),
            format!("LightmappedGeneric {{ $basetexture base $bumpmap normal {extra} }}"),
        )
        .unwrap();
        let asset = Library::default().load(&fs, "test");
        assert!(
            asset.image.is_none() && asset.report.error.is_some(),
            "{extra}"
        );
        assert!(
            crate::materials::MaterialInfo::load(&fs, "test")
                .base
                .is_some()
        );
    }
    std::fs::write(directory.path().join("materials/test.vmt"),"LightmappedGeneric { $basetexture base $bumpmap normal $bumptransform \"center 0 0 scale 2 3 rotate 0 translate .25 -.5\" }").unwrap();
    let asset = Library::default().load(&fs, "test");
    assert_eq!(asset.report.transform, [2.0, 3.0, 0.25, -0.5]);
    assert!(asset.image.is_some());
    for (at, value) in [
        (20, SSBUMP_FLAG),
        (20, 4),
        (20, 0x40),
        (20, 0x800),
        (52, 22),
    ] {
        let mut bytes = texture(1, &[[128, 128, 255, 255]]);
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        std::fs::write(directory.path().join("materials/normal.vtf"), bytes).unwrap();
        assert!(Library::default().load(&fs, "test").image.is_none());
    }
    std::fs::write(
        directory.path().join("materials/normal.vtf"),
        texture(1, &[[128, 128, 127, 255]]),
    )
    .unwrap();
    assert!(
        Library::default()
            .load(&fs, "test")
            .report
            .error
            .as_ref()
            .unwrap()
            .contains("Z≥128")
    );
}

#[test]
fn normal_admission_uses_shared_dxt_decoders_and_bounds_dimensions_before_decoding() {
    let (directory, fs) = fixture();
    for (format, prefix) in [
        (13, Vec::new()),
        (14, vec![255; 8]),
        (15, vec![255, 255, 0, 0, 0, 0, 0, 0]),
    ] {
        let mut bytes = texture(4, &[]);
        bytes[18..20].copy_from_slice(&4u16.to_le_bytes());
        bytes[52..56].copy_from_slice(&u32::to_le_bytes(format));
        bytes.extend(prefix);
        bytes.extend([0x1f, 0x84, 0x1f, 0x84, 0, 0, 0, 0]);
        std::fs::write(directory.path().join("materials/normal.vtf"), bytes).unwrap();
        let asset = Library::default().load(&fs, "test");
        assert!(asset.image.is_some(), "format{format}: {:?}", asset.report);
        assert_eq!(asset.report.dimensions, Some([4, 4]));
        assert_eq!(asset.image.as_ref().unwrap().pixels[2], 255);
    }
    std::fs::write(
        directory.path().join("materials/normal.vtf"),
        texture(1025, &vec![[128, 128, 255, 255]; 1025]),
    )
    .unwrap();
    assert!(
        Library::default()
            .load(&fs, "test")
            .report
            .error
            .as_ref()
            .unwrap()
            .contains("1–1024")
    );
    assert!(!Library::default().load(&fs, &"a".repeat(257)).retained);
}

#[test]
fn authored_uv_rotation_mirrors_and_skew_have_independent_world_tangent_expectations() {
    let mut side = document().vmf().world.solids[0].sides[0].clone();
    for u in [-1.0, 1.0] {
        for v in [-1.0, 1.0] {
            side.u_axis.axis = DVec3::X;
            side.v_axis.axis = DVec3::new(0.5, 1.0, 0.0);
            side.u_axis.scale = u;
            side.v_axis.scale = v;
            let actual = tangent(
                &side,
                DVec3::Z,
                &[DVec3::ZERO],
                [1, 1],
                [1.0, 1.0, 0.0, 0.0],
            )
            .unwrap();
            assert_eq!(actual, [u as f32, 0.0, 0.0, (u * v) as f32]);
            let t = DVec3::new(
                f64::from(actual[0]),
                f64::from(actual[1]),
                f64::from(actual[2]),
            );
            assert_eq!(DVec3::Z.cross(t) * f64::from(actual[3]), DVec3::Y * v);
        }
    }
    side.u_axis.axis = DVec3::Y;
    side.v_axis.axis = DVec3::NEG_X;
    side.u_axis.scale = 1.0;
    side.v_axis.scale = 1.0;
    assert_eq!(
        tangent(&side, DVec3::Z, &[], [1, 1], [1.0, 1.0, 0.0, 0.0]).unwrap(),
        [0.0, 1.0, 0.0, 1.0]
    );
    side.v_axis.axis = DVec3::Y;
    assert!(tangent(&side, DVec3::Z, &[], [1, 1], [1.0, 1.0, 0.0, 0.0]).is_err());
    side.u_axis.axis = DVec3::X;
    for offset in [f64::NAN, 1_000_001.0] {
        side.u_axis.offset = offset;
        assert!(
            tangent(
                &side,
                DVec3::Z,
                &[DVec3::ZERO],
                [1, 1],
                [1.0, 1.0, 0.0, 0.0]
            )
            .is_err()
        );
    }
}

fn render(
    context: &crate::GpuContext,
    renderer: &mut crate::Renderer,
    scene: &SceneMesh,
    materials: &MaterialLibrary,
    options: crate::ViewOptions,
) -> Vec<u8> {
    let gpu = renderer.upload(context, scene, materials).unwrap();
    renderer.render_to_rgba(
        context,
        &gpu,
        &crate::Camera::looking_at(glam::Vec3::new(0.0, 0.0, 96.0), glam::Vec3::ZERO),
        &options,
        65,
        65,
    )
}

fn expected_channel(normal: DVec3) -> u8 {
    let light = DVec3::new(0.35, 0.55, 0.75).normalize();
    let linear = 0.775 + 0.225 * normal.normalize().dot(light);
    ((1.055 * linear.powf(1.0 / 2.4) - 0.055) * 255.0).round() as u8
}

#[test]
#[ignore = "requires an offscreen GPU; run explicitly with --ignored"]
fn rendered_normal_maps_follow_signed_source_uv_and_preserve_disabled_diagnostic_pixels() {
    let (directory, fs) = fixture();
    let mut document = document();
    let context = crate::GpuContext::new().unwrap();
    let mut renderer = crate::Renderer::new(&context);
    let mut materials = MaterialLibrary::default();
    let off = crate::ViewOptions {
        grid_visible: false,
        ..Default::default()
    };
    let on = crate::ViewOptions {
        normal_maps: true,
        ..off
    };
    for sign in [-1.0, 1.0] {
        document
            .edit("UV frame", |tx| {
                let id = tx.vmf().world.solids[0].id;
                let solid = tx.solid_mut(id)?;
                for side in &mut solid.sides {
                    side.u_axis.axis = DVec3::X;
                    side.v_axis.axis = DVec3::Y;
                    side.u_axis.scale = sign;
                    side.v_axis.scale = 1.0;
                }
                Ok(())
            })
            .unwrap();
        let baseline = mesh(&document, &fs, &mut materials, false);
        let active = mesh(&document, &fs, &mut materials, true);
        let a = render(&context, &mut renderer, &baseline, &materials, off);
        let b = render(&context, &mut renderer, &active, &materials, on);
        assert_ne!(a, b);
        let center = (32 * 65 + 32) * 4;
        let expected = expected_channel(DVec3::new(sign, 1.0 / 255.0, 1.0 / 255.0));
        assert!(
            b[center].abs_diff(expected) <= 2,
            "sign{sign}: {} vs{expected}",
            b[center]
        );
        assert_eq!(a, render(&context, &mut renderer, &active, &materials, off));
        for surface in [
            crate::SurfaceView::Flat,
            crate::SurfaceView::Wireframe,
            crate::SurfaceView::LightmapDensity,
            crate::SurfaceView::SmoothingGroups,
        ] {
            assert_eq!(
                render(
                    &context,
                    &mut renderer,
                    &baseline,
                    &materials,
                    crate::ViewOptions { surface, ..off }
                ),
                render(
                    &context,
                    &mut renderer,
                    &active,
                    &materials,
                    crate::ViewOptions { surface, ..on }
                )
            );
        }
    }
    document
        .edit("magnify 2x1 normal texel centers", |tx| {
            let id = tx.vmf().world.solids[0].id;
            for side in &mut tx.solid_mut(id)?.sides {
                side.u_axis.scale = 64.0;
                side.v_axis.scale = 32.0;
                side.u_axis.offset = 0.25;
            }
            Ok(())
        })
        .unwrap();
    std::fs::write(
        directory.path().join("materials/normal.vtf"),
        texture(2, &[[255, 128, 128, 255], [128, 255, 128, 255]]),
    )
    .unwrap();
    for (translation, expected) in [
        (0.0, DVec3::new(1.0, 1.0 / 255.0, 1.0 / 255.0)),
        (0.5, DVec3::new(1.0 / 255.0, 1.0, 1.0 / 255.0)),
    ] {
        std::fs::write(directory.path().join("materials/test.vmt"),format!("LightmappedGeneric {{ $basetexture base $bumpmap normal $bumptransform \"translate {translation} 0\" }}")).unwrap();
        materials.reload(&fs);
        let active = mesh(&document, &fs, &mut materials, true);
        let pixels = render(&context, &mut renderer, &active, &materials, on);
        let center = (32 * 65 + 32) * 4;
        let expected = expected_channel(expected);
        assert!(
            pixels[center].abs_diff(expected) <= 2,
            "translation{translation}: actual RGBA {:?}, expected {:?}",
            &pixels[center..center + 4],
            [expected, expected, expected, 255],
        );
    }
}
