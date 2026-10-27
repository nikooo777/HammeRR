mod common;

use compiler::materials::{DimensionSource, Materials, ReflectivitySource};
use compiler::{LightingMode, Options, SkyQuality};
use glam::DVec3;
use map_core::{Document, Plane, edit};
use source_formats::bsp::{self, Bsp, LightingKind, TexData, TexInfo};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::Vmf;

const BASE_REFLECTIVITY: [f32; 3] = [0.0, 0.5, 0.0];
const ALBEDO_REFLECTIVITY: [f32; 3] = [0.5, 0.0, 0.0];

fn texture(width: u16, height: u16, reflectivity: [f32; 3], pixel: [u8; 4]) -> Vec<u8> {
    let mut bytes = vec![0; 80];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [(4, 7_u32), (8, 2), (12, 80)] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [(16, width), (18, height), (24, 1), (63, 1)] {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    for (axis, value) in reflectivity.into_iter().enumerate() {
        bytes[32 + axis * 4..36 + axis * 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[20..24].copy_from_slice(&source_formats::vtf::FLAG_NOMIP.to_le_bytes());
    bytes[56] = 1;
    bytes[57..61].copy_from_slice(&(-1_i32).to_le_bytes());
    for _ in 0..usize::from(width) * usize::from(height) {
        bytes.extend_from_slice(&pixel);
    }
    bytes
}

fn game(shader: &str, base: &str, extra: &str) -> (tempfile::TempDir, GameFs) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/validation/681");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::tempdir_in(root).unwrap();
    let materials = directory.path().join("materials/dev");
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::write(
        materials.join("dev_measuregeneric01.vmt"),
        format!("{shader} {{ $basetexture dev/{base} $bumpmap dev/missing_normal {extra} }}"),
    )
    .unwrap();
    for (name, bytes) in [
        ("base", texture(32, 16, BASE_REFLECTIVITY, [0, 255, 0, 255])),
        (
            "albedo",
            texture(64, 8, ALBEDO_REFLECTIVITY, [255, 0, 0, 255]),
        ),
    ] {
        std::fs::write(materials.join(format!("{name}.vtf")), bytes).unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn scene() -> Vmf {
    let mut document = Document::new(common::room());
    document
        .edit("bounce receiver", |tx| {
            edit::create_entity(
                tx,
                "light",
                Some(DVec3::new(-128.0, 0.0, 128.0)),
                &[
                    ("_light".into(), "255 255 255 200".into()),
                    ("_lightHDR".into(), "255 255 255 800".into()),
                    ("_constant_attn".into(), "1".into()),
                    ("_quadratic_attn".into(), "0".into()),
                ],
                Vec::new(),
            )?;
            edit::create_block(
                tx,
                DVec3::new(-96.0, -96.0, 48.0),
                DVec3::new(96.0, 96.0, 64.0),
                "dev/dev_measuregeneric01",
            )?;
            Ok(())
        })
        .unwrap();
    let mut source = document.vmf().clone();
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.lightmap_scale = 64;
    }
    source
}

fn compiled(source: &Vmf, game: &GameFs, lighting: LightingMode, bounces: u8) -> compiler::Output {
    let output = compiler::compile_with_options(
        source,
        game,
        Options {
            lighting,
            bounces,
            sky_quality: SkyQuality::Balanced,
            ..Default::default()
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    output
}

fn assert_metrics(bsp: &Bsp, albedo: bool) {
    let texdata = bsp.records::<TexData>().unwrap();
    assert!(!texdata.is_empty());
    for data in texdata {
        let (width, height, reflectivity) = if albedo {
            (64, 8, ALBEDO_REFLECTIVITY)
        } else {
            (32, 16, BASE_REFLECTIVITY)
        };
        assert_eq!((data.width, data.height), (width, height));
        assert_eq!((data.view_width, data.view_height), (width, height));
        assert_eq!(data.reflectivity, reflectivity);
    }
}

fn floor(bsp: &Bsp, kind: LightingKind) -> DVec3 {
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let lighting = bsp.lighting(kind).unwrap().unwrap();
    for face in bsp.lighting_faces(kind).unwrap() {
        let plane = planes[face.plane as usize];
        if plane.normal[2] < 0.99 || plane.dist.abs() > 0.01 {
            continue;
        }
        let info = &infos[face.texinfo as usize];
        let frame = compiler::lighting::LuxelFrame::new(
            Plane {
                normal: DVec3::Z,
                dist: 0.0,
            },
            info,
            face.lightmap_mins,
            face.lightmap_size,
        )
        .unwrap();
        let [width, height] = frame.dimensions();
        for t in 0..height {
            for s in 0..width {
                if frame.position(s, t).unwrap().length() < 1e-6 {
                    return DVec3::from_array(
                        lighting
                            .face(&face, info)
                            .unwrap()
                            .unwrap()
                            .samples(0, 0)
                            .unwrap()[t * width + s]
                            .linear(),
                    );
                }
            }
        }
    }
    panic!("Missing floor receiver");
}

#[test]
fn canonical_shaders_measure_the_selected_primary_texture_without_reading_normals() {
    for shader in [
        "LightmappedGeneric",
        "WorldVertexTransition",
        "WorldTwoTextureBlend",
    ] {
        for suppression in ["", "$nodiffusebumplighting 0", "$nodiffusebumplighting 1"] {
            let (directory, game) =
                game(shader, "base", &format!("$albedo dev/albedo {suppression}"));
            let mut materials = Materials::new(&game);
            let index = materials.load("dev/dev_measuregeneric01").unwrap();
            let metrics = materials.get(index).texture_metrics().unwrap();
            assert_eq!(
                metrics.dimension_source,
                DimensionSource::DiffuseTextureHeader
            );
            assert_eq!(
                metrics.reflectivity_source,
                ReflectivitySource::TextureHeader
            );
            let expected = if suppression.ends_with('1') {
                (32, 16, BASE_REFLECTIVITY)
            } else {
                (64, 8, ALBEDO_REFLECTIVITY)
            };
            assert_eq!(
                (metrics.width, metrics.height, metrics.reflectivity),
                expected
            );
            assert!(
                !directory
                    .path()
                    .join("materials/dev/missing_normal.vtf")
                    .exists()
            );
        }
    }
}

#[test]
fn effective_base_matches_whole_bsp_reference_and_changes_measured_world_bounce() {
    let (_active_directory, active) = game("LightmappedGeneric", "base", "$albedo dev/albedo");
    let (_reference_directory, reference) = game("LightmappedGeneric", "albedo", "");
    let (_old_directory, old_base) = game("LightmappedGeneric", "base", "");
    let source = scene();
    let before = source.clone();
    for (mode, bounces) in [
        (LightingMode::Fullbright, 0),
        (LightingMode::DirectLdr, 0),
        (LightingMode::DirectLdr, 1),
        (LightingMode::DirectLdrHdr, 1),
    ] {
        let actual = compiled(&source, &active, mode, bounces);
        let expected = compiled(&source, &reference, mode, bounces);
        let old = compiled(&source, &old_base, mode, bounces);
        if bounces != 0 {
            assert!(actual.stats.bounce_samples > 0 && actual.stats.bounce_work > 0);
        }
        let actual = actual.bsp.unwrap();
        let expected = expected.bsp.unwrap();
        let old = old.bsp.unwrap();
        assert!(
            actual.to_bytes() == expected.to_bytes(),
            "{mode:?} bounce {bounces}"
        );
        assert_metrics(&actual, true);
        assert_metrics(&old, false);
        if mode != LightingMode::Fullbright {
            let kinds = if mode == LightingMode::DirectLdrHdr {
                &[LightingKind::Ldr, LightingKind::Hdr][..]
            } else {
                &[LightingKind::Ldr][..]
            };
            for &kind in kinds {
                let selected = floor(&actual, kind);
                let old = floor(&old, kind);
                if bounces == 0 {
                    assert_eq!(selected, DVec3::ZERO);
                    assert_eq!(selected, old);
                } else {
                    assert!(
                        selected.x > 1.0 && selected.y == 0.0 && selected.z == 0.0,
                        "{selected}"
                    );
                    assert!(old.y > 1.0 && old.x == 0.0 && old.z == 0.0, "{old}");
                }
            }
        }
    }
    assert_eq!(source, before);
}

#[test]
fn suppression_one_preserves_original_base_bytes_with_missing_unused_albedo() {
    let (_active_directory, active) = game(
        "LightmappedGeneric",
        "base",
        "$albedo dev/not_present $nodiffusebumplighting 1",
    );
    let (_reference_directory, reference) =
        game("LightmappedGeneric", "base", "$nodiffusebumplighting 1");
    let source = scene();
    for (mode, bounces) in [
        (LightingMode::Fullbright, 0),
        (LightingMode::DirectLdrHdr, 1),
    ] {
        let actual = compiled(&source, &active, mode, bounces).bsp.unwrap();
        let expected = compiled(&source, &reference, mode, bounces).bsp.unwrap();
        assert!(actual.to_bytes() == expected.to_bytes());
        assert_metrics(&actual, false);
    }
}

#[test]
fn unsupported_or_unavailable_selected_albedo_preserves_prior_bsp_and_source() {
    let (directory, game) = game("LightmappedGeneric", "base", "$albedo dev/albedo");
    let material = directory
        .path()
        .join("materials/dev/dev_measuregeneric01.vmt");
    let albedo = directory.path().join("materials/dev/albedo.vtf");
    let destination = directory.path().join("previous.bsp");
    let source = common::room();
    let before = source.clone();
    let previous = compiled(&source, &game, LightingMode::Fullbright, 0)
        .bsp
        .unwrap()
        .to_bytes();
    std::fs::write(&destination, &previous).unwrap();
    for (selected, extra, bytes, diagnostic) in [
        ("dev/not_present", "", None, "not_present.vtf was not found"),
        ("../outside", "", None, "path"),
        ("dev/albedo", "", Some(b"malformed".to_vec()), "albedo.vtf"),
        (
            "dev/albedo",
            "",
            Some(texture(64, 8, [f32::NAN, 0.0, 0.0], [255; 4])),
            "reflectivity must be finite",
        ),
        (
            "dev/albedo",
            "%tooltexture dev/not_present",
            None,
            "not_present.vtf",
        ),
        ("dev/albedo", "%tooltexture \"\"", None, "%tooltexture"),
        (
            "dev/not_present",
            "%notooltexture 0",
            None,
            "not_present.vtf",
        ),
        (
            "dev/albedo",
            "%notooltexture 1",
            Some(b"malformed".to_vec()),
            "albedo.vtf",
        ),
        ("dev/albedo", "%notooltexture \"\"", None, "%notooltexture"),
        ("dev/albedo", "%notooltexture true", None, "%notooltexture"),
        (
            "dev/albedo",
            "$reflectivity \"0.5 0 0\"",
            None,
            "$reflectivity",
        ),
        ("dev/albedo", "$reflectivity \"\"", None, "$reflectivity"),
    ] {
        std::fs::write(
            &material,
            format!("LightmappedGeneric {{ $basetexture dev/base $bumpmap dev/missing_normal $albedo \"{selected}\" {extra} }}"),
        )
        .unwrap();
        let bytes = bytes.unwrap_or_else(|| texture(64, 8, ALBEDO_REFLECTIVITY, [255, 0, 0, 255]));
        std::fs::write(&albedo, bytes).unwrap();
        let output = compiler::compile(&source, &game);
        assert!(
            output.report.has_errors() && output.bsp.is_none(),
            "{:?}",
            output.report
        );
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains(diagnostic)),
            "{:?}",
            output.report
        );
        if [
            "%tooltexture \"\"",
            "%notooltexture \"\"",
            "%notooltexture true",
        ]
        .contains(&extra)
            || extra.starts_with("$reflectivity")
        {
            assert!(output.report.diagnostics.iter().any(|item| {
                item.message
                    .contains("materials/dev/dev_measuregeneric01.vmt")
            }));
        }
        assert!(!output.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), previous);
        assert_eq!(source, before);
    }
}
