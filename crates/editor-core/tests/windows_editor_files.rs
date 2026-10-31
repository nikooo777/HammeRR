#![cfg(windows)]

use editor_core::{
    prefabs::Store,
    session::{Actor, Session},
};
use glam::DVec3;
use std::fs;

#[test]
fn allow_game_logs_without_accepting_changed_materials() -> Result<(), Box<dyn std::error::Error>> {
    use compiler::{Control, asset_sources::BuildMaterialSources};
    use source_formats::gamefs::{GameFs, Mount, MountedPath};
    let directory = tempfile::tempdir()?;
    let root = directory.path().canonicalize()?;
    fs::create_dir(root.join("materials"))?;
    let material = root.join("materials/test.vmt");
    fs::write(&material, b"LightmappedGeneric {}")?;
    let game = GameFs {
        game_dir: root.clone(),
        base_dir: root.clone(),
        mounts: vec![MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(root.clone()),
        }],
    };
    let sources = BuildMaterialSources::default();
    assert!(
        sources
            .read_asset(&game, "materials/test.vmt", 4096)?
            .is_some()
    );
    let control = Control::default();
    let guard = sources.source_guard(&game, &control)?;
    fs::write(root.join("hammerr_console.log"), b"game launch")?;
    guard.verify_resolved_sources(&game, &control)?;
    fs::write(material, b"LightmappedGeneric { changed 1 }")?;
    assert!(guard.verify_resolved_sources(&game, &control).is_err());
    Ok(())
}

#[test]
fn save_reopen_and_update_a_map() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("map.vmf");
    let mut session = Session::new(Err("No game assets needed for this test".into()));
    session.create_block(
        Actor::User,
        DVec3::ZERO,
        DVec3::splat(128.0),
        "tools/toolsnodraw",
        false,
    )?;
    session.save(Actor::User, Some(&path))?;
    drop(session);
    let original = fs::read(&path)?;
    let mut session = Session::new(Err("No game assets needed for this test".into()));
    session.open(Actor::User, &path, true)?;
    session.create_block(
        Actor::User,
        DVec3::splat(256.0),
        DVec3::splat(384.0),
        "tools/toolsnodraw",
        false,
    )?;
    session.save(Actor::User, None)?;
    assert_ne!(fs::read(path)?, original);
    Ok(())
}

#[test]
fn save_read_rename_and_remove_a_prefab() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let store = Store {
        directory: directory.path().join("prefabs"),
    };
    let empty = store.snapshot()?;
    let text = "world\n{\n\"id\" \"1\"\n}\n";
    store.save("room", text, &empty.stamp)?;
    drop(empty);
    let saved = store.snapshot()?;
    assert_eq!(store.read("room", &saved.stamp)?, text);
    store.rename("room", "hall", &saved.stamp)?;
    drop(saved);
    let renamed = store.snapshot()?;
    assert_eq!(store.read("hall", &renamed.stamp)?, text);
    store.remove("hall", &renamed.stamp)?;
    drop(renamed);
    assert!(store.snapshot()?.entries.is_empty());
    Ok(())
}

#[test]
fn preserve_an_existing_prefab_when_renaming() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let store = Store {
        directory: directory.path().join("prefabs"),
    };
    store.save("room", "first", &store.snapshot()?.stamp)?;
    store.save("hall", "second", &store.snapshot()?.stamp)?;
    let snapshot = store.snapshot()?;
    assert!(store.rename("room", "hall", &snapshot.stamp).is_err());
    assert_eq!(store.read("room", &snapshot.stamp)?, "first");
    assert_eq!(store.read("hall", &snapshot.stamp)?, "second");
    Ok(())
}
