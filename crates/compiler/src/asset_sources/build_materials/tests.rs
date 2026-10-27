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

#[test]
fn identity_only_reads_release_payloads_without_applying_the_retention_limit() {
    let (_directory, game) = fixture();
    let sources = BuildMaterialSources::without_retained_payloads();
    sources.lock().unwrap().limits.payload_bytes = 0;
    let first = sources
        .read_asset(&game, "materials/source.vmt", MATERIAL.len())
        .unwrap()
        .unwrap();
    let payload = Arc::downgrade(&first.source);
    assert_eq!(first.bytes(), MATERIAL);
    assert_eq!(sources.lock().unwrap().payload_bytes, 0);
    assert!(sources.retained(first.path()).unwrap().is_none());
    let guard = sources.source_guard(&game, &Control::default()).unwrap();
    drop(first);
    assert!(payload.upgrade().is_none());
    let second = sources
        .read_asset(&game, "materials/source.vmt", MATERIAL.len())
        .unwrap()
        .unwrap();
    assert_eq!(second.bytes(), MATERIAL);
    assert_eq!(sources.lock().unwrap().payload_bytes, 0);
    drop(second);
    drop(sources);
    guard.verify_sources(&game, &Control::default()).unwrap();
}

#[test]
fn material_text_retention_preserves_original_vmt_and_releases_texture_payloads() {
    let (directory, game) = fixture();
    let texture_path = "materials/metrics.vtf";
    std::fs::write(directory.path().join(texture_path), b"original texture").unwrap();
    let sources = BuildMaterialSources::with_retained_material_text();
    sources.lock().unwrap().limits.payload_bytes = MATERIAL.len();
    let material = sources
        .read_asset(&game, "materials/source.vmt", 1024)
        .unwrap()
        .unwrap();
    assert_eq!(
        sources.retained(material.path()).unwrap().unwrap().bytes(),
        MATERIAL
    );
    let repeated = sources
        .read_asset(&game, material.path(), 1024)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&material.source, &repeated.source));
    let texture = sources
        .read_asset(&game, texture_path, 1024)
        .unwrap()
        .unwrap();
    let payload = Arc::downgrade(&texture.source);
    assert!(sources.retained(texture_path).unwrap().is_none());
    assert_eq!(sources.lock().unwrap().payload_bytes, MATERIAL.len());
    drop(texture);
    assert!(payload.upgrade().is_none());
    let guard = sources.source_guard(&game, &Control::default()).unwrap();
    std::fs::write(directory.path().join(texture_path), b"changed texture").unwrap();
    assert!(guard.verify_sources(&game, &Control::default()).is_err());
    assert!(sources.read_asset(&game, texture_path, 1024).is_err());
    assert_eq!(material.bytes(), MATERIAL);
}

#[test]
fn identity_only_reads_keep_original_sources_and_missing_observations() {
    for missing in [false, true] {
        let (directory, game) = fixture();
        let sources = BuildMaterialSources::without_retained_payloads();
        let path = if missing {
            "materials/missing.vmt"
        } else {
            "materials/source.vmt"
        };
        let original = sources.read_asset(&game, path, 1024).unwrap();
        assert_eq!(original.is_none(), missing);
        let guard = sources.source_guard(&game, &Control::default()).unwrap();
        std::fs::write(directory.path().join(path), b"changed source bytes").unwrap();
        assert!(sources.read_asset(&game, path, 1024).is_err());
        assert!(sources.source_guard(&game, &Control::default()).is_err());
        assert!(guard.verify_sources(&game, &Control::default()).is_err());
    }
}

#[test]
fn identity_only_source_guards_retain_original_mount_winners() {
    let (directory, mut game) = fixture();
    let higher = directory.path().join("higher");
    std::fs::create_dir_all(higher.join("materials")).unwrap();
    std::fs::write(higher.join("materials/source.vmt"), MATERIAL).unwrap();
    let sources = BuildMaterialSources::without_retained_payloads();
    sources
        .read_asset(&game, "materials/source.vmt", 1024)
        .unwrap();
    let guard = sources.source_guard(&game, &Control::default()).unwrap();
    drop(sources);
    game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(higher),
        },
    );
    guard.verify_metadata().unwrap();
    assert!(guard.verify_sources(&game, &Control::default()).is_err());
}

