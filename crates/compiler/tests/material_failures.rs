mod common;

use compiler::materials::Materials;
use compiler::{LightingMode, Options, SkyQuality};
use glam::DVec3;
use map_core::{Document, edit};
use source_formats::bsp::TexData;
use source_formats::gamefs::{Mount, MountedPath};
use source_formats::vmf::Vmf;

fn directory() -> tempfile::TempDir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/validation/684");
    std::fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

fn texture(reflectivity: [f32; 3]) -> Vec<u8> {
    let mut bytes = vec![0u8; 84];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [(4, 7_u32), (8, 2), (12, 80)] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for offset in [16, 18, 24, 63] {
        bytes[offset..offset + 2].copy_from_slice(&1u16.to_le_bytes());
    }
    for (axis, value) in reflectivity.into_iter().enumerate() {
        bytes[32 + axis * 4..36 + axis * 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[56] = 1;
    bytes[57..61].copy_from_slice(&(-1i32).to_le_bytes());
    bytes[80..84].copy_from_slice(&[32, 64, 192, 255]);
    bytes
}

fn scene() -> Vmf {
    let mut document = Document::new(common::room());
    document
        .edit("required texture metrics", |tx| {
            edit::create_entity(
                tx,
                "light",
                Some(DVec3::new(0.0, 0.0, 200.0)),
                &[
                    ("_light".into(), "255 255 255 64".into()),
                    ("_lightHDR".into(), "255 255 255 96".into()),
                    ("_constant_attn".into(), "1".into()),
                    ("_quadratic_attn".into(), "0".into()),
                ],
                Vec::new(),
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
        side.material = "required".into();
        side.lightmap_scale = 64;
    }
    source
}

fn options(lighting: LightingMode, bounces: u8) -> Options {
    Options {
        lighting,
        bounces,
        sky_quality: SkyQuality::Fast,
        ..Default::default()
    }
}

#[test]
fn malformed_assets_stop_compilation_and_preserve_previous_output() {
    let directory = directory();
    let materials = directory.path().join("materials");
    std::fs::create_dir(&materials).unwrap();
    let mut fs = common::empty_game();
    fs.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    let mut vmf = common::room();
    vmf.world.solids[0].sides[0].material = "broken".into();
    let source = vmf.clone();
    let destination = directory.path().join("existing.bsp");
    std::fs::write(&destination, b"previous successful build").unwrap();
    std::fs::write(materials.join("broken.vtf"), b"not a VTF").unwrap();
    for (text, message) in [
        ("LightmappedGeneric { $basetexture }", "missing a value"),
        ("Patch { include materials/absent.vmt }", "doesn't exist"),
        ("Patch { include materials/broken.vmt }", "deeply"),
        ("LightmappedGeneric { $basetexture broken }", "broken.vtf"),
        ("LightmappedGeneric { $basetexture ../outside }", "path"),
    ] {
        std::fs::write(materials.join("broken.vmt"), text).unwrap();
        let output = compiler::compile(&vmf, &fs);
        assert!(output.report.has_errors(), "{:?}", output.report);
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains(message)),
            "{:?}",
            output.report
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"previous successful build"
        );
        assert_eq!(vmf, source);
        assert!(compiler::map::Map::load(&vmf, &fs, &mut compiler::Report::default()).is_err());
    }
    let nonfinite = texture([f32::NAN, 0.25, 0.75]);
    source_formats::vtf::Vtf::parse(nonfinite.clone()).unwrap();
    std::fs::write(materials.join("broken.vtf"), nonfinite).unwrap();
    std::fs::write(
        materials.join("broken.vmt"),
        "LightmappedGeneric { $basetexture broken }",
    )
    .unwrap();
    let output = compiler::compile(&vmf, &fs);
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|item| item.message.contains("reflectivity must be finite"))
    );
    assert!(!output.write(&destination).unwrap());
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"previous successful build"
    );
    assert_eq!(vmf, source);
    #[cfg(unix)]
    {
        std::fs::remove_file(materials.join("broken.vtf")).unwrap();
        std::os::unix::fs::symlink("broken.vtf", materials.join("broken.vtf")).unwrap();
        assert!(Materials::new(&fs).load("broken").is_err());
        let output = compiler::compile(&vmf, &fs);
        assert!(output.report.has_errors() && output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains("broken.vtf"))
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"previous successful build"
        );
        assert_eq!(vmf, source);
    }
}

