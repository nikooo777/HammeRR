use super::*;
use glam::DVec3;
use std::{collections::BTreeSet, fs};

fn session(directory: &std::path::Path) -> Session {
    let mut session = Session::new(Err("no assets required".into()));
    session.configure_recovery(Ok(directory.to_owned()));
    session
}

#[test]
fn stale_writers_preserve_disk_and_owned_drafts_until_explicit_reload() {
    let directory = tempfile::tempdir().unwrap();
    let mut first = session(directory.path());
    let mut second = session(directory.path());
    let block = second
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.),
            "test/base",
            false,
        )
        .unwrap();
    second
        .preview_transform(
            Actor::User,
            &[block],
            map_core::edit::Transform {
                translation: DVec3::X * 16.,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let source = second.document().vmf().clone();
    let preview = second.render_document().vmf().clone();
    let history = second.history_snapshot();
    let camera = second.camera();
    let serials = (
        second.scene_serial(),
        second.view_serial(),
        second.camera_serial(),
        second.materials().generation(),
    );
    first
        .favorite_material("Materials/FIRST.vmt", true)
        .unwrap();
    let path = directory.path().join(crate::material_favorites::FILE_NAME);
    let bytes = fs::read(&path).unwrap();
    assert!(
        second
            .favorite_material("second", true)
            .unwrap_err()
            .contains("Reload favorites")
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(!second.is_material_favorite("first"));
    assert_eq!(
        second.material_favorite_status().persistence,
        Persistence::Blocked
    );
    second.reload_material_favorites().unwrap();
    assert!(second.is_material_favorite("first"));
    second.favorite_material("second", true).unwrap();
    assert!(first.favorite_material("first", false).is_err());
    first.reload_material_favorites().unwrap();
    first.favorite_material("first", false).unwrap();
    second.reload_material_favorites().unwrap();
    assert!(!second.is_material_favorite("first"));
    assert!(second.is_material_favorite("second"));
    let before = fs::read(&path).unwrap();
    let serial = second.material_favorites_serial();
    second
        .favorite_material("materials/SECOND.vmt", true)
        .unwrap();
    assert_eq!(second.material_favorites_serial(), serial);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(second.document().vmf(), &source);
    assert_eq!(second.render_document().vmf(), &preview);
    assert_eq!(second.history_snapshot(), history);
    assert_eq!(second.camera(), camera);
    assert_eq!(
        (
            second.scene_serial(),
            second.view_serial(),
            second.camera_serial(),
            second.materials().generation()
        ),
        serials
    );
}

#[test]
fn malformed_reload_keeps_local_set_and_session_only_reload_does_not_block_editing() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = session(directory.path());
    session.favorite_material("local", true).unwrap();
    let path = directory.path().join(crate::material_favorites::FILE_NAME);
    let bytes = fs::read(&path).unwrap();
    fs::write(&path, b"malformed").unwrap();
    assert!(session.reload_material_favorites().is_err());
    assert!(session.is_material_favorite("local"));
    assert!(session.favorite_material("new", true).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"malformed");
    fs::write(&path, bytes).unwrap();
    session.reload_material_favorites().unwrap();
    session.favorite_material("new", true).unwrap();
    assert!(session.material_favorite_status().error.is_none());
    let mut isolated = Session::new(Err("no store".into()));
    isolated.favorite_material("one", true).unwrap();
    assert!(!isolated.material_favorite_status().reload_available);
    assert!(isolated.reload_material_favorites().is_err());
    assert_eq!(
        isolated.material_favorite_status().persistence,
        Persistence::Session
    );
    isolated.favorite_material("two", true).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn favorite_lock_contention_retries_and_replaced_sources_require_reload() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = session(directory.path());
    let lock = crate::preference_file::WriteLock::acquire(
        directory.path(),
        ".material-favorites.lock",
        "Material favorites",
        "Reload favorites",
    )
    .unwrap();
    assert!(
        session
            .favorite_material("one", true)
            .unwrap_err()
            .contains("busy")
    );
    assert_eq!(
        session.material_favorite_status().persistence,
        Persistence::Persistent
    );
    drop(lock);
    session.favorite_material("one", true).unwrap();
    let path = directory.path().join(crate::material_favorites::FILE_NAME);
    let bytes = fs::read(&path).unwrap();
    fs::rename(&path, directory.path().join("old.json")).unwrap();
    fs::write(&path, &bytes).unwrap();
    assert!(session.favorite_material("two", true).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    session.reload_material_favorites().unwrap();
    session.favorite_material("two", true).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn favorites_missing_file_keeps_referring_directory_identity() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first");
    let other = directory.path().join("other");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&other).unwrap();
    let alias = directory.path().join("alias");
    symlink(&first, &alias).unwrap();
    let mut session = session(&alias);
    fs::remove_file(&alias).unwrap();
    symlink(&other, &alias).unwrap();
    assert!(session.favorite_material("one", true).is_err());
    assert!(!other.join(crate::material_favorites::FILE_NAME).exists());
    session.reload_material_favorites().unwrap();
    session.favorite_material("one", true).unwrap();
    assert!(other.join(crate::material_favorites::FILE_NAME).is_file());
}

#[test]
fn favorites_snapshot_cannot_publish_another_preference_file() {
    let directory = tempfile::tempdir().unwrap();
    let favorites = Store {
        directory: directory.path().to_owned(),
    };
    let (_, source) = favorites.load_snapshot().unwrap();
    let other = crate::preference_file::CoordinatedFile {
        directory: directory.path(),
        file_name: "recent-maps.json",
        lock_name: ".recent-maps.lock",
        label: "Recent maps",
        reload: "Reload saved list",
        max_bytes: crate::material_favorites::MAX_FILE_BYTES,
    };
    assert!(other.publish(b"{}", &source).unwrap_err().conflict);
    let bounded = crate::preference_file::CoordinatedFile {
        max_bytes: 1,
        ..other
    };
    let fresh = bounded.observe().unwrap();
    assert!(!bounded.publish(b"{}", &fresh).unwrap_err().conflict);
    assert!(!directory.path().join("recent-maps.json").exists());
    let materials = BTreeSet::from(["one".to_owned()]);
    assert!(
        favorites
            .save_checked(&materials, &source)
            .unwrap()
            .source
            .is_ok()
    );
}
