use super::*;

fn snapshot(authority: &Authority) -> String {
    authority.report()["snapshot"].as_str().unwrap().into()
}
fn color() -> Input {
    Input {
        space: Space::Rgb1,
        channels: [0.12345678901234566, 0.25000000000000006, 0.9876543210987654],
    }
}

#[test]
fn precision_restart_explicit_replace_delete_and_limits() {
    let directory = tempfile::tempdir().unwrap();
    let authority = Authority::configured(Ok(directory.path().into()));
    assert_eq!(authority.report()["ready"], true);
    authority
        .change(
            "save",
            &snapshot(&authority),
            Some("Precise"),
            Some(color()),
        )
        .unwrap();
    let restarted = Authority::configured(Ok(directory.path().into()));
    let loaded = restarted
        .change("use", &snapshot(&restarted), Some("Precise"), None)
        .unwrap();
    assert_eq!(loaded["color"]["rgb1"], json!(color().channels));
    assert!(
        authority
            .change(
                "save",
                &snapshot(&authority),
                Some("Precise"),
                Some(color())
            )
            .is_err()
    );
    for index in 1..MAX_ENTRIES {
        authority
            .change(
                "save",
                &snapshot(&authority),
                Some(&format!("Color {index:02}")),
                Some(color()),
            )
            .unwrap();
    }
    let bytes = std::fs::read(directory.path().join(FILE_NAME)).unwrap();
    assert!(
        authority
            .change(
                "save",
                &snapshot(&authority),
                Some("Overflow"),
                Some(color())
            )
            .is_err()
    );
    assert_eq!(
        std::fs::read(directory.path().join(FILE_NAME)).unwrap(),
        bytes
    );
    authority
        .change(
            "replace",
            &snapshot(&authority),
            Some("Precise"),
            Some(Input {
                space: Space::Hsv,
                channels: [120., 100., 100.],
            }),
        )
        .unwrap();
    authority
        .change("delete", &snapshot(&authority), Some("Precise"), None)
        .unwrap();
    assert_eq!(
        authority.report()["entries"].as_array().unwrap().len(),
        MAX_ENTRIES - 1
    );
}

#[test]
fn independent_writers_conflict_reload_and_corrupt_reads_preserve_last_good_colors() {
    let directory = tempfile::tempdir().unwrap();
    let first = Authority::configured(Ok(directory.path().into()));
    let second = Authority::configured(Ok(directory.path().into()));
    first
        .change("save", &snapshot(&first), Some("First"), Some(color()))
        .unwrap();
    assert!(
        second
            .change("save", &snapshot(&second), Some("Second"), Some(color()))
            .unwrap_err()
            .contains("Reload")
    );
    assert_eq!(second.report()["ready"], false);
    assert!(second.report()["entries"].as_array().unwrap().is_empty());
    second
        .change("reload", &snapshot(&second), None, None)
        .unwrap();
    second
        .change("save", &snapshot(&second), Some("Second"), Some(color()))
        .unwrap();
    let path = directory.path().join(FILE_NAME);
    let valid = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"broken").unwrap();
    assert!(
        second
            .change("reload", &snapshot(&second), None, None)
            .is_err()
    );
    assert_eq!(second.report()["ready"], false);
    assert_eq!(second.report()["entries"].as_array().unwrap().len(), 2);
    assert_eq!(second.report()["write_committed"], false);
    assert!(second.lock().unverified_write.is_none());
    assert!(
        second
            .change("use", &snapshot(&second), Some("First"), None)
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"broken");
    std::fs::write(&path, valid).unwrap();
    second
        .change("reload", &snapshot(&second), None, None)
        .unwrap();
    assert_eq!(second.report()["ready"], true);
}

