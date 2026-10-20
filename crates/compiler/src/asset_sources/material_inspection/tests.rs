use std::path::Path;

use source_formats::gamefs::{Mount, MountedPath};
use source_formats::vmt::Material;
use source_formats::vpk::Vpk;

use super::*;

const ROOT: &[u8] = b"Water { $normalmap dev/original $fogstart 16 }";

fn game(root: &Path, mounts: Vec<Mount>) -> GameFs {
    GameFs {
        game_dir: root.to_owned(),
        base_dir: root.to_owned(),
        mounts: mounts
            .into_iter()
            .map(|mount| MountedPath {
                ids: vec!["game".into()],
                mount,
            })
            .collect(),
    }
}

fn fixture() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    std::fs::write(directory.path().join("materials/source.vmt"), ROOT).unwrap();
    let game = game(
        directory.path(),
        vec![Mount::Directory(directory.path().to_owned())],
    );
    (directory, game)
}

fn policy() -> SourceGuardPolicy {
    SourceGuardPolicy {
        label: "Material inspection",
        missing_label: "Material inspection candidate",
        asset_bytes: 1024,
        physical_paths: 128,
    }
}

#[test]
fn cached_root_and_patch_bytes_remain_the_original_parse_after_both_sources_change() {
    let (directory, game) = fixture();
    let path = directory.path().join("materials/source.vmt");
    let patch = b"Patch { include materials/base.vmt replace { $fogstart 64 } }";
    std::fs::write(&path, patch).unwrap();
    let base = directory.path().join("materials/base.vmt");
    std::fs::write(&base, ROOT).unwrap();
    let control = Control::default();
    let mut capture = MaterialInspectionCapture::new(&game, &control, policy());
    let first = Material::load_with_reader("materials/source.vmt", &mut |path| {
        capture
            .read_material(path, 1024)
            .map(|asset| asset.map(|asset| asset.bytes().to_vec()))
    })
    .unwrap()
    .unwrap();
    assert_eq!(first.shader, "water");
    assert_eq!(first.get("$normalmap"), Some("dev/original"));
    assert_eq!(first.get("$fogstart"), Some("64"));
    capture.verify_originals().unwrap();
    assert_eq!(capture.payload_reads, 2);
    let original = capture
        .read_material("materials/source.vmt", 1024)
        .unwrap()
        .unwrap();
    std::fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();
    std::fs::write(&base, b"Water { $normalmap dev/replacement $fogstart 1 }").unwrap();
    let cached = capture
        .read_material(r".\MATERIALS\SOURCE.VMT", 1024)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&original.source, &cached.source));
    assert_eq!(cached.path(), "materials/source.vmt");
    assert_eq!(cached.bytes(), patch);
    let again = Material::load_with_reader("materials/source.vmt", &mut |path| {
        capture
            .read_material(path, 1024)
            .map(|asset| asset.map(|asset| asset.bytes().to_vec()))
    })
    .unwrap()
    .unwrap();
    assert_eq!(again.shader, first.shader);
    assert_eq!(again.get("$normalmap"), first.get("$normalmap"));
    assert_eq!(again.get("$fogstart"), first.get("$fogstart"));
    assert_eq!(
        capture
            .materials()
            .map(CapturedMaterial::path)
            .collect::<Vec<_>>(),
        ["materials/base.vmt", "materials/source.vmt"]
    );
    assert!(capture.verify_originals().is_err());
    assert_eq!(
        capture.issue().unwrap().kind,
        MaterialInspectionIssueKind::UnavailableSource
    );
    assert_eq!(capture.payload_reads, 2);
}

#[test]
fn same_byte_inode_replacement_cannot_rebaseline_a_cached_material() {
    let (directory, game) = fixture();
    let control = Control::default();
    let mut capture = MaterialInspectionCapture::new(&game, &control, policy());
    let original = capture
        .read_material("materials/source.vmt", ROOT.len())
        .unwrap()
        .unwrap();
    let replacement = directory.path().join("replacement.vmt");
    std::fs::write(&replacement, ROOT).unwrap();
    std::fs::rename(replacement, directory.path().join("materials/source.vmt")).unwrap();
    let cached = capture
        .read_material("materials//./source.vmt", ROOT.len())
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&original.source, &cached.source));
    assert_eq!(cached.bytes(), ROOT);
    assert!(capture.verify_originals().is_err());
    assert!(capture.verify_originals().is_err());
    assert_eq!(capture.payload_reads, 1);
}

