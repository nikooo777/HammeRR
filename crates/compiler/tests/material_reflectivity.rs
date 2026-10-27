mod common;

use compiler::materials::{DimensionSource, Materials, ReflectivitySource};
use compiler::{LightingMode, Options, SkyQuality};
use glam::DVec3;
use map_core::{Document, Plane, edit};
use source_formats::bsp::{self, Bsp, LightingKind, TexData, TexInfo};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::Vmf;

const MATERIAL: &str = "dev/dev_measuregeneric01";
const AUTHORED: [f32; 3] = [0.5, 0.0, 0.0];

fn texture(width: u16, height: u16, reflectivity: [f32; 3]) -> Vec<u8> {
    let mut bytes = vec![0u8; 80];
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
        bytes.extend_from_slice(&[32, 160, 64, 255]);
    }
    bytes
}

fn game(shader: &str, fields: &str) -> (tempfile::TempDir, GameFs) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/validation/685");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::tempdir_in(root).unwrap();
    let materials = directory.path().join("materials/dev");
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::write(
        materials.join("dev_measuregeneric01.vmt"),
        format!("{shader} {{ {fields} }}"),
    )
    .unwrap();
    for (name, width, height, reflectivity) in [
        ("base", 32, 16, [0.0, 0.5, 0.0]),
        ("albedo", 64, 8, [0.0, 0.0, 0.5]),
        ("reference_base", 32, 16, AUTHORED),
        ("reference_albedo", 64, 8, AUTHORED),
        ("nonfinite", 32, 16, [f32::NAN, 0.0, 0.0]),
    ] {
        std::fs::write(
            materials.join(format!("{name}.vtf")),
            texture(width, height, reflectivity),
        )
        .unwrap();
    }
    std::fs::write(materials.join("corrupt.vtf"), b"not a VTF").unwrap();
    let mut fs = common::empty_game();
    fs.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    (directory, fs)
}

