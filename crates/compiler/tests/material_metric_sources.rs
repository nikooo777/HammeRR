mod common;

use compiler::materials::{DimensionSource, Materials, ReflectivitySource};
use compiler::{LightingMode, Options, SkyQuality};
use glam::DVec3;
use map_core::{Document, Plane, edit};
use source_formats::bsp::{self, Bsp, LightingKind, TexData, TexInfo};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::Vmf;

const MATERIAL: &str = "dev/dev_measuregeneric01";
const GREEN: [f32; 3] = [0.0, 0.5, 0.0];
const BLUE: [f32; 3] = [0.0, 0.0, 0.5];
const TOOL_RED: [f32; 3] = [0.75, 0.0, 0.0];
const AUTHORED: [f32; 3] = [0.125, 0.25, 0.75];
const NO_PREVIEW_DIAGNOSTIC: &str = "SDK no-preview dimensions (128x128) selected for ";

fn texture(width: u16, height: u16, rho: [f32; 3]) -> Vec<u8> {
    let mut bytes = vec![0u8; 80];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [(4, 7_u32), (8, 2), (12, 80)] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [(16, width), (18, height), (24, 1), (63, 1)] {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    for (axis, value) in rho.into_iter().enumerate() {
        bytes[32 + axis * 4..36 + axis * 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[56] = 1;
    bytes[57..61].copy_from_slice(&(-1_i32).to_le_bytes());
    for _ in 0..usize::from(width) * usize::from(height) {
        bytes.extend_from_slice(&[192, 32, 16, 255]);
    }
    bytes
}

fn game(fields: &str) -> (tempfile::TempDir, GameFs) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/validation/687");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::tempdir_in(root).unwrap();
    let materials = directory.path().join("materials/dev");
    std::fs::create_dir_all(&materials).unwrap();
    write_material(directory.path(), fields);
    for (name, width, height, rho) in [
        ("base", 4, 2, GREEN),
        ("albedo", 2, 4, BLUE),
        ("tool", 8, 1, TOOL_RED),
        ("reference_green", 8, 1, GREEN),
        ("reference_blue", 8, 1, BLUE),
        ("reference_authored", 8, 1, AUTHORED),
        ("reference_no_preview", 128, 128, GREEN),
        ("reference_no_preview_authored", 128, 128, AUTHORED),
        ("nonfinite", 4, 2, [f32::NAN, 0.0, 0.0]),
        ("tool_nonfinite_rho", 8, 1, [f32::NAN, 0.0, 0.0]),
    ] {
        std::fs::write(
            materials.join(format!("{name}.vtf")),
            texture(width, height, rho),
        )
        .unwrap();
    }
    std::fs::write(materials.join("corrupt.vtf"), b"not a VTF").unwrap();
    std::fs::write(
        directory.path().join("materials/_rt_metric.vtf"),
        b"unused runtime poison",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/_rt_provider.vtf"),
        texture(16, 16, TOOL_RED),
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/0.vtf"),
        texture(16, 16, TOOL_RED),
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/0x10.vtf"),
        texture(16, 16, TOOL_RED),
    )
    .unwrap();
    let mut fs = common::empty_game();
    fs.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    (directory, fs)
}

fn write_material(root: &std::path::Path, fields: &str) {
    std::fs::write(
        root.join("materials/dev/dev_measuregeneric01.vmt"),
        format!("LightmappedGeneric {{ {fields} }}"),
    )
    .unwrap();
}

fn scene() -> Vmf {
    let mut document = Document::new(common::room());
    document
        .edit("metric provider contrast", |tx| {
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

fn compiled(source: &Vmf, fs: &GameFs, lighting: LightingMode, bounces: u8) -> compiler::Output {
    let output = compiler::compile_with_options(
        source,
        fs,
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

fn assert_metrics(bsp: &Bsp, dimensions: (i32, i32), rho: [f32; 3]) {
    let records = bsp.records::<TexData>().unwrap();
    assert!(!records.is_empty());
    for row in records {
        assert_eq!((row.width, row.height), dimensions);
        assert_eq!((row.view_width, row.view_height), dimensions);
        assert_eq!(row.reflectivity, rho);
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
    panic!("Missing provider contrast receiver");
}

#[test]
fn dimensions_and_reflectivity_keep_independent_literal_provenance() {
    for (fields, dimensions, rho, dimension_source, reflectivity_source) in [
        (
            "$basetexture dev/base",
            (4, 2),
            GREEN,
            DimensionSource::DiffuseTextureHeader,
            ReflectivitySource::TextureHeader,
        ),
        (
            "$basetexture dev/base $bumpmap dev/absent_normal $albedo dev/albedo",
            (2, 4),
            BLUE,
            DimensionSource::DiffuseTextureHeader,
            ReflectivitySource::TextureHeader,
        ),
        (
            "$basetexture dev/base %tooltexture dev/tool",
            (8, 1),
            GREEN,
            DimensionSource::ToolTextureHeader,
            ReflectivitySource::TextureHeader,
        ),
        (
            "$basetexture dev/base $bumpmap dev/absent_normal $albedo dev/albedo %tooltexture dev/tool",
            (8, 1),
            BLUE,
            DimensionSource::ToolTextureHeader,
            ReflectivitySource::TextureHeader,
        ),
        (
            "$basetexture dev/base %tooltexture dev/tool $reflectivity \"[0.125 0.25 0.75]\"",
            (8, 1),
            AUTHORED,
            DimensionSource::ToolTextureHeader,
            ReflectivitySource::AuthoredMaterial,
        ),
        (
            "%tooltexture dev/tool",
            (8, 1),
            [0.5; 3],
            DimensionSource::ToolTextureHeader,
            ReflectivitySource::CompileDefault,
        ),
        (
            "%tooltexture dev/tool $reflectivity \"[0.125 0.25 0.75]\"",
            (8, 1),
            AUTHORED,
            DimensionSource::ToolTextureHeader,
            ReflectivitySource::AuthoredMaterial,
        ),
        (
            "$basetexture dev/base %tooltexture dev/tool_nonfinite_rho",
            (8, 1),
            GREEN,
            DimensionSource::ToolTextureHeader,
            ReflectivitySource::TextureHeader,
        ),
        (
            "$basetexture dev/base %tooltexture 0x10.vtf",
            (16, 16),
            GREEN,
            DimensionSource::ToolTextureHeader,
            ReflectivitySource::TextureHeader,
        ),
    ] {
        let (_directory, fs) = game(fields);
        let mut materials = Materials::new(&fs);
        let index = materials.load(MATERIAL).unwrap();
        let metrics = materials.get(index).texture_metrics().unwrap();
        assert_eq!((metrics.width, metrics.height), dimensions);
        assert_eq!(metrics.reflectivity, rho);
        assert_eq!(metrics.dimension_source, dimension_source);
        assert_eq!(metrics.reflectivity_source, reflectivity_source);
        assert_eq!(
            metrics.reflectivity_source.is_measured_or_authored(),
            reflectivity_source != ReflectivitySource::CompileDefault
        );
        let source = common::room();
        let before = source.clone();
        let bsp = compiled(&source, &fs, LightingMode::Fullbright, 0)
            .bsp
            .unwrap();
        assert_metrics(&bsp, dimensions, rho);
        assert_eq!(source, before);
    }
}

#[test]
fn tool_dimensions_do_not_supply_actual_ldr_or_hdr_bounce_reflectivity() {
    let source = scene();
    let before = source.clone();
    for (fields, reference, counterfactual, rho, channel) in [
        (
            "$basetexture dev/base %tooltexture dev/tool",
            "$basetexture dev/reference_green",
            "$basetexture dev/tool",
            GREEN,
            DVec3::Y,
        ),
        (
            "$basetexture dev/base $bumpmap dev/absent_normal $albedo dev/albedo %tooltexture dev/tool",
            "$basetexture dev/reference_blue $bumpmap dev/absent_normal",
            "$basetexture dev/tool $bumpmap dev/absent_normal",
            BLUE,
            DVec3::Z,
        ),
    ] {
        let (_actual_directory, actual) = game(fields);
        let (_reference_directory, reference) = game(reference);
        let (_wrong_directory, wrong) = game(counterfactual);
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
            let wrong = compiled(&source, &wrong, mode, bounces).bsp.unwrap();
            assert_eq!(actual.to_bytes(), reference.to_bytes());
            assert_metrics(&actual, (8, 1), rho);
            assert_metrics(&wrong, (8, 1), TOOL_RED);
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
                let wrong = floor(&wrong, kind);
                if bounces == 0 {
                    assert_eq!(selected, DVec3::ZERO);
                    assert_eq!(selected, wrong);
                } else {
                    assert!(
                        selected.dot(channel) > 1.0 && selected.x == 0.0,
                        "{selected}"
                    );
                    assert!(wrong.x > 1.0 && wrong.y == 0.0 && wrong.z == 0.0, "{wrong}");
                }
            }
        }
    }
    assert_eq!(source, before);
}

#[test]
fn explicit_metric_providers_can_leave_runtime_images_unread() {
    let source = scene();
    let before = source.clone();
    for (fields, reference) in [
        (
            "$basetexture _rt_metric %tooltexture dev/tool $reflectivity \"[0.125 0.25 0.75]\"",
            "$basetexture dev/reference_authored",
        ),
        (
            "$basetexture _rt_metric $bumpmap dev/absent_normal $albedo _rt_metric %tooltexture dev/tool $reflectivity \"[0.125 0.25 0.75]\"",
            "$basetexture dev/reference_authored $bumpmap dev/absent_normal",
        ),
    ] {
        let (_actual_directory, actual) = game(fields);
        let (_reference_directory, reference) = game(reference);
        for (mode, bounces) in [
            (LightingMode::Fullbright, 0),
            (LightingMode::DirectLdrHdr, 1),
        ] {
            let actual = compiled(&source, &actual, mode, bounces).bsp.unwrap();
            let reference = compiled(&source, &reference, mode, bounces).bsp.unwrap();
            assert_eq!(actual.to_bytes(), reference.to_bytes());
            assert_metrics(&actual, (8, 1), [0.125, 0.25, 0.75]);
        }
    }
    assert_eq!(source, before);
}

#[test]
fn failed_required_providers_never_publish_base_or_128_fallbacks() {
    let (directory, fs) = game("$basetexture dev/base");
    let source = common::room();
    let before = source.clone();
    let previous = compiled(&source, &fs, LightingMode::Fullbright, 0)
        .bsp
        .unwrap()
        .to_bytes();
    let destination = directory.path().join("previous.bsp");
    for (fields, token) in [
        (
            "$basetexture dev/base %tooltexture dev/missing",
            "missing.vtf",
        ),
        (
            "$basetexture dev/base %tooltexture dev/missing $reflectivity \"[0.125 0.25 0.75]\"",
            "missing.vtf",
        ),
        (
            "$basetexture dev/base %tooltexture dev/corrupt $reflectivity \"[0.125 0.25 0.75]\"",
            "corrupt.vtf",
        ),
        ("$basetexture dev/base %tooltexture \"\"", "%tooltexture"),
        ("$basetexture dev/base %tooltexture ../outside", "path"),
        (
            "$basetexture dev/base %tooltexture _rt_provider $reflectivity \"[0.125 0.25 0.75]\"",
            "_rt_provider",
        ),
        (
            "$basetexture dev/missing %tooltexture dev/tool",
            "missing.vtf",
        ),
        (
            "$basetexture dev/nonfinite %tooltexture dev/tool",
            "reflectivity must be finite",
        ),
        (
            "$basetexture dev/base $bumpmap dev/absent_normal $albedo dev/missing %tooltexture dev/tool",
            "missing.vtf",
        ),
        (
            "$basetexture _rt_provider %tooltexture dev/tool",
            "_rt_provider",
        ),
    ] {
        write_material(directory.path(), fields);
        std::fs::write(&destination, &previous).unwrap();
        let output = compiler::compile(&source, &fs);
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
        assert!(
            !output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.starts_with(NO_PREVIEW_DIAGNOSTIC))
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), previous);
        assert_eq!(source, before);
    }
}

#[test]
fn no_preview_is_an_admitted_convention_before_any_unused_tool_read() {
    let source = scene();
    let before = source.clone();
    for (fields, reference, rho, reflectivity_source) in [
        (
            "$basetexture dev/base %notooltexture 0 %tooltexture dev/missing",
            "$basetexture dev/reference_no_preview",
            GREEN,
            ReflectivitySource::TextureHeader,
        ),
        (
            "$basetexture dev/base %notooltexture 1 %tooltexture ../unused-invalid",
            "$basetexture dev/reference_no_preview",
            GREEN,
            ReflectivitySource::TextureHeader,
        ),
        (
            "$basetexture _rt_metric $bumpmap dev/absent_normal $albedo _rt_metric %notooltexture 0 %tooltexture dev/missing $reflectivity \"[0.125 0.25 0.75]\"",
            "$basetexture dev/reference_no_preview_authored $bumpmap dev/absent_normal",
            AUTHORED,
            ReflectivitySource::AuthoredMaterial,
        ),
    ] {
        let (_actual_directory, actual) = game(fields);
        let (_reference_directory, reference) = game(reference);
        let mut materials = Materials::new(&actual);
        let index = materials.load(MATERIAL).unwrap();
        let metrics = materials.get(index).texture_metrics().unwrap();
        assert_eq!(metrics.dimension_source, DimensionSource::NativeNoPreview);
        assert_eq!(metrics.reflectivity_source, reflectivity_source);
        assert_eq!((metrics.width, metrics.height), (128, 128));
        assert_eq!(metrics.reflectivity, rho);
        for (mode, bounces) in [
            (LightingMode::Fullbright, 0),
            (LightingMode::DirectLdrHdr, 1),
        ] {
            let actual = compiled(&source, &actual, mode, bounces);
            assert!(
                actual
                    .report
                    .diagnostics
                    .iter()
                    .any(|item| item.message.starts_with(NO_PREVIEW_DIAGNOSTIC))
            );
            let reference = compiled(&source, &reference, mode, bounces);
            assert!(
                !reference
                    .report
                    .diagnostics
                    .iter()
                    .any(|item| item.message.starts_with(NO_PREVIEW_DIAGNOSTIC))
            );
            let actual = actual.bsp.unwrap();
            assert_eq!(actual.to_bytes(), reference.bsp.unwrap().to_bytes());
            assert_metrics(&actual, (128, 128), rho);
        }
    }
    let (_directory, fs) = game("%notooltexture 0");
    let mut materials = Materials::new(&fs);
    let index = materials.load(MATERIAL).unwrap();
    let metrics = materials.get(index).texture_metrics().unwrap();
    assert_eq!(metrics.dimension_source, DimensionSource::NativeNoPreview);
    assert_eq!(
        metrics.reflectivity_source,
        ReflectivitySource::CompileDefault
    );
    assert!(!metrics.reflectivity_source.is_measured_or_authored());
    assert_eq!(
        (metrics.width, metrics.height, metrics.reflectivity),
        (128, 128, [0.5; 3])
    );
    assert_eq!(source, before);
}

#[test]
fn unsupported_preview_declarations_and_required_rho_do_not_become_no_preview() {
    let (directory, fs) = game("$basetexture dev/base");
    let source = common::room();
    let before = source.clone();
    let previous = compiled(&source, &fs, LightingMode::Fullbright, 0)
        .bsp
        .unwrap()
        .to_bytes();
    let destination = directory.path().join("previous.bsp");
    for (fields, token) in [
        (
            "$basetexture dev/base %notooltexture \"\"",
            "%notooltexture",
        ),
        ("$basetexture dev/base %notooltexture 2", "%notooltexture"),
        ("$basetexture dev/base %tooltexture 0", "%tooltexture"),
        ("$basetexture dev/base %tooltexture 0x10", "%tooltexture"),
        ("$basetexture dev/base %tooltexture NaN", "%tooltexture"),
    ] {
        write_material(directory.path(), fields);
        std::fs::write(&destination, &previous).unwrap();
        let output = compiler::compile(&source, &fs);
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
                .any(|item| item.message.contains(token)
                    && item.message.to_ascii_lowercase().contains("unsupported")),
            "{:?}",
            output.report
        );
        assert!(
            !output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.starts_with(NO_PREVIEW_DIAGNOSTIC))
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), previous);
    }
    write_material(
        directory.path(),
        "$basetexture dev/missing %notooltexture 0 %tooltexture dev/missing_tool",
    );
    let output = compiler::compile(&source, &fs);
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
            .any(|item| item.message.contains("dev/missing.vtf")),
        "{:?}",
        output.report
    );
    assert!(!output.write(&destination).unwrap());
    assert_eq!(std::fs::read(&destination).unwrap(), previous);
    assert_eq!(source, before);
}

