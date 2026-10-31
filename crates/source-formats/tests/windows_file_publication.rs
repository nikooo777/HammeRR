#![cfg(windows)]

use std::fs;
use std::io::Write;

use source_formats::file_publication::{BoundDestination, FileIdentity, Policy};

#[test]
fn trace_assets_through_windows_drive_prefixes() -> Result<(), Box<dyn std::error::Error>> {
    use source_formats::gamefs::{GameFs, Mount, MountedPath};
    let directory = tempfile::tempdir()?;
    let material = directory.path().join("materials/test.vmt");
    fs::create_dir(directory.path().join("materials"))?;
    fs::write(&material, b"LightmappedGeneric {}")?;
    for root in [
        directory.path().to_owned(),
        directory.path().canonicalize()?,
    ] {
        let game = GameFs {
            game_dir: root.clone(),
            base_dir: root.clone(),
            mounts: vec![MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Directory(root),
            }],
        };
        let sources = game
            .asset_source_paths("materials/test.vmt")?
            .expect("Material source should be found");
        assert!(sources.files.contains(&material.canonicalize()?));
        assert_eq!(
            game.read("materials/test.vmt")?.as_deref(),
            Some(b"LightmappedGeneric {}".as_slice())
        );
    }
    Ok(())
}

#[test]
fn coordinate_writers_with_an_exclusive_lock() -> Result<(), Box<dyn std::error::Error>> {
    use source_formats::file_publication::windows::lock_file;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("output.lock");
    let lock = lock_file(&path)?;
    assert!(lock_file(&path).is_err());
    drop(lock);
    let _lock = lock_file(&path)?;
    Ok(())
}

#[test]
fn refuse_to_replace_a_rename_destination() -> Result<(), Box<dyn std::error::Error>> {
    use source_formats::file_publication::windows::rename_noreplace;
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("first.vmf");
    let destination = directory.path().join("second.vmf");
    fs::write(&source, b"first")?;
    fs::write(&destination, b"second")?;
    assert!(rename_noreplace(&source, &destination).is_err());
    assert_eq!(fs::read(&source)?, b"first");
    assert_eq!(fs::read(&destination)?, b"second");
    Ok(())
}

#[test]
fn publish_and_overwrite_a_map() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("map.vmf");
    let destination = BoundDestination::capture(&path, Policy::FollowSource)?;
    assert_eq!(destination.read_existing()?, None);
    let mut stage = destination.stage()?;
    stage.write_all(b"first map")?;
    let receipt = stage.commit()?;
    assert!(receipt.warning.is_none());
    assert_eq!(
        receipt.destination.read_existing()?.as_deref(),
        Some(b"first map".as_slice())
    );
    let mut stage = receipt.destination.stage()?;
    stage.write_all(b"updated map")?;
    let receipt = stage.commit()?;
    assert!(receipt.warning.is_none());
    assert_eq!(fs::read(path)?, b"updated map");
    Ok(())
}

#[test]
fn reject_a_destination_changed_after_observation() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("map.vmf");
    fs::write(&path, b"original")?;
    let destination = BoundDestination::capture(&path, Policy::FollowSource)?;
    let mut stage = destination.stage()?;
    stage.write_all(b"editor changes")?;
    let replacement = directory.path().join("replacement.vmf");
    fs::write(&replacement, b"external")?;
    fs::rename(replacement, &path)?;
    assert!(stage.commit().is_err());
    assert_eq!(fs::read(path)?, b"external");
    Ok(())
}

#[test]
fn remove_an_abandoned_stage() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let destination =
        BoundDestination::capture(&directory.path().join("map.vmf"), Policy::ReplaceEntry)?;
    let stage = destination.stage()?;
    assert_eq!(fs::read_dir(directory.path())?.count(), 1);
    drop(stage);
    assert_eq!(fs::read_dir(directory.path())?.count(), 0);
    Ok(())
}

#[test]
fn distinguish_files_and_hard_link_aliases() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let first = directory.path().join("first.vmf");
    let second = directory.path().join("second.vmf");
    let alias = directory.path().join("alias.vmf");
    fs::write(&first, b"same bytes")?;
    fs::write(&second, b"same bytes")?;
    fs::hard_link(&first, &alias)?;
    let first = FileIdentity::from_file(&fs::File::open(first)?)?;
    let second = FileIdentity::from_file(&fs::File::open(second)?)?;
    let alias = FileIdentity::from_file(&fs::File::open(alias)?)?;
    assert_ne!(first, second);
    assert_eq!(first, alias);
    Ok(())
}

#[test]
fn claim_case_insensitive_destination_names() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let lower = BoundDestination::capture(&directory.path().join("map.vmf"), Policy::FollowSource)?;
    let upper = BoundDestination::capture(&directory.path().join("MAP.VMF"), Policy::FollowSource)?;
    assert_eq!(lower.claim_slots(), upper.claim_slots());
    Ok(())
}

#[test]
fn bind_a_parent_created_after_observation() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let parent = directory.path().join("new");
    let path = parent.join("map.vmf");
    let destination = BoundDestination::capture(&path, Policy::ReplaceEntry)?;
    fs::create_dir(&parent)?;
    let mut stage = destination.stage()?;
    stage.write_all(b"new map")?;
    stage.commit()?;
    assert_eq!(fs::read(path)?, b"new map");
    Ok(())
}

#[test]
fn retarget_output_inside_the_same_directory() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let original = directory.path().join("original.bsp");
    let next = directory.path().join("next.bsp");
    let destination = BoundDestination::capture(&original, Policy::ReplaceEntry)?;
    let replacement = BoundDestination::capture(&next, Policy::ReplaceEntry)?;
    let mut stage = destination.stage()?;
    stage.write_all(b"compiled map")?;
    stage.retarget_output(&replacement)?;
    stage.commit()?;
    assert!(!original.exists());
    assert_eq!(fs::read(next)?, b"compiled map");
    Ok(())
}
