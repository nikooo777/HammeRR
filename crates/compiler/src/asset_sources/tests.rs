use super::*;
use source_formats::gamefs::{Mount, MountedPath};

fn fixture() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    let materials = directory.path().join("materials");
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::write(materials.join("source.vmt"), b"source bytes").unwrap();
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

fn policy() -> SourceGuardPolicy {
    SourceGuardPolicy {
        label: "Packed asset",
        missing_label: "Packed asset candidate",
        asset_bytes: 128,
        physical_paths: 128,
    }
}

#[test]
fn generic_guard_checks_non_reflection_hashes_and_backing_identity() {
    let (directory, game) = fixture();
    let path = "materials/source.vmt";
    let digest = hash(b"source bytes");
    let control = Control::default();
    let guard =
        capture_asset_source_guard([(path, digest.as_str())], [], &game, &control, policy())
            .unwrap();
    guard.verify_metadata().unwrap();
    let replacement = directory.path().join("replacement.vmt");
    std::fs::write(&replacement, b"changed data").unwrap();
    std::fs::rename(replacement, directory.path().join(path)).unwrap();
    assert!(guard.verify_metadata().is_err());
    let error =
        capture_asset_source_guard([(path, digest.as_str())], [], &game, &control, policy())
            .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(
        error.to_string(),
        "Packed asset source changed before publication: materials/source.vmt"
    );
}

#[test]
fn generic_guard_applies_caller_byte_path_and_missing_candidate_policy() {
    let (_directory, game) = fixture();
    let path = "materials/source.vmt";
    let digest = hash(b"source bytes");
    let control = Control::default();
    let byte_limit = SourceGuardPolicy {
        asset_bytes: 11,
        ..policy()
    };
    let error =
        capture_asset_source_guard([(path, digest.as_str())], [], &game, &control, byte_limit)
            .unwrap_err();
    assert!(error.to_string().contains("byte read limit"));
    let path_limit = SourceGuardPolicy {
        physical_paths: 0,
        ..policy()
    };
    let error =
        capture_asset_source_guard([(path, digest.as_str())], [], &game, &control, path_limit)
            .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Packed asset source guard exceeds 0 physical lookup paths"
    );
    let error = capture_asset_source_guard([], [path], &game, &control, policy()).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(
        error.to_string(),
        "Packed asset candidate appeared before publication: materials/source.vmt"
    );
    capture_asset_source_guard([], ["materials/absent.vmt"], &game, &control, policy())
        .unwrap()
        .verify_metadata()
        .unwrap();
}

#[test]
fn generic_guard_checks_cancellation_before_present_and_missing_lookup() {
    let (_directory, game) = fixture();
    let control = Control::default();
    control.cancel().unwrap();
    let error =
        capture_asset_source_guard([("../unsafe", "unused")], [], &game, &control, policy())
            .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    let error =
        capture_asset_source_guard([], ["../unsafe"], &game, &control, policy()).unwrap_err();
    assert!(error.to_string().contains("cancelled"));
}