#[test]
fn identity_only_reads_preserve_byte_and_metadata_limits() {
    let (_directory, game) = fixture();
    let sources = BuildMaterialSources::without_retained_payloads();
    let error = sources
        .read_asset(&game, "materials/source.vmt", MATERIAL.len() - 1)
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "asset exceeds the caller's byte read limit"
    );
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
        sources.lock().unwrap().retention = PayloadRetention::IdentityOnly;
        let error = sources
            .read_asset(&game, "materials/source.vmt", 1024)
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
}

#[test]
fn first_vpk_lookup_uses_caller_and_retained_limits_before_missing_payload_resolution() {
    let (directory, mut game) = fixture();
    let path = "materials/source.vmt";
    let index = directory.path().join("bounded_dir.vpk");
    let archive = directory.path().join("bounded_042.vpk");
    let mut tree = b"vmt\0materials\0source\0".to_vec();
    tree.extend(0u32.to_le_bytes());
    tree.extend(3u16.to_le_bytes());
    tree.extend(42u16.to_le_bytes());
    tree.extend(0u32.to_le_bytes());
    tree.extend(4u32.to_le_bytes());
    tree.extend(0xffffu16.to_le_bytes());
    tree.extend(b"pre\0\0\0");
    let mut bytes = 0x55aa_1234u32.to_le_bytes().to_vec();
    bytes.extend(1u32.to_le_bytes());
    bytes.extend((tree.len() as u32).to_le_bytes());
    bytes.extend(tree);
    std::fs::write(&index, bytes).unwrap();
    std::fs::write(directory.path().join(path), b"low").unwrap();
    game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Pak(source_formats::vpk::Vpk::open(&index).unwrap()),
        },
    );
    for (retention, payload_bytes, maximum) in [
        (PayloadRetention::Retained, 1024, 6),
        (PayloadRetention::Retained, 6, 64),
        (PayloadRetention::IdentityOnly, 0, 6),
    ] {
        let sources = owner(Limits {
            payload_bytes,
            ..Limits::default()
        });
        sources.lock().unwrap().retention = retention;
        let error = sources.read_asset(&game, path, maximum).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error
                .to_string()
                .contains("VPK entry exceeds the caller's byte read limit"),
            "{error}"
        );
        if retention == PayloadRetention::Retained {
            assert!(error.to_string().contains("effective read limit 6 bytes"));
        }
        assert_eq!(
            sources
                .read_asset(&game, path, usize::MAX)
                .unwrap_err()
                .to_string(),
            error.to_string()
        );
        assert_eq!(
            sources
                .source_guard(&game, &Control::default())
                .unwrap_err()
                .to_string(),
            error.to_string()
        );
        assert!(!archive.exists());
    }
    let missing = BuildMaterialSources::without_retained_payloads();
    assert_eq!(
        missing.read_asset(&game, path, 7).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    assert!(!archive.exists());
    std::fs::write(&archive, b"body").unwrap();
    let sources = BuildMaterialSources::without_retained_payloads();
    assert_eq!(
        sources.read_asset(&game, path, 7).unwrap().unwrap().bytes(),
        b"prebody"
    );
    sources.source_guard(&game, &Control::default()).unwrap();
}

const MODEL: &str = "models/props/source.mdl";
const PHYSICS: &str = "models/props/source.phy";

fn model_fixture() -> (tempfile::TempDir, GameFs) {
    let (directory, game) = fixture();
    std::fs::create_dir_all(directory.path().join("models/props")).unwrap();
    std::fs::write(directory.path().join(MODEL), b"original model bytes").unwrap();
    std::fs::write(directory.path().join(PHYSICS), b"original physics bytes").unwrap();
    (directory, game)
}

fn original_model_guard(game: &GameFs, paths: &[&str]) -> AssetSourceGuard {
    let sources = BuildMaterialSources::without_retained_payloads();
    for path in paths {
        sources.read_asset(game, path, 1024).unwrap();
    }
    sources.source_guard(game, &Control::default()).unwrap()
}

#[cfg(unix)]
#[test]
fn resolved_guards_accept_unrelated_siblings_without_rebaselining_publication() {
    let (directory, game) = model_fixture();
    let guard = original_model_guard(&game, &[MODEL, PHYSICS]);
    let original_stamps = guard.stamps.clone();
    let original_lookups = guard.material_lookups.clone();
    for name in ["hammerr_console_fixture.log", "models/props/unrelated.log"] {
        std::fs::write(directory.path().join(name), b"unrelated log").unwrap();
        guard
            .verify_resolved_sources(&game, &Control::default())
            .unwrap();
        assert!(guard.verify_metadata().is_err());
        assert!(guard.verify_sources(&game, &Control::default()).is_err());
    }
    assert_eq!(guard.stamps, original_stamps);
    assert_eq!(guard.material_lookups, original_lookups);
}