#[test]
fn missing_material_and_texture_paths_stay_missing_after_they_appear() {
    for material in [false, true] {
        let (directory, game) = fixture();
        let path = if material {
            "materials/absent.vmt"
        } else {
            "materials/absent.vtf"
        };
        let control = Control::default();
        let mut capture = MaterialInspectionCapture::new(&game, &control, policy());
        if material {
            assert!(capture.read_material(path, 1024).unwrap().is_none());
        } else {
            assert!(capture.read_texture(path, 1024).unwrap().is_none());
        }
        capture.verify_originals().unwrap();
        assert_eq!(capture.payload_reads, 1);
        std::fs::write(directory.path().join(path), ROOT).unwrap();
        if material {
            assert!(capture.read_material(path, 1024).unwrap().is_none());
        } else {
            assert!(capture.read_texture(path, 1024).unwrap().is_none());
        }
        assert!(capture.verify_originals().is_err());
        assert_eq!(capture.payload_reads, 1);
    }
}

#[test]
fn cached_limits_and_initial_read_errors_preserve_the_ordinary_payload_errors() {
    let (directory, game) = fixture();
    let path = "materials/source.vmt";
    let control = Control::default();
    let mut capture = MaterialInspectionCapture::new(
        &game,
        &control,
        SourceGuardPolicy {
            asset_bytes: 1,
            ..policy()
        },
    );
    let original = capture.read_material(path, ROOT.len()).unwrap().unwrap();
    let expected = game.read_asset_limited(path, ROOT.len() - 1).unwrap_err();
    let limited = capture.read_material(path, ROOT.len() - 1).unwrap_err();
    assert_eq!(limited.kind(), expected.kind());
    assert_eq!(limited.to_string(), expected.to_string());
    assert!(Arc::ptr_eq(
        &original.source,
        &capture
            .read_material(path, ROOT.len())
            .unwrap()
            .unwrap()
            .source
    ));
    capture.verify_originals().unwrap();
    assert_eq!(capture.payload_reads, 1);

    let mut failed = MaterialInspectionCapture::new(&game, &control, policy());
    let first = failed.read_material(path, ROOT.len() - 1).unwrap_err();
    std::fs::write(directory.path().join(path), b"Water {}").unwrap();
    let second = failed.read_material(path, 1024).unwrap_err();
    assert_eq!(second.kind(), first.kind());
    assert_eq!(second.to_string(), first.to_string());
    assert!(failed.verify_originals().is_err());
    assert_eq!(failed.payload_reads, 1);
}

#[test]
fn observation_path_limits_are_sticky_without_replacing_ordinary_material_results() {
    let (_directory, game) = fixture();
    let control = Control::default();
    let mut capture = MaterialInspectionCapture::new(
        &game,
        &control,
        SourceGuardPolicy {
            physical_paths: 0,
            ..policy()
        },
    );
    let first = capture
        .read_material("materials/source.vmt", 1024)
        .unwrap()
        .unwrap();
    let material = Material::load_with_reader(first.path(), &mut |path| {
        capture
            .read_material(path, 1024)
            .map(|asset| asset.map(|asset| asset.bytes().to_vec()))
    })
    .unwrap()
    .unwrap();
    assert_eq!(material.shader, "water");
    assert_eq!(material.get("$normalmap"), Some("dev/original"));
    let issue = capture.issue().unwrap();
    assert_eq!(issue.kind, MaterialInspectionIssueKind::Limited);
    assert_eq!(issue.path, "materials/source.vmt");
    assert!(issue.message.contains("0 retained lookup rows"));
    let first_issue = issue.message.clone();
    assert_eq!(capture.lookup_rows, 0);
    assert!(capture.stamps.is_empty());
    assert!(
        capture.observations["materials/source.vmt"]
            .lookup
            .is_none()
    );
    assert!(capture.verify_originals().is_err());
    assert!(Arc::ptr_eq(
        &first.source,
        &capture
            .read_material(first.path(), 1024)
            .unwrap()
            .unwrap()
            .source
    ));
    assert_eq!(capture.issue().unwrap().message, first_issue);
    assert_eq!(capture.payload_reads, 1);
}

