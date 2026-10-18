use super::*;
use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;

use crate::preference_file::WriteLock;

fn update(authority: &Authority, interval: u64, keep: usize) -> Result<SettingsSnapshot, String> {
    authority.update(
        SettingsUpdate {
            interval_seconds: Some(interval),
            keep_per_map: Some(keep),
            ..Default::default()
        },
        None,
    )
}

fn reload(authority: &Authority) -> SettingsSnapshot {
    match authority.reload(None) {
        Ok(snapshot) => snapshot,
        Err(ReloadError::Storage(error) | ReloadError::Stale(error)) => panic!("{error}"),
    }
}

#[test]
fn independent_authorities_reject_stale_sources_until_explicit_reload() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preferences");
    let first = Authority::configured(Ok(path.clone()));
    let second = Authority::configured(Ok(path.clone()));
    let before = second.snapshot();
    let saved = update(&first, 60, 4).unwrap();
    let stale = update(&second, 90, 3).unwrap_err();
    assert!(stale.contains("Reload settings"), "{stale}");
    assert_eq!(second.snapshot().settings, before.settings);
    assert_eq!(second.snapshot().revision, before.revision);
    assert_eq!(update(&second, 90, 3).unwrap_err(), stale);
    let store = Store { directory: path };
    assert_eq!(store.load_settings().unwrap(), saved.settings);
    assert_eq!(reload(&second).settings, saved.settings);
    let merged = second
        .update(
            SettingsUpdate {
                keep_per_map: Some(3),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    assert_eq!(merged.settings.interval_seconds, 60);
    assert_eq!(merged.settings.keep_per_map, 3);
    assert_eq!(store.load_settings().unwrap(), merged.settings);
    assert_eq!(first.snapshot(), saved);
}

#[test]
fn observed_invalid_settings_are_repairable_but_intervening_writes_require_reload() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(FILE_NAME);
    for invalid in [
        b"broken".to_vec(),
        vec![b' '; MAX_SETTINGS_BYTES as usize + 1],
    ] {
        fs::write(&path, &invalid).unwrap();
        let authority = Authority::configured(Ok(directory.path().into()));
        assert!(!authority.snapshot().settings.enabled);
        let mut replacement = invalid.clone();
        replacement.push(b' ');
        fs::write(&path, &replacement).unwrap();
        let before = authority.snapshot();
        assert!(
            update(&authority, 60, 4)
                .unwrap_err()
                .contains("Reload settings")
        );
        assert_eq!(fs::read(&path).unwrap(), replacement);
        assert_eq!(authority.snapshot().settings, before.settings);
        assert_eq!(authority.snapshot().revision, before.revision);
        assert!(matches!(
            authority.reload(None),
            Err(ReloadError::Storage(_))
        ));
        let repaired = update(&authority, 60, 4).unwrap();
        assert_eq!(repaired.settings.interval_seconds, 60);
        assert_eq!(repaired.settings.keep_per_map, 4);
        assert!(repaired.error.is_none());
        assert_eq!(
            Store {
                directory: directory.path().into()
            }
            .load_settings()
            .unwrap(),
            repaired.settings
        );
    }
}

#[test]
fn unreadable_source_cannot_be_repaired_without_a_new_observation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(FILE_NAME);
    fs::create_dir(&path).unwrap();
    let authority = Authority::configured(Ok(directory.path().into()));
    let before = authority.snapshot();
    fs::remove_dir(&path).unwrap();
    fs::write(&path, b"broken").unwrap();
    update(&authority, 60, 4).unwrap_err();
    assert_eq!(fs::read(&path).unwrap(), b"broken");
    assert_eq!(authority.snapshot(), before);
    assert!(matches!(
        authority.reload(None),
        Err(ReloadError::Storage(_))
    ));
    update(&authority, 60, 4).unwrap();
    assert!(authority.snapshot().error.is_none());
}

#[test]
fn symlink_aliases_and_public_store_writes_share_the_physical_destination_lock() {
    let directory = tempfile::tempdir().unwrap();
    let target_directory = directory.path().join("target");
    fs::create_dir(&target_directory).unwrap();
    let target = target_directory.join("shared-settings.json");
    fs::write(&target, serde_json::to_vec(&Settings::default()).unwrap()).unwrap();
    let mut authorities = Vec::new();
    let mut stores = Vec::new();
    for name in ["first", "second"] {
        let root = directory.path().join(name);
        fs::create_dir(&root).unwrap();
        symlink(&target, root.join(FILE_NAME)).unwrap();
        authorities.push(Authority::configured(Ok(root.clone())));
        stores.push(Store { directory: root });
    }
    let guard = WriteLock::acquire(
        &target_directory,
        LOCK_FILE_NAME,
        "Recovery settings",
        "Reload settings",
    )
    .unwrap();
    for (authority, store) in authorities.iter().zip(&stores) {
        let before = authority.snapshot();
        assert!(update(authority, 60, 4).unwrap_err().contains("busy"));
        assert_eq!(authority.snapshot().settings, before.settings);
        assert_eq!(authority.snapshot().revision, before.revision);
        assert!(
            store
                .save_settings(&Settings::default())
                .unwrap_err()
                .contains("busy")
        );
        assert!(!store.directory.join(LOCK_FILE_NAME).exists());
    }
    assert_eq!(stores[0].load_settings().unwrap(), Settings::default());
    drop(guard);
    update(&authorities[0], 60, 4).unwrap();
    assert!(
        update(&authorities[1], 90, 3)
            .unwrap_err()
            .contains("Reload settings")
    );
    reload(&authorities[1]);
    update(&authorities[1], 90, 3).unwrap();
    assert_eq!(stores[0].load_settings().unwrap().interval_seconds, 90);
    for store in &stores {
        assert!(
            fs::symlink_metadata(store.directory.join(FILE_NAME))
                .unwrap()
                .is_symlink()
        );
    }
}

