use super::*;
use std::ffi::OsString;
use std::os::unix::{ffi::OsStrExt, ffi::OsStringExt, fs::symlink};

fn session(directory: &Path) -> Session {
    let mut session = Session::new(Err("No assets required".into()));
    session.configure_recovery(Ok(directory.join("recovery")));
    session
}

fn edit(session: &mut Session, index: u32) {
    session
        .create_block(
            Actor::User,
            glam::DVec3::splat(f64::from(index) * 128.0),
            glam::DVec3::splat(f64::from(index) * 128.0 + 64.0),
            "test",
            false,
        )
        .unwrap();
}

fn finish(session: &mut Session, request: RecoveryRequest) -> Entry {
    session
        .finish_recovery_snapshot(Actor::System, &request, request.run())
        .unwrap()
}

fn capture(session: &mut Session) -> Entry {
    let request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
    finish(session, request)
}

#[test]
fn rejected_directory_keeps_reports_usable_and_blocks_previous_storage() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = session(directory.path());
    let settings = Settings {
        interval_seconds: 60,
        ..Settings::default()
    };
    session
        .set_recovery_settings(Actor::User, settings.clone(), None)
        .unwrap();
    let settings_path = directory.path().join("recovery/settings.json");
    let previous = std::fs::read(&settings_path).unwrap();
    let raw = directory
        .path()
        .join(OsString::from_vec(b"rejected-\xff".to_vec()));
    session.configure_recovery(Ok(raw.clone()));
    let status = session.recovery_status_snapshot().1;
    assert_eq!(status["available"], false);
    assert!(status["directory"].is_null());
    assert_eq!(
        status["storage_error"],
        "Recovery directory must be valid UTF-8"
    );
    assert_eq!(
        status["settings_error"],
        "Recovery directory must be valid UTF-8"
    );
    assert_eq!(session.recovery_settings(), settings);
    assert!(session.begin_recovery_snapshot(Kind::Manual).is_err());
    assert!(
        session
            .set_recovery_settings(Actor::User, Settings::default(), None)
            .is_err()
    );
    assert!(session.reload_recovery_settings(Actor::User, None).is_err());
    for report in [
        session.grid_preferences_state(),
        session.preview_preferences_state(),
        session.navigation_preferences_state(),
        session.keyboard_shortcuts_state(),
    ] {
        assert_eq!(report["ready"], false);
        assert!(report["path"].is_null());
        assert!(report["error"].as_str().unwrap().contains("valid UTF-8"));
        serde_json::to_vec(&report).unwrap();
    }
    let favorites = serde_json::to_value(session.material_favorite_status()).unwrap();
    assert_eq!(favorites["persistence"], "blocked");
    assert!(favorites["directory"].is_null());
    assert!(favorites["error"].as_str().unwrap().contains("valid UTF-8"));
    let recents = session.recent_maps();
    if std::env::var_os("HAMMERR_RECENT_MAPS_DIR").is_none() {
        assert_eq!(recents["persistence"], "blocked");
        assert!(recents["directory"].is_null());
        assert!(recents["error"].as_str().unwrap().contains("valid UTF-8"));
    }
    serde_json::to_vec(&recents).unwrap();
    assert_eq!(std::fs::read(&settings_path).unwrap(), previous);
    assert!(!raw.exists());
    session.configure_recovery(Ok(directory.path().join("recovery")));
    assert!(session.recovery_status()["storage_error"].is_null());
    assert!(session.recovery_status()["settings_error"].is_null());
    assert_eq!(session.recovery_settings(), settings);
}