#[test]
fn resolved_guards_reject_changed_replaced_or_missing_model_and_physics_files() {
    for path in [MODEL, PHYSICS] {
        for change in ["changed", "replaced", "missing"] {
            let (directory, game) = model_fixture();
            let guard = original_model_guard(&game, &[MODEL, PHYSICS]);
            let file = directory.path().join(path);
            match change {
                "changed" => std::fs::write(&file, b"changed consumed source").unwrap(),
                "replaced" => {
                    let replacement = file.with_extension("replacement");
                    std::fs::write(&replacement, std::fs::read(&file).unwrap()).unwrap();
                    std::fs::rename(replacement, &file).unwrap();
                }
                _ => std::fs::remove_file(&file).unwrap(),
            }
            assert!(
                guard
                    .verify_resolved_sources(&game, &Control::default())
                    .is_err(),
                "{path}: {change}"
            );
        }
    }
}

#[test]
fn resolved_guards_reject_new_winners_and_original_missing_candidates() {
    for change in ["higher file", "mount order", "missing"] {
        let (directory, mut game) = model_fixture();
        let higher = directory.path().join("higher");
        std::fs::create_dir_all(higher.join("models/props")).unwrap();
        if change == "mount order" {
            std::fs::copy(directory.path().join(MODEL), higher.join(MODEL)).unwrap();
        } else {
            game.mounts.insert(
                0,
                MountedPath {
                    ids: vec!["game".into()],
                    mount: Mount::Directory(higher.clone()),
                },
            );
        }
        let missing = "models/props/missing.mdl";
        let guard = original_model_guard(&game, &[MODEL, missing]);
        match change {
            "higher file" => {
                std::fs::copy(directory.path().join(MODEL), higher.join(MODEL)).unwrap();
            }
            "mount order" => game.mounts.insert(
                0,
                MountedPath {
                    ids: vec!["game".into()],
                    mount: Mount::Directory(higher),
                },
            ),
            _ => std::fs::write(directory.path().join(missing), b"new candidate").unwrap(),
        }
        assert!(
            guard
                .verify_resolved_sources(&game, &Control::default())
                .is_err(),
            "{change}"
        );
        assert_eq!(
            std::fs::read(directory.path().join(MODEL)).unwrap(),
            b"original model bytes"
        );
    }
}