#[test]
fn settings_keep_the_requested_directory_binding_until_reload() {
    for existing in [false, true] {
        for replace in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let original = directory.path().join("original");
            let other = directory.path().join("other");
            fs::create_dir(&original).unwrap();
            fs::create_dir(&other).unwrap();
            if existing {
                Store {
                    directory: original.clone(),
                }
                .save_settings(&Settings::default())
                .unwrap();
            }
            let requested = directory.path().join("requested");
            symlink(&original, &requested).unwrap();
            let authority = Authority::configured(Ok(requested.clone()));
            let before = authority.snapshot();
            if replace {
                fs::rename(&original, directory.path().join("retired")).unwrap();
                fs::create_dir(&original).unwrap();
            } else {
                fs::remove_file(&requested).unwrap();
                symlink(&other, &requested).unwrap();
            }
            let error = update(&authority, 60, 4).unwrap_err();
            assert!(error.contains("directory"), "{existing}/{replace}: {error}");
            assert!(error.contains("Reload settings"), "{error}");
            assert_eq!(authority.snapshot().settings, before.settings);
            assert_eq!(authority.snapshot().revision, before.revision);
            assert!(!requested.join(FILE_NAME).exists());
            assert!(!other.join(FILE_NAME).exists());
            assert_eq!(update(&authority, 60, 4).unwrap_err(), error);
            reload(&authority);
            update(&authority, 60, 4).unwrap();
            assert!(requested.join(FILE_NAME).is_file());
        }
    }
}

#[test]
fn settings_symlinks_reject_replaced_target_directories_and_lock_file_targets() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("settings");
    let physical = directory.path().join("physical");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&physical).unwrap();
    let target = physical.join("shared-settings.json");
    let original = serde_json::to_vec(&Settings::default()).unwrap();
    fs::write(&target, &original).unwrap();
    symlink(&target, root.join(FILE_NAME)).unwrap();
    let authority = Authority::configured(Ok(root.clone()));
    let before = authority.snapshot();
    fs::rename(&physical, directory.path().join("retired")).unwrap();
    fs::create_dir(&physical).unwrap();
    fs::write(&target, &original).unwrap();
    let error = update(&authority, 60, 4).unwrap_err();
    assert!(error.contains("directory"), "{error}");
    assert!(error.contains("Reload settings"), "{error}");
    assert_eq!(authority.snapshot().settings, before.settings);
    assert_eq!(authority.snapshot().revision, before.revision);
    assert_eq!(fs::read(&target).unwrap(), original);
    reload(&authority);
    update(&authority, 60, 4).unwrap();
    assert_eq!(
        Store {
            directory: root.clone()
        }
        .load_settings()
        .unwrap()
        .interval_seconds,
        60
    );
    fs::remove_file(root.join(FILE_NAME)).unwrap();
    let lock_target = physical.join(LOCK_FILE_NAME);
    fs::write(&lock_target, &original).unwrap();
    symlink(&lock_target, root.join(FILE_NAME)).unwrap();
    reload(&authority);
    assert!(
        update(&authority, 90, 3)
            .unwrap_err()
            .contains("coordination lock")
    );
    assert_eq!(fs::read(lock_target).unwrap(), original);
}

#[test]
fn committed_unverified_settings_are_adopted_and_retained_until_verified_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("settings");
    let retired = directory.path().join("retired");
    fs::create_dir(&root).unwrap();
    let authority = Authority::configured(Ok(root.clone()));
    let before = authority.snapshot();
    let replacement = Settings {
        enabled: false,
        interval_seconds: 60,
        keep_per_map: 4,
    };
    let error = authority
        .update_with(
            replacement.clone().into(),
            None,
            |store, settings, expected| {
                let data = serde_json::to_vec_pretty(settings).unwrap();
                store.settings_target().publish_following_with(
                    &data,
                    expected,
                    |destination, data| {
                        let mut stage = destination.stage().unwrap();
                        stage.write_all(data).unwrap();
                        let receipt = stage.commit().unwrap();
                        fs::rename(&root, &retired).unwrap();
                        fs::create_dir(&root).unwrap();
                        fs::write(root.join(FILE_NAME), b"broken").unwrap();
                        Ok(receipt)
                    },
                )
            },
        )
        .unwrap_err();
    assert!(
        error.contains("were written but could not verify"),
        "{error}"
    );
    let committed = authority.snapshot();
    assert_eq!(committed.settings, replacement);
    assert_eq!(committed.revision, before.revision + 1);
    assert_eq!(committed.error.as_deref(), Some(error.as_str()));
    assert_eq!(
        Store { directory: retired }.load_settings().unwrap(),
        replacement
    );
    assert_eq!(update(&authority, 90, 3).unwrap_err(), error);
    assert_eq!(fs::read(root.join(FILE_NAME)).unwrap(), b"broken");
    let failed_reload = match authority.reload(None) {
        Err(ReloadError::Storage(error)) => error,
        _ => panic!("Malformed replacement must fail Reload"),
    };
    assert!(failed_reload.contains(&error));
    assert!(authority.snapshot().error.unwrap().contains(&error));
    assert_eq!(authority.snapshot().settings, replacement);
    assert_eq!(authority.snapshot().revision, committed.revision);
    fs::remove_file(root.join(FILE_NAME)).unwrap();
    let recovered = reload(&authority);
    assert_eq!(recovered.settings, Settings::default());
    assert!(recovered.error.is_none());
    update(&authority, 90, 3).unwrap();
}