#[test]
fn rejected_document_directory_preserves_application_settings_authority() {
    let directory = tempfile::tempdir().unwrap();
    let authority = Authority::configured(Ok(directory.path().join("settings")));
    let mut session =
        Session::with_recovery_settings(Err("No assets required".into()), authority.clone());
    session.configure_recovery(Ok(directory.path().join("snapshots")));
    session
        .set_recovery_settings(
            Actor::User,
            Settings {
                interval_seconds: 60,
                ..Settings::default()
            },
            None,
        )
        .unwrap();
    let settings = authority.snapshot();
    let raw = directory
        .path()
        .join(OsString::from_vec(b"rejected-\xff".to_vec()));
    session.configure_recovery(Ok(raw));
    assert_eq!(authority.snapshot(), settings);
    let status = session.recovery_status();
    assert_eq!(status["available"], false);
    assert!(status["directory"].is_null());
    assert!(status["settings_error"].is_null());
    assert_eq!(
        status["storage_error"],
        "Recovery directory must be valid UTF-8"
    );
}

#[test]
fn utf8_and_raw_physical_aliases_support_snapshot_save_list_and_restore() {
    for raw in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let physical = directory.path().join(if raw {
            OsString::from_vec(b"physical-\xff.vmf".to_vec())
        } else {
            OsString::from("physical.vmf")
        });
        let original = Document::empty().vmf().write(&WriteOptions::default());
        std::fs::write(&physical, original.as_bytes()).unwrap();
        let alias = directory.path().join("map-alias.vmf");
        symlink(physical.file_name().unwrap(), &alias).unwrap();
        let mut session = session(directory.path());
        session.open(Actor::User, &alias, false).unwrap();
        let key = session.recovery.document_key.clone();
        let source = if raw { &alias } else { &physical };
        assert_eq!(session.recovery.source_path.as_ref(), Some(source));
        if raw {
            let (pairs, remainder) = key
                .strip_prefix("map-bytes:")
                .unwrap()
                .as_bytes()
                .as_chunks::<2>();
            assert!(remainder.is_empty());
            let bytes: Vec<u8> = pairs
                .iter()
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect();
            assert_eq!(bytes, physical.as_os_str().as_bytes());
        } else {
            assert_eq!(key, format!("map:{}", physical.to_str().unwrap()));
        }
        edit(&mut session, 0);
        let manual = capture(&mut session);
        edit(&mut session, 1);
        session.recovery.last_attempt = Instant::now() - Duration::from_secs(121);
        let request = session.begin_due_recovery_snapshot().unwrap();
        let automatic = finish(&mut session, request);
        edit(&mut session, 2);
        let request = session.begin_exit_recovery_snapshot().unwrap().unwrap();
        let exit = finish(&mut session, request);
        assert_eq!(manual.kind, Kind::Manual);
        assert_eq!(automatic.kind, Kind::Autosave);
        assert_eq!(exit.kind, Kind::Autosave);
        session.save(Actor::User, None).unwrap();
        assert_eq!(session.path(), Some(alias.as_path()));
        assert_eq!(session.recovery.document_key, key);
        assert_eq!(
            std::fs::read_link(&alias).unwrap(),
            Path::new(physical.file_name().unwrap())
        );
        assert_eq!(
            session
                .file_claims
                .source
                .as_ref()
                .unwrap()
                .destination()
                .physical_path(),
            physical
        );
        let committed = std::fs::read_to_string(&physical).unwrap();
        assert_eq!(Vmf::parse(&committed).unwrap().world.solids.len(), 3);
        assert!(session.recovery_status()["error"].is_null());
        let entries = session.recovery_entries().unwrap().entries;
        assert_eq!(entries.len(), 4);
        let before_save = entries
            .iter()
            .find(|entry| entry.kind == Kind::BeforeSave)
            .unwrap();
        for entry in &entries {
            assert_eq!(entry.format, 1);
            assert_eq!(entry.source_path.as_ref(), Some(source));
            assert_eq!(entry.document_key, key);
            let (loaded, text) = session
                .recovery
                .store
                .as_ref()
                .unwrap()
                .read(&entry.id)
                .unwrap();
            assert_eq!(loaded, *entry);
            let expected = if entry.id == manual.id {
                1
            } else if entry.id == automatic.id {
                2
            } else if entry.id == exit.id {
                3
            } else {
                assert_eq!(entry.kind, Kind::BeforeSave);
                assert_eq!(text, original);
                0
            };
            assert_eq!(Vmf::parse(&text).unwrap().world.solids.len(), expected);
        }
        for (entry, solids) in [(&manual, 1), (&automatic, 2), (&exit, 3), (before_save, 0)] {
            session
                .restore_recovery_snapshot(Actor::User, &entry.id, true, None)
                .unwrap();
            assert!(session.path().is_none());
            assert!(session.document().is_dirty());
            assert_eq!(session.document().vmf().world.solids.len(), solids);
            assert_eq!(session.recovery.source_path.as_ref(), Some(source));
            assert_eq!(std::fs::read_to_string(&physical).unwrap(), committed);
        }
    }
}