#[test]
fn unavailable_required_metrics_refuse_geometry_and_bounced_publication() {
    let directory = directory();
    let materials = directory.path().join("materials");
    std::fs::create_dir(&materials).unwrap();
    std::fs::write(
        materials.join("_rt_required.vtf"),
        texture([0.125, 0.25, 0.75]),
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    let source = scene();
    let before = source.clone();
    let destination = directory.path().join("previous.bsp");
    for (base, extra, token) in [
        ("absent", "", "absent.vtf"),
        ("_rt_absent", "", "_rt_absent"),
        ("_rt_required", "", "_rt_required"),
        ("absent", "%tooltexture tool", "%tooltexture"),
        ("absent", "%notooltexture 0", "%notooltexture"),
        ("absent", "$reflectivity \"[0.1 0.2 0.3]\"", "$reflectivity"),
    ] {
        std::fs::write(
            materials.join("required.vmt"),
            format!("LightmappedGeneric {{ $basetexture {base} {extra} }}"),
        )
        .unwrap();
        let mut loaded = Materials::new(&game);
        let index = loaded.load("required").unwrap();
        let error = loaded.get(index).texture_metrics().unwrap_err();
        assert!(
            error.contains(token) && error.contains("$basetexture"),
            "{error}"
        );
        assert!(
            error.contains("unavailable") || error.contains("unsupported"),
            "{error}"
        );
        for (lighting, bounces) in [
            (LightingMode::Fullbright, 0),
            (LightingMode::DirectLdrHdr, 1),
        ] {
            std::fs::write(&destination, b"previous successful build").unwrap();
            let output = compiler::compile_with_options(&source, &game, options(lighting, bounces));
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
            assert_eq!(
                std::fs::read(&destination).unwrap(),
                b"previous successful build"
            );
            assert_eq!(source, before);
        }
    }
}

#[test]
fn valid_headers_and_absent_optional_inputs_keep_actual_texdata() {
    let directory = directory();
    let materials = directory.path().join("materials");
    std::fs::create_dir(&materials).unwrap();
    std::fs::write(materials.join("valid.vtf"), texture([0.125, 0.25, 0.75])).unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    let source = scene();
    let before = source.clone();
    for (lighting, bounces) in [
        (LightingMode::Fullbright, 0),
        (LightingMode::DirectLdrHdr, 1),
    ] {
        let mut absent = None;
        for (material, size, reflectivity) in [
            (None, 64, [0.5; 3]),
            (Some("LightmappedGeneric { }"), 64, [0.5; 3]),
            (
                Some("LightmappedGeneric { $basetexture valid }"),
                1,
                [0.125, 0.25, 0.75],
            ),
        ] {
            let path = materials.join("required.vmt");
            if let Some(text) = material {
                std::fs::write(&path, text).unwrap();
            } else if path.exists() {
                std::fs::remove_file(&path).unwrap();
            }
            let output = compiler::compile_with_options(&source, &game, options(lighting, bounces));
            assert!(!output.report.has_errors(), "{:?}", output.report);
            if bounces != 0 {
                assert!(output.stats.bounce_work > 0 && output.stats.bounce_samples > 0);
            }
            let bsp = output.bsp.unwrap();
            let records = bsp.records::<TexData>().unwrap();
            assert!(!records.is_empty());
            for row in records {
                assert_eq!(
                    (row.width, row.height, row.view_width, row.view_height),
                    (size, size, size, size)
                );
                assert_eq!(row.reflectivity, reflectivity);
            }
            if material.is_none() {
                absent = Some(bsp.to_bytes());
            } else if size == 64 {
                assert_eq!(Some(bsp.to_bytes()), absent);
            }
            assert_eq!(source, before);
        }
    }
}

#[test]
fn raw_nodraw_and_sky_geometry_do_not_read_unused_texture_files() {
    let directory = directory();
    let materials = directory.path().join("materials");
    std::fs::create_dir(&materials).unwrap();
    std::fs::write(materials.join("unused.vtf"), b"not a VTF").unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    for flags in [
        "%compilenodraw 1",
        "%compilesky 1",
        "%compilesky 1 %compile2dsky 1",
    ] {
        std::fs::write(
            materials.join("semantic.vmt"),
            format!("UnlitGeneric {{ $basetexture unused {flags} }}"),
        )
        .unwrap();
        let mut source = common::room();
        for side in &mut source.world.solids[1].sides {
            side.material = "semantic".into();
        }
        let before = source.clone();
        let output = compiler::compile(&source, &game);
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let records = output.bsp.unwrap().records::<TexData>().unwrap();
        assert!(!records.is_empty());
        for row in records {
            assert_eq!((row.width, row.height), (64, 64));
            assert_eq!(row.reflectivity, [0.5; 3]);
        }
        assert_eq!(source, before);
    }
}
