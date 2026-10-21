use source_formats::gamefs::{Mount, MountedPath};

use crate::materials::{Materials, Requirements};

use super::*;

const MATERIAL: &[u8] = br#"LightmappedGeneric { $reflectivity "[0.1 0.2 0.3]" }"#;

fn fixture() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    std::fs::write(directory.path().join("materials/source.vmt"), MATERIAL).unwrap();
    let mut game = crate::test_common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn owner(limits: Limits) -> BuildMaterialSources {
    BuildMaterialSources {
        state: Arc::new(Mutex::new(State {
            limits,
            ..State::default()
        })),
    }
}

#[test]
fn later_material_collections_refuse_drift_from_the_first_consumed_payload() {
    let (directory, game) = fixture();
    let sources = BuildMaterialSources::default();
    let mut region = Materials::with_build_sources(&game, Requirements::Compile, &sources);
    region.load("source").unwrap();
    let original = sources.retained("materials/source.vmt").unwrap().unwrap();
    assert_eq!(original.bytes(), MATERIAL);
    let guard = sources.source_guard(&game, &Control::default()).unwrap();
    std::fs::write(
        directory.path().join("materials/source.vmt"),
        br#"LightmappedGeneric { $reflectivity "[0.9 0.8 0.7]" }"#,
    )
    .unwrap();
    let mut main = Materials::with_build_sources(&game, Requirements::Compile, &sources);
    let error = main.load("source").unwrap_err();
    assert!(error.contains("changed"), "{error}");
    assert_eq!(original.bytes(), MATERIAL);
    assert!(guard.verify_metadata().is_err());
    assert!(sources.source_guard(&game, &Control::default()).is_err());
}

#[test]
fn a_new_higher_priority_winner_invalidates_the_original_lookup_and_guard() {
    let (directory, mut game) = fixture();
    let higher = directory.path().join("higher");
    std::fs::create_dir_all(higher.join("materials")).unwrap();
    game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(higher.clone()),
        },
    );
    let sources = BuildMaterialSources::default();
    let original = sources
        .read_asset(&game, "materials/source.vmt", 1024)
        .unwrap()
        .unwrap();
    let guard = sources.source_guard(&game, &Control::default()).unwrap();
    std::fs::write(higher.join("materials/source.vmt"), MATERIAL).unwrap();
    assert_eq!(
        std::fs::read(directory.path().join("materials/source.vmt")).unwrap(),
        MATERIAL
    );
    assert!(guard.verify_sources(&game, &Control::default()).is_err());
    assert!(sources.source_guard(&game, &Control::default()).is_err());
    let error = sources
        .read_asset(&game, "materials/source.vmt", 1024)
        .unwrap_err();
    assert!(error.to_string().contains("winning/missing lookup changed"));
    assert_eq!(original.bytes(), MATERIAL);
}

#[test]
fn original_logical_lookups_survive_guard_handoff_when_only_mount_order_changes() {
    let (directory, mut game) = fixture();
    let higher = directory.path().join("higher");
    std::fs::create_dir_all(higher.join("materials")).unwrap();
    std::fs::write(higher.join("materials/source.vmt"), MATERIAL).unwrap();
    let sources = BuildMaterialSources::default();
    sources
        .read_asset(&game, "materials/source.vmt", 1024)
        .unwrap();
    let guard = sources.source_guard(&game, &Control::default()).unwrap();
    game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(higher),
        },
    );
    guard.verify_metadata().unwrap();
    assert!(guard.verify_sources(&game, &Control::default()).is_err());
    assert!(sources.source_guard(&game, &Control::default()).is_err());
}

#[test]
fn missing_sources_are_original_observations_and_cannot_be_rebaselined() {
    let (directory, game) = fixture();
    let sources = BuildMaterialSources::default();
    let path = "materials/missing.vmt";
    assert!(!sources.is_observed(path).unwrap());
    assert!(sources.read_asset(&game, path, 1024).unwrap().is_none());
    assert!(sources.is_observed(path).unwrap());
    assert!(sources.retained(path).unwrap().is_none());
    let guard = sources.source_guard(&game, &Control::default()).unwrap();
    assert!(!guard.material_lookups[path].present);
    std::fs::write(directory.path().join(path), MATERIAL).unwrap();
    assert!(guard.verify_metadata().is_err());
    assert!(sources.source_guard(&game, &Control::default()).is_err());
    let error = sources.read_asset(&game, path, 1024).unwrap_err();
    assert!(error.to_string().contains("winning/missing lookup changed"));
}

#[test]
fn repeated_actual_reads_share_one_retained_payload_without_spending_retention_twice() {
    let (directory, game) = fixture();
    std::fs::write(directory.path().join("materials/one.vtf"), b"1234").unwrap();
    std::fs::write(directory.path().join("materials/two.vtf"), b"x").unwrap();
    let sources = owner(Limits {
        payload_bytes: 4,
        ..Limits::default()
    });
    let first = sources
        .read_asset(&game, "materials/one.vtf", 4)
        .unwrap()
        .unwrap();
    let second = sources
        .read_asset(&game, r"MATERIALS\ONE.VTF", 4)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&first.source, &second.source));
    assert_eq!(sources.lock().unwrap().payload_bytes, 4);
    assert!(
        sources
            .read_asset(&game, "materials/absent.vtf", 4)
            .unwrap()
            .is_none()
    );
    let error = sources
        .read_asset(&game, "materials/two.vtf", 4)
        .unwrap_err();
    assert!(error.to_string().contains("retained payload limit 4 bytes"));
    assert!(error.to_string().contains("effective read limit 0 bytes"));
}

#[test]
fn logical_lookup_and_physical_stamp_bounds_are_independent_of_payload_retention() {
    let (_directory, game) = fixture();
    for (limits, expected) in [
        (
            Limits {
                logical_paths: 0,
                ..Limits::default()
            },
            "0 logical paths",
        ),
        (
            Limits {
                lookup_rows: 0,
                ..Limits::default()
            },
            "0 retained lookup rows",
        ),
        (
            Limits {
                physical_paths: 0,
                ..Limits::default()
            },
            "0 physical lookup paths",
        ),
    ] {
        let sources = owner(limits);
        let error = sources
            .read_asset(&game, "materials/source.vmt", 1024)
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
}

#[test]
fn original_guards_outlive_the_owner_without_retaining_unrelated_payloads() {
    let (_directory, game) = fixture();
    let sources = BuildMaterialSources::default();
    let asset = sources
        .read_asset(&game, "materials/source.vmt", 1024)
        .unwrap()
        .unwrap();
    let payload = Arc::downgrade(&asset.source);
    let guard = sources.source_guard(&game, &Control::default()).unwrap();
    assert!(guard.material_lookups[asset.path()].present);
    drop(asset);
    drop(sources);
    assert!(payload.upgrade().is_none());
    guard.verify_sources(&game, &Control::default()).unwrap();
}

#[test]
fn dry_metric_payloads_are_not_charged_to_one_water_graphs_source_limit() {
    let (directory, game) = fixture();
    let path = "materials/dry.vtf";
    let bytes = crate::water_dependencies::MAX_SOURCE_BYTES + 1;
    let file = std::fs::File::create(directory.path().join(path)).unwrap();
    file.set_len(bytes as u64).unwrap();
    let sources = BuildMaterialSources::default();
    let asset = sources.read_asset(&game, path, bytes).unwrap().unwrap();
    assert_eq!(asset.bytes().len(), bytes);
    assert_eq!(sources.lock().unwrap().payload_bytes, bytes);
    sources.source_guard(&game, &Control::default()).unwrap();
}
