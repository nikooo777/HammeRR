use super::*;

const SOURCE_MATERIAL: &str = "indirect_sources/surface";
const SOURCE_TEXTURE: &str = "materials/indirect_textures/source.vtf";
const UNUSED_TEXTURE: &str = "materials/indirect_textures/unused.vtf";
const REFLECTIVITY: [f32; 3] = [0.125, 0.25, 0.75];

fn texture() -> Vec<u8> {
    let mut bytes = vec![0; 84];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [(4, 7_u32), (8, 2), (12, 80)] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for offset in [16, 18, 24, 63] {
        bytes[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
    }
    for (axis, value) in REFLECTIVITY.into_iter().enumerate() {
        bytes[32 + axis * 4..36 + axis * 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[56] = 1;
    bytes[57..61].copy_from_slice(&(-1_i32).to_le_bytes());
    bytes[80..84].copy_from_slice(&[32, 64, 128, 255]);
    bytes
}

pub(super) fn world_fixture(present: bool) -> (tempfile::TempDir, GameFs, Vmf, Vec<u8>) {
    let (directory, game) = fixtures::game();
    for folder in ["materials/indirect_sources", "materials/indirect_textures"] {
        std::fs::create_dir_all(directory.path().join(folder)).unwrap();
    }
    for (path, contents) in [
        (
            "materials/indirect_sources/base.vmt",
            "LightmappedGeneric { $basetexture indirect_textures/unused }",
        ),
        (
            "materials/indirect_sources/surface.vmt",
            "Patch { include materials/indirect_sources/base.vmt replace { $basetexture indirect_textures/source } }",
        ),
    ] {
        std::fs::write(directory.path().join(path), contents).unwrap();
    }
    let original = texture();
    if present {
        std::fs::write(directory.path().join(SOURCE_TEXTURE), &original).unwrap();
    }
    let mut vmf = fixtures::scene(false);
    for side in vmf
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = SOURCE_MATERIAL.into();
    }
    (directory, game, vmf, original)
}

fn source_metrics(bsp: &Bsp) -> bsp::TexData {
    let names = bsp.texdata_strings().unwrap();
    *bsp.records::<bsp::TexData>()
        .unwrap()
        .iter()
        .find(|data| names[data.name_index as usize].eq_ignore_ascii_case(SOURCE_MATERIAL))
        .unwrap()
}

#[test]
fn one_bounce_publication_retains_original_selected_vtf_after_output_drop() {
    for change_header in [false, true] {
        let (directory, game, vmf, original) = world_fixture(true);
        let output_directory = tempfile::tempdir_in(directory.path().parent().unwrap()).unwrap();
        let destination = output_directory.path().join("previous.bsp");
        std::fs::write(&destination, b"previous complete output").unwrap();
        let direct = compile(&vmf, &game, false);
        let direct_color = file(direct.bsp.as_ref().unwrap(), "sp_0.vhv").meshes[0].colors[1];
        drop(direct);
        let output = crate::compile_with_options(
            &vmf,
            &game,
            Options {
                bounces: 1,
                ..options(false)
            },
        );
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let bsp = output.bsp.as_ref().unwrap();
        assert_eq!(output.stats.bounces, 1);
        assert!(output.stats.bounce_samples > 0);
        assert_eq!(output.stats.prop_vertex_lighting.as_ref().unwrap().props, 1);
        assert_eq!(source_metrics(bsp).reflectivity, REFLECTIVITY);
        let received = file(bsp, "sp_0.vhv").meshes[0].colors[1];
        assert!(
            received.red > direct_color.red,
            "direct={direct_color:?}, one bounce={received:?}"
        );
        assert_eq!((received.green, received.blue), (0, 0));
        let manifest =
            crate::prop_lighting_assets::read_generated_prop_lighting(bsp, &Control::default())
                .unwrap()
                .unwrap();
        let captured = manifest
            .sources
            .iter()
            .find(|source| source.path == SOURCE_TEXTURE)
            .unwrap();
        assert_eq!(captured.bytes, original.len());
        assert_eq!(captured.sha256, crate::asset_sources::hash(&original));
        for path in [
            "materials/indirect_sources/surface.vmt",
            "materials/indirect_sources/base.vmt",
        ] {
            assert!(
                manifest.sources.iter().any(|source| source.path == path),
                "{path}"
            );
        }
        assert!(
            !manifest
                .sources
                .iter()
                .any(|source| source.path == UNUSED_TEXTURE)
        );
        assert!(
            !manifest
                .missing_sources
                .iter()
                .any(|path| path == UNUSED_TEXTURE)
        );
        let control = Control::default();
        let prepared = output
            .prepare_write_controlled(&destination, &control)
            .unwrap()
            .unwrap();
        drop(output);
        let mut replacement = original.clone();
        if change_header {
            replacement[32..36].copy_from_slice(&0.875_f32.to_le_bytes());
        } else {
            replacement[80] ^= 0xff;
        }
        assert_eq!(replacement.len(), original.len());
        assert_ne!(crate::asset_sources::hash(&replacement), captured.sha256);
        let valid = source_formats::vtf::Vtf::parse(replacement.clone()).unwrap();
        assert!(
            valid
                .reflectivity
                .iter()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
        );
        overwrite(&directory.path().join(SOURCE_TEXTURE), 0, &replacement);
        assert!(
            prepared.commit(&control).is_err(),
            "changed_header={change_header}"
        );
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"previous complete output"
        );
    }
}

#[test]
fn one_bounce_rejects_absent_optional_source_metrics_while_direct_keeps_its_scope() {
    for (declaration, width, height) in [
        ("", 64, 64),
        ("%tooltexture indirect_textures/tool", 1, 1),
        ("%notooltexture 0", 128, 128),
    ] {
        let (directory, game, vmf, _) = world_fixture(false);
        std::fs::write(
            directory
                .path()
                .join("materials/indirect_textures/tool.vtf"),
            texture(),
        )
        .unwrap();
        std::fs::write(
            directory
                .path()
                .join("materials/indirect_sources/surface.vmt"),
            format!("LightmappedGeneric {{ {declaration} }}"),
        )
        .unwrap();
        let output_directory = tempfile::tempdir_in(directory.path().parent().unwrap()).unwrap();
        let destination = output_directory.path().join("previous.bsp");
        std::fs::write(&destination, b"previous complete output").unwrap();
        let direct = compile(&vmf, &game, false);
        let bsp = direct.bsp.as_ref().unwrap();
        let metrics = source_metrics(bsp);
        assert_eq!(
            (metrics.width, metrics.height, metrics.reflectivity),
            (width, height, [0.5; 3])
        );
        let manifest =
            crate::prop_lighting_assets::read_generated_prop_lighting(bsp, &Control::default())
                .unwrap()
                .unwrap();
        assert!(
            !manifest
                .sources
                .iter()
                .any(|source| source.path == SOURCE_TEXTURE)
        );
        assert!(
            !manifest
                .missing_sources
                .iter()
                .any(|path| path == SOURCE_TEXTURE)
        );
        assert!(
            !manifest
                .sources
                .iter()
                .any(|source| source.path.ends_with(".vtf"))
        );
        let control = Control::default();
        let previous = bsp.to_bytes();
        assert!(direct.write_controlled(&destination, &control).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), previous);
        drop(direct);
        let failed = crate::compile_with_options(
            &vmf,
            &game,
            Options {
                bounces: 1,
                ..options(false)
            },
        );
        assert!(failed.report.has_errors());
        assert!(failed.bsp.is_none());
        assert!(failed.prop_lighting_assets.is_none());
        assert!(
            failed.report.diagnostics.iter().any(|diagnostic| {
                let message = diagnostic.message.to_ascii_lowercase();
                diagnostic.level == crate::Level::Error
                    && message.contains("measured")
                    && message.contains("reflectivity")
                    && message.contains(SOURCE_MATERIAL)
            }),
            "{:?}",
            failed.report
        );
        assert!(!failed.write_controlled(&destination, &control).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), previous);
    }
}