#[test]
fn repeated_lookup_paths_count_toward_one_retained_row_budget_before_they_are_stored() {
    let (directory, game) = fixture();
    std::fs::write(directory.path().join("materials/extra.vmt"), b"Water {}").unwrap();
    let paths = ["materials/source.vmt", "materials/extra.vmt"];
    let lookups = paths.map(|path| game.asset_source_paths(path).unwrap().unwrap());
    let rows = lookups
        .iter()
        .map(|lookup| {
            lookup.files.len() + lookup.lookup_directories.len() + lookup.lookup_symlinks.len()
        })
        .sum::<usize>();
    let unique = lookups
        .iter()
        .flat_map(|lookup| {
            lookup
                .files
                .iter()
                .chain(&lookup.lookup_directories)
                .chain(&lookup.lookup_symlinks)
        })
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    assert!(unique < rows);
    let control = Control::default();
    let mut complete = MaterialInspectionCapture::new(
        &game,
        &control,
        SourceGuardPolicy {
            physical_paths: rows,
            ..policy()
        },
    );
    for path in paths {
        assert!(complete.read_material(path, 1024).unwrap().is_some());
    }
    assert_eq!(complete.lookup_rows, rows);
    complete.verify_originals().unwrap();
    assert_eq!(complete.payload_reads, 2);

    assert!(unique < rows);
    let mut limited = MaterialInspectionCapture::new(
        &game,
        &control,
        SourceGuardPolicy {
            physical_paths: rows - 1,
            ..policy()
        },
    );
    let first = limited.read_material(paths[0], 1024).unwrap().unwrap();
    let second = limited.read_material(paths[1], 1024).unwrap().unwrap();
    assert_eq!(first.bytes(), ROOT);
    assert_eq!(second.bytes(), b"Water {}");
    assert_eq!(
        limited.lookup_rows,
        lookups[0].files.len()
            + lookups[0].lookup_directories.len()
            + lookups[0].lookup_symlinks.len()
    );
    assert!(limited.observations[paths[0]].lookup.is_some());
    assert!(limited.observations[paths[1]].lookup.is_none());
    assert_eq!(
        limited.issue().unwrap().kind,
        MaterialInspectionIssueKind::Limited
    );
    assert_eq!(limited.issue().unwrap().path, paths[1]);
    assert!(limited.verify_originals().is_err());
    assert!(Arc::ptr_eq(
        &second.source,
        &limited
            .read_material(paths[1], 1024)
            .unwrap()
            .unwrap()
            .source
    ));
    assert_eq!(limited.payload_reads, 2);
}

#[test]
fn an_initial_stamp_conflict_does_not_hide_a_successful_second_payload_read() {
    let (directory, game) = fixture();
    let control = Control::default();
    let mut capture = MaterialInspectionCapture::new(&game, &control, policy());
    capture
        .read_material("materials/source.vmt", 1024)
        .unwrap()
        .unwrap();
    assert!(capture.issue().is_none());
    std::fs::write(directory.path().join("materials/extra.vmt"), b"Water {}").unwrap();
    let second = capture
        .read_material("materials/extra.vmt", 1024)
        .unwrap()
        .unwrap();
    assert_eq!(second.bytes(), b"Water {}");
    let issue = capture.issue().unwrap();
    assert_eq!(issue.kind, MaterialInspectionIssueKind::UnavailableSource);
    assert_eq!(issue.path, "materials/extra.vmt");
    assert!(
        issue
            .message
            .contains("backing file or lookup path changed")
    );
    assert!(capture.verify_originals().is_err());
    assert_eq!(capture.payload_reads, 2);
}

#[test]
fn an_equal_byte_higher_priority_winner_does_not_replace_the_observed_origin() {
    let (directory, mut game) = fixture();
    let high = directory.path().join("high");
    std::fs::create_dir(&high).unwrap();
    std::fs::create_dir(high.join("materials")).unwrap();
    game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(high.clone()),
        },
    );
    let control = Control::default();
    let mut capture = MaterialInspectionCapture::new(&game, &control, policy());
    let first = capture
        .read_material("materials/source.vmt", 1024)
        .unwrap()
        .unwrap();
    let AssetOrigin::Directory { mount, .. } = first.origin() else {
        panic!()
    };
    assert_eq!(mount, directory.path());
    std::fs::write(high.join("materials/source.vmt"), ROOT).unwrap();
    let current = game
        .read_asset_limited("materials/source.vmt", 1024)
        .unwrap()
        .unwrap();
    let AssetOrigin::Directory { mount, .. } = current.origin else {
        panic!()
    };
    assert_eq!(mount, high);
    assert_eq!(current.data, ROOT);
    let cached = capture
        .read_material("materials/source.vmt", 1024)
        .unwrap()
        .unwrap();
    assert_eq!(cached.origin(), first.origin());
    assert!(Arc::ptr_eq(&first.source, &cached.source));
    assert!(capture.verify_originals().is_err());
    assert_eq!(capture.payload_reads, 1);
}

#[cfg(unix)]
#[test]
fn equal_byte_mount_symlink_retargeting_keeps_the_original_handle_and_refuses_verification() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    for name in ["first", "second"] {
        std::fs::create_dir_all(directory.path().join(name).join("materials")).unwrap();
        std::fs::write(
            directory.path().join(name).join("materials/source.vmt"),
            ROOT,
        )
        .unwrap();
    }
    let alias = directory.path().join("mounted");
    symlink(directory.path().join("first"), &alias).unwrap();
    let game = game(directory.path(), vec![Mount::Directory(alias.clone())]);
    let control = Control::default();
    let mut capture = MaterialInspectionCapture::new(&game, &control, policy());
    let first = capture
        .read_material("materials/source.vmt", 1024)
        .unwrap()
        .unwrap();
    std::fs::remove_file(&alias).unwrap();
    symlink(directory.path().join("second"), &alias).unwrap();
    let cached = capture
        .read_material("materials/source.vmt", 1024)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&first.source, &cached.source));
    assert!(capture.verify_originals().is_err());
    assert_eq!(capture.payload_reads, 1);
}

