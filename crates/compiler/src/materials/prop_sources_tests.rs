use super::*;
use crate::asset_sources::{SourceGuardPolicy, capture_asset_source_guard};
use crate::{Control, Report, map::Map};
use source_formats::gamefs::{Mount, MountedPath};

fn fixture() -> (tempfile::TempDir, GameFs) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/validation/620");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::tempdir_in(root).unwrap();
    for folder in ["materials/reflectivity", "materials/texture_dependency"] {
        std::fs::create_dir_all(directory.path().join(folder)).unwrap();
    }
    for (path, text) in [
        (
            "base.vmt",
            "LightmappedGeneric { $basetexture reflectivity/unused }",
        ),
        (
            "patch.vmt",
            "Patch { include materials/reflectivity/base.vmt replace { $basetexture reflectivity/chosen } }",
        ),
        (
            "second.vmt",
            "LightmappedGeneric { $basetexture reflectivity/chosen }",
        ),
        (
            "missing.vmt",
            "LightmappedGeneric { $basetexture texture_dependency/missing }",
        ),
    ] {
        std::fs::write(
            directory
                .path()
                .join(format!("materials/reflectivity/{path}")),
            text,
        )
        .unwrap();
    }
    for (name, reflectivity) in [("chosen", [0.125, 0.25, 0.75]), ("unused", [0.5; 3])] {
        std::fs::write(
            directory
                .path()
                .join(format!("materials/reflectivity/{name}.vtf")),
            texture(reflectivity),
        )
        .unwrap();
    }
    let game = GameFs {
        game_dir: directory.path().to_owned(),
        base_dir: directory.path().to_owned(),
        mounts: vec![MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.path().to_owned()),
        }],
    };
    (directory, game)
}