#[test]
fn authored_reflectivity_drives_measured_one_bounce_prop_lighting() {
    let (directory, game, vmf, original_texture) = world_fixture(true);
    let options = Options {
        bounces: 1,
        ..options(false)
    };
    let baseline = crate::compile_with_options(&vmf, &game, options);
    assert!(!baseline.report.has_errors(), "{:?}", baseline.report);
    assert!(baseline.stats.bounce_samples > 0);
    let baseline_bsp = baseline.bsp.as_ref().unwrap();
    assert_eq!(source_metrics(baseline_bsp).reflectivity, REFLECTIVITY);
    let baseline_color = file(baseline_bsp, "sp_0.vhv").meshes[0].colors[1];
    drop(baseline);
    let patch = "Patch { include materials/indirect_sources/base.vmt replace { $basetexture indirect_textures/source } insert { $reflectivity \"[0 0 0]\" } }";
    std::fs::write(
        directory
            .path()
            .join("materials/indirect_sources/surface.vmt"),
        patch,
    )
    .unwrap();
    let output = crate::compile_with_options(&vmf, &game, options);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.bounces, 1);
    assert!(output.stats.bounce_work > 0);
    assert_eq!(output.stats.prop_vertex_lighting.as_ref().unwrap().props, 1);
    let bsp = output.bsp.as_ref().unwrap();
    assert_eq!(source_metrics(bsp).reflectivity, [0.0; 3]);
    let received = file(bsp, "sp_0.vhv").meshes[0].colors[1];
    assert!(
        received.red < baseline_color.red,
        "baseline={baseline_color:?}, authored={received:?}"
    );
    assert_eq!((received.green, received.blue), (0, 0));
    let manifest =
        crate::prop_lighting_assets::read_generated_prop_lighting(bsp, &Control::default())
            .unwrap()
            .unwrap();
    let texture = manifest
        .sources
        .iter()
        .find(|source| source.path == SOURCE_TEXTURE)
        .unwrap();
    assert_eq!(texture.bytes, original_texture.len());
    assert_eq!(
        texture.sha256,
        crate::asset_sources::hash(&original_texture)
    );
    let source = manifest
        .sources
        .iter()
        .find(|source| source.path == "materials/indirect_sources/surface.vmt")
        .unwrap();
    assert_eq!(source.sha256, crate::asset_sources::hash(patch.as_bytes()));
    assert!(
        manifest
            .sources
            .iter()
            .any(|source| source.path == "materials/indirect_sources/base.vmt")
    );
    assert!(
        !manifest
            .sources
            .iter()
            .any(|source| source.path == UNUSED_TEXTURE)
    );
}