#[test]
fn raw_physical_keys_separate_lossy_collisions_and_share_alias_retention() {
    let directory = tempfile::tempdir().unwrap();
    let original = Document::empty().vmf().write(&WriteOptions::default());
    let mut sessions = Vec::new();
    let mut targets = Vec::new();
    for (index, byte) in [0xfe, 0xff].into_iter().enumerate() {
        let mut name = b"physical-".to_vec();
        name.extend_from_slice(&[byte, b'.', b'v', b'm', b'f']);
        let target = directory.path().join(OsString::from_vec(name));
        std::fs::write(&target, original.as_bytes()).unwrap();
        let alias = directory.path().join(format!("alias-{index}.vmf"));
        symlink(&target, &alias).unwrap();
        let mut session = session(directory.path());
        session.open(Actor::User, &alias, false).unwrap();
        capture(&mut session);
        targets.push(target);
        sessions.push(session);
    }
    assert_eq!(targets[0].to_string_lossy(), targets[1].to_string_lossy());
    assert_ne!(
        sessions[0].recovery.document_key,
        sessions[1].recovery.document_key
    );
    let other_alias = directory.path().join("another-alias.vmf");
    symlink(&targets[0], &other_alias).unwrap();
    let mut alias_session = session(directory.path());
    alias_session
        .open(Actor::User, &other_alias, false)
        .unwrap();
    assert_eq!(
        alias_session.recovery.document_key,
        sessions[0].recovery.document_key
    );
    let retained = capture(&mut alias_session);
    let store = alias_session.recovery.store.as_ref().unwrap();
    store.prune(&retained.document_key, 1).unwrap();
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|entry| entry.id == retained.id));
    assert!(
        entries
            .iter()
            .any(|entry| entry.document_key == sessions[1].recovery.document_key)
    );
}

#[test]
fn unrepresentable_reopen_metadata_is_rejected_before_open_or_save_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join(OsString::from_vec(b"map-\xff.vmf".to_vec()));
    let original = Document::empty().vmf().write(&WriteOptions::default());
    std::fs::write(&path, original.as_bytes()).unwrap();
    let mut session = session(directory.path());
    edit(&mut session, 0);
    let before = session.document().vmf().clone();
    let generation = session.document_generation();
    let revision = session.document().revision();
    let key = session.recovery.document_key.clone();
    assert!(
        session
            .open(Actor::User, &path, true)
            .unwrap_err()
            .contains("use a UTF-8 alias")
    );
    assert!(
        session
            .save(Actor::User, Some(&path))
            .unwrap_err()
            .contains("use a UTF-8 alias")
    );
    assert_eq!(session.document().vmf(), &before);
    assert_eq!(session.document_generation(), generation);
    assert_eq!(session.document().revision(), revision);
    assert_eq!(session.recovery.document_key, key);
    assert!(session.document().is_dirty());
    assert!(session.path().is_none());
    assert!(session.recovery_entries().unwrap().entries.is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    let alias = directory.path().join("representable.vmf");
    symlink(&path, &alias).unwrap();
    session.save(Actor::User, Some(&alias)).unwrap();
    assert_eq!(session.path(), Some(alias.as_path()));
    assert_eq!(session.recovery.source_path.as_ref(), Some(&alias));
}
