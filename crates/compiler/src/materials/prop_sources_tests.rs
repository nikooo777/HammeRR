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
                source: MetricSource::TextureHeader,
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
    assert_eq!(
        info.texture_metrics().unwrap().source,
        MetricSource::CompileDefault
    );
    assert!(!info.warnings.is_empty());
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