#[test]
fn independent_dimension_providers_and_authored_rho_drive_measured_prop_bounce() {
    for no_preview in [false, true] {
        let (directory, game, vmf, _) = world_fixture(true);
        let direct = compile(&vmf, &game, false);
        let direct_color = file(direct.bsp.as_ref().unwrap(), "sp_0.vhv").meshes[0].colors[1];
        drop(direct);
        let tool_path = "materials/indirect_textures/tool.vtf";
        let mut tool = texture();
        tool[16..18].copy_from_slice(&2_u16.to_le_bytes());
        tool[32..36].copy_from_slice(&f32::NAN.to_le_bytes());
        tool.resize(88, 128);
        std::fs::write(directory.path().join(tool_path), &tool).unwrap();
        let declaration = if no_preview {
            "%notooltexture 1 %tooltexture ../unused_tool"
        } else {
            "%tooltexture indirect_textures/tool"
        };
        let text = format!(
            "LightmappedGeneric {{ $basetexture _rt_Unused $reflectivity \"[0.5 0 0]\" {declaration} }}"
        );
        std::fs::write(
            directory
                .path()
                .join("materials/indirect_sources/surface.vmt"),
            &text,
        )
        .unwrap();
        let output = crate::compile_with_options(
            &vmf,
            &game,
            Options {
                bounces: 1,
                ..options(false)
            },
        );
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.stats.bounces, 1);
        assert!(output.stats.bounce_samples > 0);
        assert_eq!(output.stats.prop_vertex_lighting.as_ref().unwrap().props, 1);
        let bsp = output.bsp.as_ref().unwrap();
        let metrics = source_metrics(bsp);
        assert_eq!(
            (metrics.width, metrics.height),
            if no_preview { (128, 128) } else { (2, 1) }
        );
        assert_eq!(metrics.reflectivity, [0.5, 0.0, 0.0]);
        let received = file(bsp, "sp_0.vhv").meshes[0].colors[1];
        assert!(
            received.red > direct_color.red,
            "direct={direct_color:?}, bounce={received:?}"
        );
        assert_eq!((received.green, received.blue), (0, 0));
        let manifest =
            crate::prop_lighting_assets::read_generated_prop_lighting(bsp, &Control::default())
                .unwrap()
                .unwrap();
        let observed_tool = manifest
            .sources
            .iter()
            .find(|source| source.path == tool_path);
        assert_eq!(observed_tool.is_some(), !no_preview);
        if let Some(observed) = observed_tool {
            assert_eq!(observed.sha256, crate::asset_sources::hash(&tool));
        }
        let material = manifest
            .sources
            .iter()
            .find(|source| source.path == "materials/indirect_sources/surface.vmt")
            .unwrap();
        assert_eq!(material.sha256, crate::asset_sources::hash(text.as_bytes()));
        for unused in [SOURCE_TEXTURE, UNUSED_TEXTURE, "materials/_rt_unused.vtf"] {
            assert!(
                !manifest.sources.iter().any(|source| source.path == unused),
                "{unused}"
            );
            assert!(
                !manifest
                    .missing_sources
                    .iter()
                    .any(|source| source == unused),
                "{unused}"
            );
        }
    }
}

#[test]
fn required_missing_world_texture_refuses_direct_and_bounced_prop_output() {
    let (directory, game, vmf, _) = world_fixture(true);
    let before = vmf.clone();
    let output_directory = tempfile::tempdir_in(directory.path().parent().unwrap()).unwrap();
    let destination = output_directory.path().join("previous.bsp");
    let direct = compile(&vmf, &game, false);
    let previous = direct.bsp.as_ref().unwrap().to_bytes();
    assert!(direct.write(&destination).unwrap());
    drop(direct);
    std::fs::remove_file(directory.path().join(SOURCE_TEXTURE)).unwrap();
    for bounces in [0, 1] {
        let failed = crate::compile_with_options(
            &vmf,
            &game,
            Options {
                bounces,
                ..options(false)
            },
        );
        assert!(failed.report.has_errors());
        assert!(failed.bsp.is_none() && failed.prop_lighting_assets.is_none());
        assert!(
            failed.report.diagnostics.iter().any(|item| {
                item.level == crate::Level::Error
                    && item.message.contains(SOURCE_MATERIAL)
                    && item.message.contains(SOURCE_TEXTURE)
                    && item.message.contains("unavailable")
            }),
            "{:?}",
            failed.report
        );
        assert!(!failed.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), previous);
        assert_eq!(vmf, before);
    }
}