#[test]
fn invalid_names_colors_files_and_unconfigured_storage_are_unavailable() {
    let unavailable = Authority::default();
    assert_eq!(unavailable.report()["ready"], false);
    assert!(unavailable.report()["error"].is_string());
    assert!(
        unavailable
            .change("save", &snapshot(&unavailable), Some("Name"), Some(color()))
            .is_err()
    );
    let directory = tempfile::tempdir().unwrap();
    let authority = Authority::configured(Ok(directory.path().into()));
    for name in [
        "",
        " leading",
        "trailing ",
        "new\nline",
        &"x".repeat(MAX_NAME_CHARACTERS + 1),
    ] {
        assert!(
            authority
                .change("save", &snapshot(&authority), Some(name), Some(color()))
                .is_err()
        );
    }
    assert!(
        authority
            .change(
                "save",
                &snapshot(&authority),
                Some("Invalid"),
                Some(Input {
                    space: Space::Rgb1,
                    channels: [-1., 0., 0.]
                })
            )
            .is_err()
    );
    assert!(!directory.path().join(FILE_NAME).exists());
    std::fs::write(
        directory.path().join(FILE_NAME),
        vec![b' '; MAX_FILE_BYTES as usize + 1],
    )
    .unwrap();
    let invalid = Authority::configured(Ok(directory.path().into()));
    assert_eq!(invalid.report()["ready"], false);
    assert!(invalid.report()["entries"].as_array().unwrap().is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn directory_replacement_and_participating_writer_lock_require_reload() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("palette");
    let authority = Authority::configured(Ok(directory.clone()));
    let guard = crate::preference_file::WriteLock::acquire(
        &directory,
        ".color-palette.lock",
        "Color palette",
        "Reload palette",
    )
    .unwrap();
    assert!(
        authority
            .change("save", &snapshot(&authority), Some("Busy"), Some(color()))
            .unwrap_err()
            .contains("busy")
    );
    assert_eq!(authority.report()["ready"], false);
    assert!(!directory.join(FILE_NAME).exists());
    drop(guard);
    authority
        .change("reload", &snapshot(&authority), None, None)
        .unwrap();
    authority
        .change("save", &snapshot(&authority), Some("First"), Some(color()))
        .unwrap();
    let old = root.path().join("previous");
    std::fs::rename(&directory, &old).unwrap();
    std::fs::create_dir(&directory).unwrap();
    let bytes = std::fs::read(old.join(FILE_NAME)).unwrap();
    std::fs::write(directory.join(FILE_NAME), &bytes).unwrap();
    assert!(
        authority
            .change("delete", &snapshot(&authority), Some("First"), None)
            .is_err()
    );
    assert_eq!(std::fs::read(directory.join(FILE_NAME)).unwrap(), bytes);
    authority
        .change("reload", &snapshot(&authority), None, None)
        .unwrap();
    authority
        .change("delete", &snapshot(&authority), Some("First"), None)
        .unwrap();
    let path = directory.join(FILE_NAME);
    let target = root.path().join("target.json");
    std::fs::rename(&path, &target).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(
        authority
            .change("reload", &snapshot(&authority), None, None)
            .is_err()
    );
    assert_eq!(authority.report()["ready"], false);
    assert!(
        std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn unverified_publication_warning_survives_failed_reload_until_verified_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let authority = Authority::configured(Ok(directory.path().into()));
    let entries = BTreeMap::from([("Committed".into(), color().channels)]);
    let warning = "Color palette was written but could not verify saved colors; Reload palette: synthetic verification failure";
    let error = authority
        .lock()
        .accept(
            entries.clone(),
            Publication {
                source: Err(warning.into()),
            },
        )
        .unwrap_err();
    assert_eq!(error, warning);
    let committed = authority.report();
    assert_eq!(committed["write_committed"], true);
    assert_eq!(committed["ready"], false);
    assert_eq!(committed["error"], warning);
    assert_eq!(committed["entries"][0]["rgb1"], json!(color().channels));
    assert!(authority.lock().source.is_none());
    let path = directory.path().join(FILE_NAME);
    std::fs::write(&path, b"broken").unwrap();
    for _ in 0..2 {
        let error = authority
            .change("reload", &snapshot(&authority), None, None)
            .unwrap_err();
        assert!(error.contains(warning));
        assert!(error.contains("could not load"));
        let state = authority.report();
        assert_eq!(state["write_committed"], true);
        assert_eq!(state["ready"], false);
        assert_eq!(state["entries"], committed["entries"]);
        assert_eq!(state["error"], error);
        assert!(
            authority
                .change("use", &snapshot(&authority), Some("Committed"), None)
                .unwrap_err()
                .contains(warning)
        );
    }
    std::fs::remove_file(&path).unwrap();
    let reloaded = authority
        .change("reload", &snapshot(&authority), None, None)
        .unwrap();
    assert_eq!(reloaded["ready"], true);
    assert_eq!(reloaded["write_committed"], false);
    assert!(reloaded["error"].is_null());
    assert!(reloaded["entries"].as_array().unwrap().is_empty());
    authority
        .lock()
        .accept(
            entries.clone(),
            Publication {
                source: Err(warning.into()),
            },
        )
        .unwrap_err();
    let store = Store {
        directory: directory.path().into(),
    };
    let source = store.target().observe().unwrap();
    let publication = store.save(&entries, &source).unwrap();
    let verified = authority.lock().accept(entries, publication).unwrap();
    assert_eq!(verified["ready"], true);
    assert_eq!(verified["write_committed"], true);
    assert!(verified["error"].is_null());
    assert!(authority.lock().unverified_write.is_none());
}

#[test]
fn rename_preserves_full_palette_color_and_rejects_invalid_missing_duplicate_and_stale_names() {
    let directory = tempfile::tempdir().unwrap();
    let authority = Authority::configured(Ok(directory.path().into()));
    for index in 0..MAX_ENTRIES {
        authority
            .change(
                "save",
                &snapshot(&authority),
                Some(&format!("Saved {index:02}")),
                Some(color()),
            )
            .unwrap();
    }
    let serial = authority.lock().serial;
    let old_snapshot = snapshot(&authority);
    let renamed = authority
        .rename(&old_snapshot, "Saved 00", "A precise color")
        .unwrap();
    assert_eq!(renamed["ready"], true);
    assert_eq!(renamed["write_committed"], true);
    assert_eq!(authority.lock().serial, serial + 1);
    assert_eq!(authority.lock().entries.len(), MAX_ENTRIES);
    assert!(!authority.lock().entries.contains_key("Saved 00"));
    assert_eq!(
        authority.lock().entries["A precise color"].map(f64::to_bits),
        color().channels.map(f64::to_bits)
    );
    let store = Store {
        directory: directory.path().into(),
    };
    let source = store.target().observe().unwrap().source;
    for (old, new) in [
        ("A precise color", "Saved 01"),
        ("Missing", "Unused"),
        ("A precise color", ""),
        ("A precise color", " surrounding "),
        ("A precise color", "new\nline"),
        ("A precise color", "A precise color"),
    ] {
        assert!(authority.rename(&snapshot(&authority), old, new).is_err());
        assert_eq!(store.target().observe().unwrap().source, source);
    }
    assert!(
        authority
            .rename(&old_snapshot, "A precise color", "Stale")
            .is_err()
    );
    assert_eq!(store.target().observe().unwrap().source, source);
    let restarted = Authority::configured(Ok(directory.path().into()));
    assert_eq!(
        restarted.lock().entries["A precise color"].map(f64::to_bits),
        color().channels.map(f64::to_bits)
    );
    authority
        .rename(&snapshot(&authority), "A precise color", "First writer")
        .unwrap();
    let source = store.target().observe().unwrap().source;
    assert!(
        restarted
            .rename(&snapshot(&restarted), "A precise color", "Other writer")
            .is_err()
    );
    assert_eq!(restarted.report()["ready"], false);
    assert_eq!(store.target().observe().unwrap().source, source);
    restarted
        .change("reload", &snapshot(&restarted), None, None)
        .unwrap();
    restarted
        .rename(
            &snapshot(&restarted),
            "First writer",
            "Choose a saved color",
        )
        .unwrap();
    assert_eq!(
        restarted.lock().entries["Choose a saved color"].map(f64::to_bits),
        color().channels.map(f64::to_bits)
    );
    std::fs::write(directory.path().join(FILE_NAME), b"invalid").unwrap();
    assert!(
        restarted
            .change("reload", &snapshot(&restarted), None, None)
            .is_err()
    );
    assert!(
        restarted
            .rename(&snapshot(&restarted), "Choose a saved color", "Unavailable")
            .is_err()
    );
    assert_eq!(
        std::fs::read(directory.path().join(FILE_NAME)).unwrap(),
        b"invalid"
    );
}