fn texture(reflectivity: [f32; 3]) -> Vec<u8> {
    let mut bytes = vec![0; 84];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [(4, 7_u32), (8, 2), (12, 80)] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for offset in [16, 18, 24, 63] {
        bytes[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
    }
    for (axis, value) in reflectivity.into_iter().enumerate() {
        bytes[32 + axis * 4..36 + axis * 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[56] = 1;
    bytes[57..61].copy_from_slice(&(-1_i32).to_le_bytes());
    bytes[80..84].copy_from_slice(&[32, 64, 128, 255]);
    bytes
}

fn sized_texture(width: u16, height: u16, reflectivity: [f32; 3]) -> Vec<u8> {
    let mut bytes = texture(reflectivity);
    bytes[16..18].copy_from_slice(&width.to_le_bytes());
    bytes[18..20].copy_from_slice(&height.to_le_bytes());
    bytes.resize(80 + usize::from(width) * usize::from(height) * 4, 128);
    bytes
}

fn guard(
    sources: &[crate::prop_lighting_assets::SourceAsset],
    missing: &[String],
    game: &GameFs,
) -> std::io::Result<crate::asset_sources::AssetSourceGuard> {
    capture_asset_source_guard(
        sources
            .iter()
            .map(|source| (source.path.as_str(), source.sha256.as_str())),
        missing.iter().map(String::as_str),
        game,
        &Control::default(),
        SourceGuardPolicy {
            label: "Prop reflectivity",
            missing_label: "Prop reflectivity source",
            asset_bytes: 64 * 1024 * 1024,
            physical_paths: 4096,
        },
    )
}

fn overwrite(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

#[test]
fn original_selected_texture_bytes_link_metrics_and_reject_header_or_payload_changes() {
    for change_header in [false, true] {
        let (directory, game) = fixture();
        let original = texture([0.125, 0.25, 0.75]);
        let mut materials = Materials::new(&game);
        materials
            .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
            .unwrap();
        let index = materials.load("reflectivity/patch").unwrap();
        assert_eq!(
            materials.get(index).texture_metrics().unwrap(),
            TextureMetrics {
                dimension_source: DimensionSource::DiffuseTextureHeader,
                reflectivity_source: ReflectivitySource::TextureHeader,
                width: 1,
                height: 1,
                reflectivity: [0.125, 0.25, 0.75],
            }
        );
        let mut replacement = original.clone();
        if change_header {
            replacement[32..36].copy_from_slice(&0.875_f32.to_le_bytes());
        } else {
            replacement[83] ^= 0xff;
        }
        overwrite(
            &directory.path().join("materials/reflectivity/chosen.vtf"),
            &replacement,
        );
        let error = materials.load("reflectivity/second").unwrap_err();
        assert!(
            error.contains("chosen.vtf changed during baking"),
            "{error}"
        );
        assert_eq!(
            materials.get(index).texture_metrics().unwrap().reflectivity,
            [0.125, 0.25, 0.75]
        );
        let (sources, missing) = materials.take_prop_lighting_sources().unwrap().into_parts();
        let selected = sources
            .iter()
            .find(|source| source.path == "materials/reflectivity/chosen.vtf")
            .unwrap();
        assert_eq!(selected.bytes, original.len());
        assert_eq!(selected.sha256, crate::asset_sources::hash(&original));
        assert!(
            sources
                .iter()
                .any(|source| source.path == "materials/reflectivity/base.vmt")
        );
        assert!(
            sources
                .iter()
                .any(|source| source.path == "materials/reflectivity/patch.vmt")
        );
        assert!(
            !sources
                .iter()
                .any(|source| source.path.ends_with("unused.vtf"))
        );
        assert!(missing.is_empty());
        assert!(guard(&sources, &missing, &game).is_err());
    }
}

#[test]
fn missing_texture_is_not_measured_and_its_later_appearance_invalidates_the_guard() {
    let (directory, game) = fixture();
    let mut materials = Materials::new(&game);
    materials
        .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
        .unwrap();
    let index = materials.load("reflectivity/missing").unwrap();
    let info = materials.get(index);
    let error = info.texture_metrics().unwrap_err();
    assert!(
        error.contains("materials/reflectivity/missing.vmt"),
        "{error}"
    );
    assert!(error.contains("$basetexture"), "{error}");
    assert!(
        error.contains("materials/texture_dependency/missing.vtf"),
        "{error}"
    );
    assert!(error.contains("unavailable"), "{error}");
    assert!(info.warnings.is_empty());
    let alias = materials
        .add_alias(index, "maps/reflectivity/missing_alias")
        .unwrap();
    assert_eq!(materials.get(alias).texture_metrics().unwrap_err(), error);
    assert_eq!(materials.get(alias).metrics, materials.get(index).metrics);
    let (sources, missing) = materials.take_prop_lighting_sources().unwrap().into_parts();
    assert_eq!(missing, ["materials/texture_dependency/missing.vtf"]);
    let captured = guard(&sources, &missing, &game).unwrap();
    std::fs::write(
        directory
            .path()
            .join("materials/texture_dependency/missing.vtf"),
        texture([0.5; 3]),
    )
    .unwrap();
    assert!(captured.verify_metadata().is_err());
    assert!(guard(&sources, &missing, &game).is_err());
}

#[test]
fn direct_mode_preserves_metrics_and_excludes_texture_bytes_and_missing_lookups() {
    let (directory, game) = fixture();
    let mut ordinary = Materials::new(&game);
    let mut direct = Materials::new(&game);
    direct
        .track_prop_lighting_sources(PropSourceMode::Direct)
        .unwrap();
    for name in ["reflectivity/patch", "reflectivity/missing"] {
        let expected = ordinary.load(name).unwrap();
        let actual = direct.load(name).unwrap();
        assert_eq!(direct.get(actual), ordinary.get(expected));
    }
    assert!(ordinary.take_prop_lighting_sources().is_none());
    let (sources, missing) = direct.take_prop_lighting_sources().unwrap().into_parts();
    assert!(sources.iter().all(|source| source.path.ends_with(".vmt")));
    assert!(missing.is_empty());
    let captured = guard(&sources, &missing, &game).unwrap();
    overwrite(
        &directory.path().join("materials/reflectivity/chosen.vtf"),
        &texture([0.875; 3]),
    );
    captured.verify_metadata().unwrap();
    guard(&sources, &missing, &game).unwrap();
}

#[test]
fn map_capture_mode_is_selected_before_resolution_and_cannot_upgrade_cached_metrics() {
    let (_directory, game) = fixture();
    let mut vmf = crate::test_common::room();
    for side in vmf
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = "reflectivity/patch".into();
    }
    for mode in [PropSourceMode::Direct, PropSourceMode::WithReflectivity] {
        let mut map = Map::load_with_prop_sources_controlled(
            &vmf,
            &game,
            mode,
            &mut Report::default(),
            &Control::default(),
        )
        .unwrap();
        assert!(
            map.materials
                .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
                .is_err()
        );
        let (sources, _) = map
            .materials
            .take_prop_lighting_sources()
            .unwrap()
            .into_parts();
        assert_eq!(
            sources
                .iter()
                .any(|source| source.path.ends_with("chosen.vtf")),
            mode == PropSourceMode::WithReflectivity
        );
    }
    let mut ordinary = Materials::new(&game);
    ordinary.load("reflectivity/patch").unwrap();
    assert!(
        ordinary
            .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
            .is_err()
    );
}

#[test]
fn malformed_nonfinite_and_aggregate_overbudget_textures_fail_without_default_fallback() {
    for bad in [b"malformed".to_vec(), texture([f32::NAN, 0.5, 0.5])] {
        let (directory, game) = fixture();
        std::fs::write(
            directory.path().join("materials/reflectivity/chosen.vtf"),
            bad,
        )
        .unwrap();
        let mut materials = Materials::new(&game);
        materials
            .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
            .unwrap();
        assert!(materials.load("reflectivity/patch").is_err());
        assert_eq!(materials.iter().count(), 0);
    }
    let (directory, game) = fixture();
    let path = directory.path().join("materials/reflectivity/chosen.vtf");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    let mut materials = Materials::new(&game);
    materials
        .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
        .unwrap();
    let error = materials.load("reflectivity/patch").unwrap_err();
    assert!(error.contains("byte read limit"), "{error}");
    assert_eq!(materials.iter().count(), 0);
}

#[test]
fn optional_inputs_and_non_diffuse_surfaces_keep_metadata_without_texture_reads() {
    let (directory, game) = fixture();
    std::fs::write(
        directory.path().join("materials/reflectivity/baseless.vmt"),
        "LightmappedGeneric { $surfaceprop concrete }",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/reflectivity/chosen.vtf"),
        b"unused malformed texture",
    )
    .unwrap();
    let mut materials = Materials::new(&game);
    materials
        .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
        .unwrap();
    for (name, found) in [("absent", false), ("baseless", true)] {
        let index = materials.load(&format!("reflectivity/{name}")).unwrap();
        assert_eq!(materials.get(index).found, found);
        assert_eq!(
            materials.get(index).texture_metrics().unwrap(),
            TextureMetrics {
                dimension_source: DimensionSource::CompileDefault,
                reflectivity_source: ReflectivitySource::CompileDefault,
                width: 64,
                height: 64,
                reflectivity: [0.5; 3],
            }
        );
    }
    for (name, flag, expected_surface, expected_contents) in [
        ("nodraw", "%compilenodraw", surf::NODRAW, contents::SOLID),
        ("origin", "%compileorigin", surf::NODRAW, contents::ORIGIN),
        ("sky", "%compilesky", surf::SKY, contents::SOLID),
        ("sky2d", "%compile2dsky", surf::SKY2D, contents::SOLID),
    ] {
        std::fs::write(
            directory
                .path()
                .join(format!("materials/reflectivity/{name}.vmt")),
            format!("LightmappedGeneric {{ {flag} 1 $basetexture reflectivity/chosen }}"),
        )
        .unwrap();
        let index = materials.load(&format!("reflectivity/{name}")).unwrap();
        let info = materials.get(index);
        assert_ne!(info.surface & expected_surface, 0);
        assert_eq!(info.contents, expected_contents);
        assert_eq!(
            info.texture_metrics().unwrap().reflectivity_source,
            ReflectivitySource::CompileDefault
        );
        assert_eq!(info.texture_metrics().unwrap().reflectivity, [0.5; 3]);
        assert!(info.warnings.is_empty());
    }
    std::fs::write(
        directory
            .path()
            .join("materials/reflectivity/runtime_sky.vmt"),
        "UnlitGeneric { %compile2dsky 1 $basetexture _rt_SmallFB0 }",
    )
    .unwrap();
    let index = materials.load("reflectivity/runtime_sky").unwrap();
    assert_eq!(
        materials
            .get(index)
            .texture_metrics()
            .unwrap()
            .reflectivity_source,
        ReflectivitySource::CompileDefault
    );
    let (sources, missing) = materials.take_prop_lighting_sources().unwrap().into_parts();
    assert!(sources.iter().all(|source| source.path.ends_with(".vmt")));
    assert_eq!(missing, ["materials/reflectivity/absent.vmt"]);
}

#[test]
fn runtime_texture_roots_are_unavailable_without_file_dependencies() {
    let (directory, game) = fixture();
    for text in [
        r#"UnlitGeneric { $basetexture "_rt_SmallFB0" }"#,
        r#"UnlitGeneric { $basetexture " materials\_RT_SmallFB0.VTF " }"#,
        r#"LightmappedGeneric { $basetexture reflectivity/chosen $bumpmap reflectivity/missing_normal $albedo _rt_SmallFB0 }"#,
        r#"LightmappedGeneric { $basetexture reflectivity/chosen $bumpmap reflectivity/missing_normal $albedo _rt_SmallFB0 $reflectivity "[0.5 0 0]" }"#,
    ] {
        std::fs::write(
            directory.path().join("materials/reflectivity/runtime.vmt"),
            text,
        )
        .unwrap();
        let mut materials = Materials::new(&game);
        materials
            .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
            .unwrap();
        let index = materials.load("reflectivity/runtime").unwrap();
        let error = materials.get(index).texture_metrics().unwrap_err();
        assert!(error.contains("runtime.vmt"), "{error}");
        assert!(error.contains("materials/_rt_smallfb0.vtf"), "{error}");
        assert!(error.contains("unsupported"), "{error}");
        let (sources, missing) = materials.take_prop_lighting_sources().unwrap().into_parts();
        assert!(sources.iter().all(|source| source.path.ends_with(".vmt")));
        assert!(missing.is_empty());
        let captured = guard(&sources, &missing, &game).unwrap();
        let unrelated = directory.path().join("materials/_rt_smallfb0.vtf");
        if unrelated.exists() {
            overwrite(&unrelated, &texture([0.875; 3]));
        }
        captured.verify_metadata().unwrap();
        guard(&sources, &missing, &game).unwrap();
        std::fs::write(unrelated, texture([0.125; 3])).unwrap();
    }
    std::fs::write(
        directory.path().join("materials/reflectivity/nested.vmt"),
        "LightmappedGeneric { $basetexture effects/_rt_ordinary }",
    )
    .unwrap();
    let mut materials = Materials::new(&game);
    materials
        .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
        .unwrap();
    let index = materials.load("reflectivity/nested").unwrap();
    let error = materials.get(index).texture_metrics().unwrap_err();
    assert!(error.contains("was not found"), "{error}");
    assert!(!error.contains("runtime-generated"), "{error}");
    let (_, missing) = materials.take_prop_lighting_sources().unwrap().into_parts();
    assert_eq!(missing, ["materials/effects/_rt_ordinary.vtf"]);
}

#[test]
fn independent_dimensions_do_not_rescue_missing_diffuse_rho_or_missing_base_dimensions() {
    for (parameter, value, dimension_source, width, height) in [
        (
            "%tooltexture",
            "reflectivity/chosen",
            DimensionSource::ToolTextureHeader,
            1,
            1,
        ),
        (
            "%notooltexture",
            "0",
            DimensionSource::NativeNoPreview,
            128,
            128,
        ),
        (
            "$reflectivity",
            "[0.9 0.8 0.7]",
            DimensionSource::DiffuseTextureHeader,
            1,
            1,
        ),
    ] {
        let (directory, game) = fixture();
        std::fs::write(
            directory.path().join("materials/reflectivity/override.vmt"),
            format!("LightmappedGeneric {{ $basetexture texture_dependency/missing {parameter} \"{value}\" }}"),
        ).unwrap();
        let mut materials = Materials::new(&game);
        materials
            .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
            .unwrap();
        let index = materials.load("reflectivity/override").unwrap();
        let error = materials.get(index).texture_metrics().unwrap_err();
        assert!(
            error.contains("override.vmt") && error.contains("$basetexture"),
            "{error}"
        );
        assert!(
            error.contains("materials/texture_dependency/missing.vtf"),
            "{error}"
        );
        assert!(error.contains("unavailable"), "{error}");
        assert!(
            error.contains(if parameter == "$reflectivity" {
                "dimensions"
            } else {
                "reflectivity"
            }),
            "{error}"
        );
        let (sources, missing) = materials.take_prop_lighting_sources().unwrap().into_parts();
        assert_eq!(
            sources
                .iter()
                .filter(|source| source.path.ends_with(".vtf"))
                .count(),
            usize::from(parameter == "%tooltexture")
        );
        assert_eq!(missing, ["materials/texture_dependency/missing.vtf"]);
        let captured = guard(&sources, &missing, &game).unwrap();
        std::fs::write(
            directory
                .path()
                .join("materials/texture_dependency/missing.vtf"),
            texture([0.125, 0.25, 0.75]),
        )
        .unwrap();
        assert!(captured.verify_metadata().is_err());
        let mut present = Materials::new(&game);
        let index = present.load("reflectivity/override").unwrap();
        assert_eq!(
            present.get(index).texture_metrics().unwrap(),
            TextureMetrics {
                dimension_source,
                reflectivity_source: if parameter == "$reflectivity" {
                    ReflectivitySource::AuthoredMaterial
                } else {
                    ReflectivitySource::TextureHeader
                },
                width,
                height,
                reflectivity: if parameter == "$reflectivity" {
                    [0.9, 0.8, 0.7]
                } else {
                    [0.125, 0.25, 0.75]
                },
            }
        );
    }
}

#[test]
fn independent_tool_dimensions_retain_diffuse_rho_and_original_provider_guards() {
    for mode in [PropSourceMode::Direct, PropSourceMode::WithReflectivity] {
        for changed in ["chosen", "tool"] {
            let (directory, game) = fixture();
            let tool = sized_texture(4, 2, [f32::NAN, f32::INFINITY, -1.0]);
            std::fs::write(
                directory.path().join("materials/reflectivity/tool.vtf"),
                &tool,
            )
            .unwrap();
            std::fs::write(
                directory.path().join("materials/reflectivity/split.vmt"),
                "LightmappedGeneric { $basetexture reflectivity/chosen %tooltexture reflectivity/tool }",
            ).unwrap();
            let mut materials = Materials::new(&game);
            materials.track_prop_lighting_sources(mode).unwrap();
            let index = materials.load("reflectivity/split").unwrap();
            let expected = TextureMetrics {
                dimension_source: DimensionSource::ToolTextureHeader,
                reflectivity_source: ReflectivitySource::TextureHeader,
                width: 4,
                height: 2,
                reflectivity: [0.125, 0.25, 0.75],
            };
            assert_eq!(materials.get(index).texture_metrics().unwrap(), expected);
            let alias = materials.add_alias(index, "maps/split/alias").unwrap();
            assert_eq!(materials.get(alias).texture_metrics().unwrap(), expected);
            let (sources, missing) = materials.take_prop_lighting_sources().unwrap().into_parts();
            assert!(missing.is_empty());
            let textures = sources
                .iter()
                .filter(|source| source.path.ends_with(".vtf"))
                .collect::<Vec<_>>();
            assert_eq!(
                textures.len(),
                if mode == PropSourceMode::WithReflectivity {
                    2
                } else {
                    0
                }
            );
            if mode == PropSourceMode::WithReflectivity {
                for (path, bytes) in [
                    ("materials/reflectivity/tool.vtf", tool),
                    (
                        "materials/reflectivity/chosen.vtf",
                        texture([0.125, 0.25, 0.75]),
                    ),
                ] {
                    let observed = textures.iter().find(|source| source.path == path).unwrap();
                    assert_eq!(observed.sha256, crate::asset_sources::hash(&bytes));
                }
            }
            let captured = guard(&sources, &missing, &game).unwrap();
            overwrite(
                &directory.path().join("materials/reflectivity/unused.vtf"),
                &texture([0.875; 3]),
            );
            captured.verify_metadata().unwrap();
            guard(&sources, &missing, &game).unwrap();
            let replacement = if changed == "tool" {
                sized_texture(4, 2, [0.25; 3])
            } else {
                texture([0.875; 3])
            };
            overwrite(
                &directory
                    .path()
                    .join(format!("materials/reflectivity/{changed}.vtf")),
                &replacement,
            );
            assert_eq!(
                captured.verify_metadata().is_err(),
                mode == PropSourceMode::WithReflectivity
            );
            assert_eq!(
                guard(&sources, &missing, &game).is_err(),
                mode == PropSourceMode::WithReflectivity
            );
        }
    }
}

#[test]
fn authored_rho_and_tool_dimensions_skip_unused_runtime_and_albedo_headers() {
    for text in [
        "LightmappedGeneric { $basetexture _rt_Unused %tooltexture reflectivity/chosen $reflectivity \"[0.5 0 0]\" }",
        "LightmappedGeneric { $basetexture reflectivity/unused $bumpmap missing/normal $albedo _rt_Unused %tooltexture reflectivity/chosen $reflectivity \"[0.5 0 0]\" }",
        "LightmappedGeneric { %tooltexture reflectivity/chosen $reflectivity \"[0.5 0 0]\" }",
    ] {
        let (directory, game) = fixture();
        let runtime = directory.path().join("materials/_rt_unused.vtf");
        std::fs::write(&runtime, b"unused malformed runtime asset").unwrap();
        std::fs::write(
            directory.path().join("materials/reflectivity/split.vmt"),
            text,
        )
        .unwrap();
        let mut materials = Materials::new(&game);
        materials
            .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
            .unwrap();
        let index = materials.load("reflectivity/split").unwrap();
        assert_eq!(
            materials.get(index).texture_metrics().unwrap(),
            TextureMetrics {
                dimension_source: DimensionSource::ToolTextureHeader,
                reflectivity_source: ReflectivitySource::AuthoredMaterial,
                width: 1,
                height: 1,
                reflectivity: [0.5, 0.0, 0.0],
            }
        );
        let (sources, missing) = materials.take_prop_lighting_sources().unwrap().into_parts();
        assert!(missing.is_empty());
        assert_eq!(
            sources
                .iter()
                .filter(|source| source.path.ends_with(".vtf"))
                .count(),
            1
        );
        assert!(
            sources
                .iter()
                .any(|source| source.path == "materials/reflectivity/chosen.vtf")
        );
        let captured = guard(&sources, &missing, &game).unwrap();
        overwrite(&runtime, b"changed unused runtime payload");
        captured.verify_metadata().unwrap();
        guard(&sources, &missing, &game).unwrap();
    }
}

#[test]
fn split_header_identity_and_required_tool_failures_never_select_another_provider() {
    let (directory, game) = fixture();
    let path = directory.path().join("materials/reflectivity/split.vmt");
    std::fs::write(&path, r#"LightmappedGeneric { $basetexture reflectivity/chosen %tooltexture " MATERIALS\REFLECTIVITY\CHOSEN.VTF " }"#).unwrap();
    let mut materials = Materials::new(&game);
    materials
        .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
        .unwrap();
    let index = materials.load("reflectivity/split").unwrap();
    let metrics = materials.get(index).texture_metrics().unwrap();
    assert_eq!(metrics.dimension_source, DimensionSource::ToolTextureHeader);
    assert_eq!(
        metrics.reflectivity_source,
        ReflectivitySource::TextureHeader
    );
    assert_eq!(
        (metrics.width, metrics.height, metrics.reflectivity),
        (1, 1, [0.125, 0.25, 0.75])
    );
    let (sources, missing) = materials.take_prop_lighting_sources().unwrap().into_parts();
    assert!(missing.is_empty());
    assert_eq!(
        sources
            .iter()
            .filter(|source| source.path.ends_with(".vtf"))
            .count(),
        1
    );
    for invalid in [
        "",
        " ",
        "../outside",
        "/absolute",
        "C:/absolute",
        "0",
        "1.5",
        "NaN",
        "inf",
        "0x1p2",
        "-0x2",
        "nan(0)",
        "+NaN(foo_bar)",
        "-nan()",
    ] {
        std::fs::write(&path, format!("LightmappedGeneric {{ $basetexture reflectivity/chosen %tooltexture \"{invalid}\" $reflectivity \"[0.5 0 0]\" }}")).unwrap();
        let error = Materials::new(&game)
            .load("reflectivity/split")
            .unwrap_err();
        assert!(
            error.contains("split.vmt") && error.contains("%tooltexture"),
            "{error}"
        );
    }
    std::fs::write(
        directory.path().join("materials/0x1p2.vtf"),
        texture([0.875; 3]),
    )
    .unwrap();
    std::fs::write(
        &path,
        "LightmappedGeneric { $basetexture reflectivity/chosen %tooltexture 0x1p2.vtf }",
    )
    .unwrap();
    let mut string_control = Materials::new(&game);
    let index = string_control.load("reflectivity/split").unwrap();
    let metrics = string_control.get(index).texture_metrics().unwrap();
    assert_eq!(metrics.dimension_source, DimensionSource::ToolTextureHeader);
    assert_eq!(metrics.reflectivity, [0.125, 0.25, 0.75]);
    for tool in ["reflectivity/absent", "_rt_Required"] {
        std::fs::write(&path, format!("LightmappedGeneric {{ $basetexture reflectivity/chosen %tooltexture {tool} $reflectivity \"[0.5 0 0]\" }}")).unwrap();
        let mut materials = Materials::new(&game);
        materials
            .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
            .unwrap();
        let index = materials.load("reflectivity/split").unwrap();
        let error = materials.get(index).texture_metrics().unwrap_err();
        assert!(
            error.contains("%tooltexture") && error.contains("dimensions"),
            "{error}"
        );
        assert!(error.contains(&vmt::texture_path(tool)), "{error}");
        let (sources, missing) = materials.take_prop_lighting_sources().unwrap().into_parts();
        assert!(sources.iter().all(|source| source.path.ends_with(".vmt")));
        assert_eq!(
            missing,
            if tool.starts_with("_rt_") {
                Vec::<String>::new()
            } else {
                vec![vmt::texture_path(tool)]
            }
        );
    }
    std::fs::write(
        directory.path().join("materials/reflectivity/broken.vtf"),
        b"bad required tool header",
    )
    .unwrap();
    std::fs::write(&path, "LightmappedGeneric { $basetexture reflectivity/chosen %tooltexture reflectivity/broken $reflectivity \"[0.5 0 0]\" }").unwrap();
    let error = Materials::new(&game)
        .load("reflectivity/split")
        .unwrap_err();
    assert!(error.contains("broken.vtf"), "{error}");
}

#[test]
fn native_no_preview_is_selected_before_reads_and_keeps_independent_rho() {
    for value in ["0", "1", " 0 "] {
        for (parameters, reflectivity_source, rho, reads_header) in [
            (
                "$basetexture reflectivity/chosen",
                ReflectivitySource::TextureHeader,
                [0.125, 0.25, 0.75],
                true,
            ),
            (
                "$basetexture _rt_Unused $reflectivity \"[0.5 0 0]\"",
                ReflectivitySource::AuthoredMaterial,
                [0.5, 0.0, 0.0],
                false,
            ),
            (
                "$reflectivity \"[0.5 0 0]\"",
                ReflectivitySource::AuthoredMaterial,
                [0.5, 0.0, 0.0],
                false,
            ),
            ("", ReflectivitySource::CompileDefault, [0.5; 3], false),
        ] {
            let (directory, game) = fixture();
            let text = format!(
                "LightmappedGeneric {{ {parameters} %notooltexture \"{value}\" %tooltexture ../ignored_tool }}"
            );
            std::fs::write(
                directory
                    .path()
                    .join("materials/reflectivity/no_preview.vmt"),
                text,
            )
            .unwrap();
            let mut materials = Materials::new(&game);
            materials
                .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
                .unwrap();
            let index = materials.load("reflectivity/no_preview").unwrap();
            assert_eq!(
                materials.get(index).texture_metrics().unwrap(),
                TextureMetrics {
                    dimension_source: DimensionSource::NativeNoPreview,
                    reflectivity_source,
                    width: 128,
                    height: 128,
                    reflectivity: rho,
                }
            );
            let (sources, missing) = materials.take_prop_lighting_sources().unwrap().into_parts();
            assert!(missing.is_empty());
            assert_eq!(
                sources
                    .iter()
                    .filter(|source| source.path.ends_with(".vtf"))
                    .count(),
                usize::from(reads_header)
            );
            let captured = guard(&sources, &missing, &game).unwrap();
            overwrite(
                &directory.path().join("materials/reflectivity/unused.vtf"),
                &texture([0.875; 3]),
            );
            captured.verify_metadata().unwrap();
            guard(&sources, &missing, &game).unwrap();
        }
    }
    let (directory, game) = fixture();
    let path = directory
        .path()
        .join("materials/reflectivity/no_preview.vmt");
    for value in ["", " ", "2", "true", "0.0", "01"] {
        std::fs::write(&path, format!("LightmappedGeneric {{ $basetexture reflectivity/chosen %notooltexture \"{value}\" %tooltexture ../ignored_tool }}")).unwrap();
        let error = Materials::new(&game)
            .load("reflectivity/no_preview")
            .unwrap_err();
        assert!(
            error.contains("%notooltexture") && error.contains("unsupported"),
            "{error}"
        );
    }
    std::fs::write(
        directory.path().join("materials/reflectivity/chosen.vtf"),
        texture([f32::NAN, 0.25, 0.75]),
    )
    .unwrap();
    std::fs::write(
        &path,
        "LightmappedGeneric { $basetexture reflectivity/chosen %notooltexture 1 }",
    )
    .unwrap();
    assert!(
        Materials::new(&game)
            .load("reflectivity/no_preview")
            .unwrap_err()
            .contains("reflectivity must be finite")
    );
    std::fs::write(&path, "LightmappedGeneric { $basetexture reflectivity/chosen %notooltexture 1 $reflectivity \"[0.5 0 0]\" }").unwrap();
    let mut materials = Materials::new(&game);
    let index = materials.load("reflectivity/no_preview").unwrap();
    assert_eq!(
        materials.get(index).texture_metrics().unwrap().reflectivity,
        [0.5, 0.0, 0.0]
    );
}

#[test]
fn tool_only_dimensions_keep_optional_neutral_rho_without_promoting_it_to_measured() {
    let (directory, game) = fixture();
    std::fs::write(
        directory
            .path()
            .join("materials/reflectivity/tool_only.vmt"),
        "LightmappedGeneric { %tooltexture reflectivity/chosen }",
    )
    .unwrap();
    let mut materials = Materials::new(&game);
    let index = materials.load("reflectivity/tool_only").unwrap();
    let metrics = materials.get(index).texture_metrics().unwrap();
    assert_eq!(
        metrics,
        TextureMetrics {
            dimension_source: DimensionSource::ToolTextureHeader,
            reflectivity_source: ReflectivitySource::CompileDefault,
            width: 1,
            height: 1,
            reflectivity: [0.5; 3],
        }
    );
    assert!(!metrics.reflectivity_source.is_measured_or_authored());
}

#[test]
fn authored_reflectivity_uses_resolved_patch_values_and_original_sources() {
    for mode in [PropSourceMode::Direct, PropSourceMode::WithReflectivity] {
        let (directory, game) = fixture();
        std::fs::write(
            directory.path().join("materials/reflectivity/base.vmt"),
            "LightmappedGeneric { $basetexture reflectivity/unused $reflectivity \"[0 0.25 0]\" }",
        )
        .unwrap();
        let patch = "Patch { include materials/reflectivity/base.vmt replace { $basetexture reflectivity/chosen $reflectivity \"[5e-1 0 0]\" } }";
        let path = directory.path().join("materials/reflectivity/patch.vmt");
        std::fs::write(&path, patch).unwrap();
        let mut materials = Materials::new(&game);
        materials.track_prop_lighting_sources(mode).unwrap();
        let index = materials.load("reflectivity/patch").unwrap();
        let expected = TextureMetrics {
            dimension_source: DimensionSource::DiffuseTextureHeader,
            reflectivity_source: ReflectivitySource::AuthoredMaterial,
            width: 1,
            height: 1,
            reflectivity: [0.5, 0.0, 0.0],
        };
        assert_eq!(materials.get(index).texture_metrics().unwrap(), expected);
        assert!(expected.reflectivity_source.is_measured_or_authored());
        assert!(!ReflectivitySource::CompileDefault.is_measured_or_authored());
        let alias = materials.add_alias(index, "maps/authored/alias").unwrap();
        assert_eq!(materials.get(alias).texture_metrics().unwrap(), expected);
        let (sources, missing) = materials.take_prop_lighting_sources().unwrap().into_parts();
        let captured_patch = sources
            .iter()
            .find(|source| source.path == "materials/reflectivity/patch.vmt")
            .unwrap();
        assert_eq!(
            captured_patch.sha256,
            crate::asset_sources::hash(patch.as_bytes())
        );
        assert!(
            sources
                .iter()
                .any(|source| source.path == "materials/reflectivity/base.vmt")
        );
        assert_eq!(
            sources
                .iter()
                .any(|source| source.path == "materials/reflectivity/chosen.vtf"),
            mode == PropSourceMode::WithReflectivity
        );
        assert!(
            !sources
                .iter()
                .any(|source| source.path.ends_with("unused.vtf"))
        );
        assert!(missing.is_empty());
        let captured = guard(&sources, &missing, &game).unwrap();
        overwrite(&path, patch.replace("[5e-1 0 0]", "[0 5e-1 0]").as_bytes());
        assert!(captured.verify_metadata().is_err());
        assert!(guard(&sources, &missing, &game).is_err());
    }
}

#[test]
fn authored_reflectivity_replaces_unused_header_values_without_rescuing_dimensions() {
    for (name, text) in [
        (
            "raw",
            "LightmappedGeneric { $basetexture reflectivity/chosen $reflectivity \"[0.5 0 0]\" }",
        ),
        (
            "albedo",
            "LightmappedGeneric { $basetexture reflectivity/unused $bumpmap reflectivity/missing_normal $albedo reflectivity/chosen $reflectivity \"[0.5 0 0]\" }",
        ),
    ] {
        let (directory, game) = fixture();
        std::fs::write(
            directory
                .path()
                .join(format!("materials/reflectivity/{name}.vmt")),
            text,
        )
        .unwrap();
        let texture_path = directory.path().join("materials/reflectivity/chosen.vtf");
        std::fs::write(&texture_path, texture([f32::NAN, f32::INFINITY, -1.0])).unwrap();
        let mut materials = Materials::new(&game);
        let index = materials.load(&format!("reflectivity/{name}")).unwrap();
        let metrics = materials.get(index).texture_metrics().unwrap();
        assert_eq!(
            metrics.reflectivity_source,
            ReflectivitySource::AuthoredMaterial
        );
        assert_eq!(
            (metrics.width, metrics.height, metrics.reflectivity),
            (1, 1, [0.5, 0.0, 0.0])
        );
        assert!(Materials::new(&game).load("reflectivity/second").is_err());
        std::fs::write(&texture_path, b"malformed required dimensions").unwrap();
        assert!(
            Materials::new(&game)
                .load(&format!("reflectivity/{name}"))
                .is_err()
        );
        std::fs::remove_file(&texture_path).unwrap();
        let mut missing = Materials::new(&game);
        if name == "albedo" {
            assert!(
                missing
                    .load("reflectivity/albedo")
                    .unwrap_err()
                    .contains("chosen.vtf")
            );
        } else {
            let index = missing.load("reflectivity/raw").unwrap();
            assert!(
                missing
                    .get(index)
                    .texture_metrics()
                    .unwrap_err()
                    .contains("dimensions")
            );
        }
    }
}

#[test]
fn demanded_authored_reflectivity_has_strict_bounded_syntax_and_metric_scope() {
    let (directory, game) = fixture();
    let path = directory.path().join("materials/reflectivity/authored.vmt");
    for value in [
        "0.5",
        "0.5 0 0",
        "{128 0 0}",
        "[]",
        "[0 0]",
        "[0 0 0 0]",
        "[NaN 0 0]",
        "[inf 0 0]",
        "[-0.1 0 0]",
        "[1.0000000001 0 0]",
        "[(0) 0 0]",
        "[0 0 0] trailing",
        "[0\u{a0}0 0]",
    ] {
        std::fs::write(
            &path,
            format!("LightmappedGeneric {{ $basetexture reflectivity/chosen $reflectivity \"{value}\" }}"),
        )
        .unwrap();
        let error = Materials::new(&game)
            .load("reflectivity/authored")
            .unwrap_err();
        assert!(
            error.contains("authored.vmt") && error.contains("$reflectivity"),
            "{error}"
        );
    }
    let fitting = " ".repeat(MAX_REFLECTIVITY_VALUE_BYTES - 7) + "[0 0 0]";
    for (value, accepted) in [(fitting.clone(), true), (format!(" {fitting}"), false)] {
        std::fs::write(
            &path,
            format!("LightmappedGeneric {{ $basetexture reflectivity/chosen $reflectivity \"{value}\" }}"),
        )
        .unwrap();
        assert_eq!(
            Materials::new(&game).load("reflectivity/authored").is_ok(),
            accepted
        );
    }
    std::fs::write(&path, "LightmappedGeneric { $reflectivity \"[0.5 0 0]\" }").unwrap();
    let mut materials = Materials::new(&game);
    let index = materials.load("reflectivity/authored").unwrap();
    let error = materials.get(index).texture_metrics().unwrap_err();
    assert!(
        error.contains("$reflectivity") && error.contains("dimensions"),
        "{error}"
    );
    for (parameter, value) in [("%tooltexture", "0"), ("%notooltexture", "2")] {
        std::fs::write(
            &path,
            format!("LightmappedGeneric {{ $basetexture reflectivity/chosen $reflectivity \"[0.5 0 0]\" {parameter} {value} }}"),
        )
        .unwrap();
        let error = Materials::new(&game)
            .load("reflectivity/authored")
            .unwrap_err();
        assert!(
            error.contains(parameter) && error.contains("unsupported"),
            "{error}"
        );
    }
    std::fs::write(
        &path,
        "LightmappedGeneric { %compilesky 1 $reflectivity bad }",
    )
    .unwrap();
    let mut materials = Materials::new(&game);
    let index = materials.load("reflectivity/authored").unwrap();
    assert_eq!(
        materials
            .get(index)
            .texture_metrics()
            .unwrap()
            .reflectivity_source,
        ReflectivitySource::CompileDefault
    );
    std::fs::write(
        &path,
        "LightmappedGeneric { $basetexture reflectivity/chosen $reflectivity bad }",
    )
    .unwrap();
    let mut geometry = Materials::with_requirements(&game, Requirements::GeometrySemantics);
    let index = geometry.load("reflectivity/authored").unwrap();
    assert_eq!(geometry.get(index).metrics, MetricState::Deferred);
}

fn albedo_fixture() -> (tempfile::TempDir, GameFs) {
    let (directory, game) = fixture();
    for name in ["albedo", "albedo_second"] {
        std::fs::write(
            directory
                .path()
                .join(format!("materials/reflectivity/{name}.vmt")),
            "LightmappedGeneric { $basetexture reflectivity/unused $bumpmap reflectivity/missing_normal $albedo reflectivity/chosen }",
        )
        .unwrap();
    }
    (directory, game)
}

#[test]
fn active_albedo_tracks_original_selected_bytes_and_ignores_unused_base_and_normal() {
    for change_header in [false, true] {
        let (directory, game) = albedo_fixture();
        let mut materials = Materials::new(&game);
        materials
            .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
            .unwrap();
        let index = materials.load("reflectivity/albedo").unwrap();
        assert_eq!(
            materials.get(index).texture_metrics().unwrap().reflectivity,
            [0.125, 0.25, 0.75]
        );
        materials.load("reflectivity/albedo_second").unwrap();
        let (sources, missing) = materials.take_prop_lighting_sources().unwrap().into_parts();
        let selected = sources
            .iter()
            .find(|source| source.path == "materials/reflectivity/chosen.vtf")
            .unwrap();
        let original = texture([0.125, 0.25, 0.75]);
        assert_eq!(selected.bytes, original.len());
        assert_eq!(selected.sha256, crate::asset_sources::hash(&original));
        assert_eq!(
            sources
                .iter()
                .filter(|source| source.path.ends_with(".vtf"))
                .count(),
            1
        );
        assert!(missing.is_empty());
        let captured = guard(&sources, &missing, &game).unwrap();
        overwrite(
            &directory.path().join("materials/reflectivity/unused.vtf"),
            &texture([0.875; 3]),
        );
        captured.verify_metadata().unwrap();
        guard(&sources, &missing, &game).unwrap();

        let mut replacement = original;
        if change_header {
            replacement[32..36].copy_from_slice(&0.875_f32.to_le_bytes());
        } else {
            replacement[83] ^= 0xff;
        }
        overwrite(
            &directory.path().join("materials/reflectivity/chosen.vtf"),
            &replacement,
        );
        assert!(captured.verify_metadata().is_err());
        assert!(guard(&sources, &missing, &game).is_err());
    }
}

#[test]
fn active_albedo_change_during_baking_is_rejected_without_replacing_metrics() {
    let (directory, game) = albedo_fixture();
    let mut materials = Materials::new(&game);
    materials
        .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
        .unwrap();
    let index = materials.load("reflectivity/albedo").unwrap();
    let mut changed = texture([0.125, 0.25, 0.75]);
    changed[83] ^= 0xff;
    overwrite(
        &directory.path().join("materials/reflectivity/chosen.vtf"),
        &changed,
    );
    let error = materials.load("reflectivity/albedo_second").unwrap_err();
    assert!(
        error.contains("chosen.vtf changed during baking"),
        "{error}"
    );
    assert_eq!(materials.iter().count(), 1);
    assert_eq!(
        materials.get(index).texture_metrics().unwrap().reflectivity,
        [0.125, 0.25, 0.75]
    );
}

#[test]
fn missing_active_albedo_fails_and_retains_its_negative_source_observation() {
    let (directory, game) = albedo_fixture();
    std::fs::write(
        directory.path().join("materials/reflectivity/albedo.vmt"),
        "LightmappedGeneric { $basetexture reflectivity/chosen $bumpmap reflectivity/missing_normal $albedo reflectivity/chosen }",
    )
    .unwrap();
    let path = directory.path().join("materials/reflectivity/chosen.vtf");
    std::fs::remove_file(&path).unwrap();
    let mut materials = Materials::new(&game);
    materials
        .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
        .unwrap();
    let error = materials.load("reflectivity/albedo").unwrap_err();
    assert!(
        error.contains("$albedo") && error.contains("chosen.vtf was not found"),
        "{error}"
    );
    assert_eq!(materials.iter().count(), 0);
    let (sources, missing) = materials.take_prop_lighting_sources().unwrap().into_parts();
    assert_eq!(missing, ["materials/reflectivity/chosen.vtf"]);
    assert!(sources.iter().all(|source| source.path.ends_with(".vmt")));
    let captured = guard(&sources, &missing, &game).unwrap();
    let mut direct = Materials::new(&game);
    direct
        .track_prop_lighting_sources(PropSourceMode::Direct)
        .unwrap();
    assert!(direct.load("reflectivity/albedo").is_err());
    let (direct_sources, direct_missing) =
        direct.take_prop_lighting_sources().unwrap().into_parts();
    assert!(
        direct_sources
            .iter()
            .all(|source| source.path.ends_with(".vmt"))
    );
    assert!(direct_missing.is_empty());
    std::fs::write(&path, texture([0.125, 0.25, 0.75])).unwrap();
    assert!(captured.verify_metadata().is_err());
    assert!(guard(&sources, &missing, &game).is_err());
}

#[test]
fn active_albedo_direct_mode_measures_without_tracking_texture_bytes() {
    let (directory, game) = albedo_fixture();
    let mut ordinary = Materials::new(&game);
    let expected = ordinary.load("reflectivity/albedo").unwrap();
    let mut direct = Materials::new(&game);
    direct
        .track_prop_lighting_sources(PropSourceMode::Direct)
        .unwrap();
    let actual = direct.load("reflectivity/albedo").unwrap();
    assert_eq!(direct.get(actual), ordinary.get(expected));
    let (sources, missing) = direct.take_prop_lighting_sources().unwrap().into_parts();
    assert!(sources.iter().all(|source| source.path.ends_with(".vmt")));
    assert!(missing.is_empty());
    let captured = guard(&sources, &missing, &game).unwrap();
    overwrite(
        &directory.path().join("materials/reflectivity/chosen.vtf"),
        &texture([0.875; 3]),
    );
    captured.verify_metadata().unwrap();
    guard(&sources, &missing, &game).unwrap();
}

#[test]
fn active_albedo_failures_never_publish_metrics_but_geometry_remains_deferred() {
    for bad in [
        None,
        Some(b"malformed".to_vec()),
        Some(texture([f32::INFINITY, 0.5, 0.5])),
    ] {
        let (directory, game) = albedo_fixture();
        let path = directory.path().join("materials/reflectivity/chosen.vtf");
        match bad {
            Some(bytes) => std::fs::write(&path, bytes).unwrap(),
            None => std::fs::remove_file(&path).unwrap(),
        }
        let mut materials = Materials::new(&game);
        assert!(materials.load("reflectivity/albedo").is_err());
        assert_eq!(materials.iter().count(), 0);
        let mut geometry = Materials::with_requirements(&game, Requirements::GeometrySemantics);
        let index = geometry.load("reflectivity/albedo").unwrap();
        assert_eq!(geometry.get(index).metrics, MetricState::Deferred);
        assert!(geometry.get(index).texture_metrics().is_err());
    }
}

#[test]
fn non_diffuse_flags_do_not_relax_active_albedo_admission() {
    for flag in ["%compilenodraw", "%compilesky", "%compile2dsky"] {
        let (directory, game) = albedo_fixture();
        std::fs::write(
            directory.path().join("materials/reflectivity/albedo.vmt"),
            format!("LightmappedGeneric {{ {flag} 1 $basetexture reflectivity/unused $bumpmap reflectivity/missing_normal $albedo reflectivity/chosen }}"),
        )
        .unwrap();
        std::fs::remove_file(directory.path().join("materials/reflectivity/chosen.vtf")).unwrap();
        let mut materials = Materials::new(&game);
        let error = materials.load("reflectivity/albedo").unwrap_err();
        assert!(
            error.contains("$albedo") && error.contains("chosen.vtf"),
            "{error}"
        );
        assert_eq!(materials.iter().count(), 0);
    }
}