#[test]
fn resolved_guards_reject_replaced_lookup_directories_even_when_absence_is_unchanged() {
    let (directory, game) = model_fixture();
    let empty = directory.path().join("models/empty");
    std::fs::create_dir(&empty).unwrap();
    let guard = original_model_guard(&game, &["models/empty/missing.mdl"]);
    std::fs::rename(&empty, directory.path().join("models/retired")).unwrap();
    std::fs::create_dir(&empty).unwrap();
    assert!(
        guard
            .verify_resolved_sources(&game, &Control::default())
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn resolved_guards_keep_mount_alias_identity_and_accept_unrelated_target_siblings() {
    let (directory, mut game) = model_fixture();
    let alias = directory.path().join("mount-alias");
    std::os::unix::fs::symlink(directory.path(), &alias).unwrap();
    game.mounts[0].mount = Mount::Directory(alias.clone());
    let guard = original_model_guard(&game, &[MODEL]);
    std::fs::write(
        directory.path().join("hammerr_console_alias.log"),
        b"unrelated",
    )
    .unwrap();
    guard
        .verify_resolved_sources(&game, &Control::default())
        .unwrap();
    let replacement = directory.path().join("replacement-alias");
    std::os::unix::fs::symlink(directory.path(), &replacement).unwrap();
    std::fs::rename(replacement, alias).unwrap();
    assert!(
        guard
            .verify_resolved_sources(&game, &Control::default())
            .is_err()
    );
}

#[test]
fn resolved_guards_keep_consulted_vpk_index_and_numbered_payload_identity() {
    for changed in ["physics_dir.vpk", "physics_042.vpk"] {
        let (directory, mut game) = model_fixture();
        let physics = std::fs::read(directory.path().join(PHYSICS)).unwrap();
        let mut tree = b"phy\0models/props\0source\0".to_vec();
        tree.extend_from_slice(&0_u32.to_le_bytes());
        tree.extend_from_slice(&0_u16.to_le_bytes());
        tree.extend_from_slice(&42_u16.to_le_bytes());
        tree.extend_from_slice(&0_u32.to_le_bytes());
        tree.extend_from_slice(&u32::try_from(physics.len()).unwrap().to_le_bytes());
        tree.extend_from_slice(&0xffff_u16.to_le_bytes());
        tree.extend_from_slice(b"\0\0\0");
        let mut index = 0x55aa_1234_u32.to_le_bytes().to_vec();
        index.extend_from_slice(&1_u32.to_le_bytes());
        index.extend_from_slice(&u32::try_from(tree.len()).unwrap().to_le_bytes());
        index.extend_from_slice(&tree);
        let index_path = directory.path().join("physics_dir.vpk");
        std::fs::write(&index_path, index).unwrap();
        std::fs::write(directory.path().join("physics_042.vpk"), physics).unwrap();
        game.mounts.insert(
            0,
            MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Pak(source_formats::vpk::Vpk::open(index_path).unwrap()),
            },
        );
        let guard = original_model_guard(&game, &[MODEL, PHYSICS]);
        guard
            .verify_resolved_sources(&game, &Control::default())
            .unwrap();
        let path = directory.path().join(changed);
        let replacement = path.with_extension("replacement");
        std::fs::write(&replacement, std::fs::read(&path).unwrap()).unwrap();
        std::fs::rename(replacement, path).unwrap();
        assert!(
            guard
                .verify_resolved_sources(&game, &Control::default())
                .is_err(),
            "{changed}"
        );
    }
}

#[test]
fn resolved_guards_require_original_logical_observations_and_honor_cancellation() {
    let (_directory, game) = model_fixture();
    let guard = BuildMaterialSources::without_retained_payloads()
        .source_guard(&game, &Control::default())
        .unwrap();
    guard.verify_sources(&game, &Control::default()).unwrap();
    let error = guard
        .verify_resolved_sources(&game, &Control::default())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("requires original logical lookups")
    );
    let digest = hash(b"original model bytes");
    let guard = capture_asset_source_guard(
        [(MODEL, digest.as_str())],
        [],
        &game,
        &Control::default(),
        SourceGuardPolicy {
            label: "Model",
            missing_label: "Model candidate",
            asset_bytes: 1024,
            physical_paths: 128,
        },
    )
    .unwrap();
    guard.verify_metadata().unwrap();
    let error = guard
        .verify_resolved_sources(&game, &Control::default())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("requires original logical lookups")
    );
    let guard = original_model_guard(&game, &[MODEL]);
    let control = Control::default();
    control.cancel().unwrap();
    let error = guard.verify_resolved_sources(&game, &control).unwrap_err();
    assert!(error.to_string().contains("cancelled"));
}

#[test]
fn bounded_preview_owner_preserves_original_payload_and_metadata_guards() {
    let (directory, game) = fixture();
    let control = Control::default();
    let short = BuildMaterialSources::bounded(MATERIAL.len() - 1, 4, 128, 128);
    assert!(
        short
            .read_asset(&game, "materials/source.vmt", MATERIAL.len())
            .is_err()
    );
    assert!(short.original_source_guard(&control, 1024 * 1024).is_err());
    let sources = BuildMaterialSources::bounded(MATERIAL.len(), 4, 128, 128);
    let original = sources
        .read_asset(&game, "materials/source.vmt", MATERIAL.len())
        .unwrap()
        .unwrap();
    assert_eq!(original.bytes(), MATERIAL);
    assert!(sources.original_source_guard(&control, 0).is_err());
    let guard = sources
        .original_source_guard(&control, 1024 * 1024)
        .unwrap();
    std::fs::write(
        directory.path().join("materials/source.vmt"),
        b"UnlitGeneric {}",
    )
    .unwrap();
    assert!(
        guard
            .verify_original_metadata(&control, 1024 * 1024)
            .is_err()
    );
    assert!(
        sources
            .original_source_guard(&control, 1024 * 1024)
            .is_err()
    );
    assert_eq!(original.bytes(), MATERIAL);
    control.cancel().unwrap();
    assert!(
        sources
            .original_source_guard(&control, 1024 * 1024)
            .is_err()
    );
}