fn vpk(path: &Path, payload: &[u8]) {
    let mut tree = b"vmt\0materials\0source\0".to_vec();
    tree.extend(0u32.to_le_bytes());
    tree.extend(0u16.to_le_bytes());
    tree.extend(0x7fffu16.to_le_bytes());
    tree.extend(0u32.to_le_bytes());
    tree.extend((payload.len() as u32).to_le_bytes());
    tree.extend(0xffffu16.to_le_bytes());
    tree.extend([0, 0, 0]);
    let mut bytes = 0x55aa_1234u32.to_le_bytes().to_vec();
    bytes.extend(1u32.to_le_bytes());
    bytes.extend((tree.len() as u32).to_le_bytes());
    bytes.extend(tree);
    bytes.extend(payload);
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn vpk_origin_cached_limits_and_original_index_identity_survive_payload_cache_hits() {
    let (directory, mut game) = fixture();
    let index = directory.path().join("source_dir.vpk");
    vpk(&index, ROOT);
    game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Pak(Vpk::open(&index).unwrap()),
        },
    );
    let control = Control::default();
    let mut capture = MaterialInspectionCapture::new(&game, &control, policy());
    let original = capture
        .read_material("materials/source.vmt", ROOT.len())
        .unwrap()
        .unwrap();
    let AssetOrigin::Vpk {
        index: observed,
        files,
    } = original.origin()
    else {
        panic!()
    };
    assert_eq!(observed, &index);
    assert!(files.contains(&index));
    let expected = game
        .read_asset_limited("materials/source.vmt", ROOT.len() - 1)
        .unwrap_err();
    let limited = capture
        .read_material("materials/source.vmt", ROOT.len() - 1)
        .unwrap_err();
    assert_eq!(limited.kind(), expected.kind());
    assert_eq!(limited.to_string(), expected.to_string());
    capture.verify_originals().unwrap();
    let replacement = directory.path().join("replacement.vpk");
    std::fs::copy(&index, &replacement).unwrap();
    std::fs::rename(replacement, &index).unwrap();
    let cached = capture
        .read_material("materials/source.vmt", ROOT.len())
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&original.source, &cached.source));
    assert_eq!(cached.origin(), original.origin());
    assert!(capture.verify_originals().is_err());
    assert_eq!(capture.payload_reads, 1);
}

#[test]
fn texture_payload_is_consumed_once_while_its_whole_file_hash_and_origin_remain() {
    let (directory, game) = fixture();
    std::fs::write(directory.path().join("materials/normal.vtf"), b"test").unwrap();
    let control = Control::default();
    let mut capture = MaterialInspectionCapture::new(&game, &control, policy());
    let texture = capture
        .read_texture("materials/normal.vtf", 4)
        .unwrap()
        .unwrap();
    assert_eq!(texture.data, b"test");
    let origin = texture.origin.clone();
    drop(texture);
    assert!(capture.materials().next().is_none());
    assert!(matches!(
        capture.reads["materials/normal.vtf"],
        CachedRead::Texture(4)
    ));
    let content = capture.observations["materials/normal.vtf"]
        .content
        .as_ref()
        .unwrap();
    assert_eq!(
        content.sha256,
        "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
    );
    assert_eq!(content.origin, origin);
    capture.verify_originals().unwrap();
    let error = capture.read_texture("materials/normal.vtf", 4).unwrap_err();
    assert!(error.to_string().contains("already consumed"));
    assert_eq!(capture.payload_reads, 1);
    std::fs::write(directory.path().join("materials/normal.vtf"), b"best").unwrap();
    assert!(capture.verify_originals().is_err());
    assert_eq!(capture.payload_reads, 1);
}

#[test]
fn cancellation_between_original_reads_prevents_another_read_and_final_verification() {
    let (_directory, game) = fixture();
    let control = Control::default();
    let mut capture = MaterialInspectionCapture::new(&game, &control, policy());
    let original = capture
        .read_material("materials/source.vmt", 1024)
        .unwrap()
        .unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| control.cancel().unwrap()).join().unwrap();
    });
    for path in ["materials/source.vmt", "materials/unread.vmt"] {
        let error = capture.read_material(path, 1024).unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    }
    let error = capture
        .read_texture("materials/unread.vtf", 1024)
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert!(
        capture
            .verify_originals()
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    assert_eq!(original.bytes(), ROOT);
    assert_eq!(capture.payload_reads, 1);
    assert_eq!(capture.reads.len(), 1);
}