#[test]
fn tool_name_bounds_and_nan_payloads_preserve_explicit_provider_selection() {
    let (directory, fs) = game("$basetexture dev/base");
    let stem = format!("deep/{}/{}", "a".repeat(120), "b".repeat(115));
    assert_eq!(stem.len(), 241);
    for name in [
        stem.clone(),
        format!("{stem}b"),
        format!("{stem}{}", "b".repeat(10)),
        "nan(0)".into(),
    ] {
        let path = directory.path().join(format!("materials/{name}.vtf"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, texture(8, 1, TOOL_RED)).unwrap();
    }
    let source = common::room();
    let original = source.clone();
    let previous = compiled(&source, &fs, LightingMode::Fullbright, 0)
        .bsp
        .unwrap()
        .to_bytes();
    let destination = directory.path().join("previous.bsp");
    for selected in [&stem, "nan(0).vtf"] {
        write_material(
            directory.path(),
            &format!("$basetexture dev/base %tooltexture {selected}"),
        );
        let output = compiled(&source, &fs, LightingMode::Fullbright, 0);
        assert_metrics(output.bsp.as_ref().unwrap(), (8, 1), GREEN);
        assert!(
            !output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.starts_with(NO_PREVIEW_DIAGNOSTIC))
        );
    }
    for selected in [
        format!("{stem}b"),
        format!("{stem}{}", "b".repeat(10)),
        "nan(0)".into(),
    ] {
        write_material(
            directory.path(),
            &format!("$basetexture dev/base %tooltexture {selected}"),
        );
        std::fs::write(&destination, &previous).unwrap();
        let output = compiler::compile(&source, &fs);
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
                .any(|item| item.message.contains("%tooltexture")
                    && item.message.contains("unsupported")),
            "{:?}",
            output.report
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), previous);
    }
    write_material(
        directory.path(),
        &format!(
            "$basetexture dev/base %notooltexture 0 %tooltexture {stem}{}",
            "b".repeat(10)
        ),
    );
    let output = compiled(&source, &fs, LightingMode::Fullbright, 0);
    assert_metrics(output.bsp.as_ref().unwrap(), (128, 128), GREEN);
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|item| item.message.starts_with(NO_PREVIEW_DIAGNOSTIC))
    );
    assert_eq!(source, original);
}