fn scene() -> Vmf {
    let mut document = Document::new(common::room());
    document
        .edit("reflectivity contrast", |tx| {
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
                MATERIAL,
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

fn assert_metrics(bsp: &Bsp, width: i32, height: i32, reflectivity: [f32; 3]) {
    let records = bsp.records::<TexData>().unwrap();
    assert!(!records.is_empty());
    for row in records {
        assert_eq!(
            (row.width, row.height, row.view_width, row.view_height),
            (width, height, width, height)
        );
        assert_eq!(row.reflectivity, reflectivity);
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
    panic!("Missing floor contrast receiver");
}

#[test]
fn authored_reflectivity_keeps_selected_header_dimensions_and_independent_provenance() {
    for shader in [
        "LightmappedGeneric",
        "WorldVertexTransition",
        "WorldTwoTextureBlend",
    ] {
        for (fields, dimensions) in [
            (
                "$basetexture dev/base $reflectivity \"[0.5 0 0]\"",
                (32, 16),
            ),
            (
                "$basetexture dev/nonfinite $reflectivity \"[0.5 0 0]\"",
                (32, 16),
            ),
            (
                "$basetexture dev/base $bumpmap dev/absent_normal $albedo dev/albedo $reflectivity \"[0.5 0 0]\"",
                (64, 8),
            ),
        ] {
            let (directory, game) = game(shader, fields);
            let mut materials = Materials::new(&game);
            let index = materials.load(MATERIAL).unwrap();
            let metrics = materials.get(index).texture_metrics().unwrap();
            assert_eq!(
                metrics.dimension_source,
                DimensionSource::DiffuseTextureHeader
            );
            assert_eq!(
                metrics.reflectivity_source,
                ReflectivitySource::AuthoredMaterial
            );
            assert_eq!((metrics.width, metrics.height), dimensions);
            assert_eq!(metrics.reflectivity, [0.5, 0.0, 0.0]);
            assert!(
                !directory
                    .path()
                    .join("materials/dev/absent_normal.vtf")
                    .exists()
            );
        }
    }
}

#[test]
fn mixed_linear_components_are_emitted_without_color_rescaling() {
    let (_directory, game) = game(
        "LightmappedGeneric",
        "$basetexture dev/base $reflectivity \"[0.125 1 0.25]\"",
    );
    let source = common::room();
    let before = source.clone();
    let bsp = compiled(&source, &game, LightingMode::Fullbright, 0)
        .bsp
        .unwrap();
    assert_metrics(&bsp, 32, 16, [0.125, 1.0, 0.25]);
    assert_eq!(source, before);
}

#[test]
fn authored_reflectivity_matches_header_reference_and_changes_actual_one_bounce() {
    let source = scene();
    let before = source.clone();
    for (fields, reference, dimensions, old_color) in [
        (
            "$basetexture dev/base",
            "$basetexture dev/reference_base",
            (32, 16),
            DVec3::Y,
        ),
        (
            "$basetexture dev/base $bumpmap dev/absent_normal $albedo dev/albedo",
            "$basetexture dev/reference_albedo $bumpmap dev/absent_normal",
            (64, 8),
            DVec3::Z,
        ),
    ] {
        let (_actual_directory, actual) = game(
            "LightmappedGeneric",
            &format!("{fields} $reflectivity \"[0.5 0 0]\""),
        );
        let (_reference_directory, reference) = game("LightmappedGeneric", reference);
        let (_old_directory, old) = game("LightmappedGeneric", fields);
        for (mode, bounces) in [
            (LightingMode::Fullbright, 0),
            (LightingMode::DirectLdr, 0),
            (LightingMode::DirectLdrHdr, 1),
        ] {
            let output = compiled(&source, &actual, mode, bounces);
            if bounces != 0 {
                assert!(output.stats.bounce_samples > 0 && output.stats.bounce_work > 0);
            }
            let actual = output.bsp.unwrap();
            let reference = compiled(&source, &reference, mode, bounces).bsp.unwrap();
            let old = compiled(&source, &old, mode, bounces).bsp.unwrap();
            assert_eq!(actual.to_bytes(), reference.to_bytes());
            assert_metrics(&actual, dimensions.0, dimensions.1, [0.5, 0.0, 0.0]);
            if mode == LightingMode::Fullbright {
                continue;
            }
            let kinds = if bounces == 0 {
                &[LightingKind::Ldr][..]
            } else {
                &[LightingKind::Ldr, LightingKind::Hdr][..]
            };
            for &kind in kinds {
                let selected = floor(&actual, kind);
                let header = floor(&old, kind);
                if bounces == 0 {
                    assert_eq!(selected, DVec3::ZERO);
                    assert_eq!(selected, header);
                } else {
                    assert!(
                        selected.x > 1.0 && selected.y == 0.0 && selected.z == 0.0,
                        "{selected}"
                    );
                    assert!(
                        header.dot(old_color) > 1.0 && header.dot(DVec3::X) == 0.0,
                        "{header}"
                    );
                }
            }
        }
    }
    assert_eq!(source, before);
}

#[test]
fn authored_reflectivity_replaces_unused_nonfinite_header_rho() {
    let (_actual_directory, actual) = game(
        "LightmappedGeneric",
        "$basetexture dev/nonfinite $reflectivity \"[0.5 0 0]\"",
    );
    let (_reference_directory, reference) =
        game("LightmappedGeneric", "$basetexture dev/reference_base");
    let source = scene();
    let before = source.clone();
    for (mode, bounces) in [
        (LightingMode::Fullbright, 0),
        (LightingMode::DirectLdrHdr, 1),
    ] {
        let actual = compiled(&source, &actual, mode, bounces).bsp.unwrap();
        let reference = compiled(&source, &reference, mode, bounces).bsp.unwrap();
        assert_eq!(actual.to_bytes(), reference.to_bytes());
        assert_metrics(&actual, 32, 16, [0.5, 0.0, 0.0]);
    }
    assert_eq!(source, before);
}

#[test]
fn malformed_overrides_and_unavailable_dimensions_preserve_prior_bsp_and_source() {
    let (directory, game) = game("LightmappedGeneric", "$basetexture dev/base");
    std::fs::write(
        directory.path().join("materials/_rt_albedo.vtf"),
        texture(32, 16, [0.0, 0.5, 0.0]),
    )
    .unwrap();
    let source = common::room();
    let before = source.clone();
    let previous = compiled(&source, &game, LightingMode::Fullbright, 0)
        .bsp
        .unwrap()
        .to_bytes();
    let destination = directory.path().join("previous.bsp");
    let material = directory
        .path()
        .join("materials/dev/dev_measuregeneric01.vmt");
    for (fields, token) in [
        (
            "$basetexture dev/base $bumpmap dev/absent_normal $albedo _rt_albedo $reflectivity \"[0.5 0 0]\"",
            "_rt_albedo",
        ),
        (
            "$basetexture dev/base $reflectivity \"0.5\"",
            "$reflectivity",
        ),
        (
            "$basetexture dev/base $reflectivity \"{0.5 0 0}\"",
            "$reflectivity",
        ),
        (
            "$basetexture dev/base $reflectivity \"[red 0 0]\"",
            "$reflectivity",
        ),
        (
            "$basetexture dev/base $reflectivity \"[0.5 0]\"",
            "$reflectivity",
        ),
        (
            "$basetexture dev/base $reflectivity \"[0.5 0 0 0]\"",
            "$reflectivity",
        ),
        (
            "$basetexture dev/base $reflectivity \"[NaN 0 0]\"",
            "$reflectivity",
        ),
        (
            "$basetexture dev/base $reflectivity \"[-0.1 0 0]\"",
            "$reflectivity",
        ),
        (
            "$basetexture dev/base $reflectivity \"[1.1 0 0]\"",
            "$reflectivity",
        ),
        (
            "$basetexture dev/base $reflectivity \"[0.5\u{a0}0 0]\"",
            "$reflectivity",
        ),
        ("$reflectivity \"[0.5 0 0]\"", "$reflectivity"),
        (
            "$basetexture dev/not_present $reflectivity \"[0.5 0 0]\"",
            "not_present.vtf",
        ),
        (
            "$basetexture dev/corrupt $reflectivity \"[0.5 0 0]\"",
            "corrupt.vtf",
        ),
        ("$basetexture dev/nonfinite", "reflectivity must be finite"),
        (
            "$basetexture dev/base $bumpmap dev/absent_normal $albedo dev/not_present $reflectivity \"[0.5 0 0]\"",
            "not_present.vtf",
        ),
        (
            "$basetexture dev/base $reflectivity \"[0.5 0 0]\" %tooltexture dev/not_present",
            "not_present.vtf",
        ),
        (
            "$basetexture dev/base $bumpmap dev/absent_normal $albedo dev/albedo $reflectivity \"[0.5 0 0]\" %tooltexture dev/corrupt",
            "corrupt.vtf",
        ),
    ] {
        std::fs::write(&material, format!("LightmappedGeneric {{ {fields} }}")).unwrap();
        std::fs::write(&destination, &previous).unwrap();
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
                .any(|item| item.message.contains(token)),
            "{:?}",
            output.report
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), previous);
        assert_eq!(source, before);
    }
}
