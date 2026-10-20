use super::*;
use std::ffi::OsString;

#[test]
fn directory_selection_preserves_precedence_and_requires_absolute_requests() {
    assert_eq!(
        select_directory(
            Some(OsString::from("/override")),
            Some(OsString::from("/state")),
            Some(OsString::from("/home")),
        )
        .unwrap(),
        Path::new("/override")
    );
    assert_eq!(
        select_directory(None, Some(OsString::from("/state")), None).unwrap(),
        Path::new("/state/hammerr/recovery")
    );
    assert_eq!(
        select_directory(
            None,
            Some(OsString::from("relative-state")),
            Some(OsString::from("/home")),
        )
        .unwrap(),
        Path::new("/home/.local/state/hammerr/recovery")
    );
    for directory in [
        select_directory(Some(OsString::from("relative")), None, None),
        select_directory(None, None, Some(OsString::from("relative"))),
        select_directory(None, None, None),
    ] {
        assert!(directory.is_err());
    }
}

#[cfg(unix)]
#[test]
fn selected_raw_override_state_home_and_home_are_rejected_without_fallback() {
    use std::os::unix::ffi::OsStringExt;

    let raw = OsString::from_vec(b"/raw-\xff".to_vec());
    for directory in [
        select_directory(
            Some(raw.clone()),
            Some(OsString::from("/valid-state")),
            Some(OsString::from("/valid-home")),
        ),
        select_directory(None, Some(raw.clone()), Some(OsString::from("/valid-home"))),
        select_directory(None, None, Some(raw)),
    ] {
        assert_eq!(
            directory.unwrap_err(),
            "Recovery directory must be valid UTF-8"
        );
    }
}

#[cfg(unix)]
#[test]
fn requested_utf8_storage_alias_keeps_its_name_and_raw_physical_binding() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};

    let directory = tempfile::tempdir().unwrap();
    let physical = directory
        .path()
        .join(OsString::from_vec(b"state-\xff".to_vec()));
    fs::create_dir(&physical).unwrap();
    let alias = directory.path().join("state-alias");
    symlink(&physical, &alias).unwrap();
    let requested = select_directory(Some(alias.clone().into_os_string()), None, None).unwrap();
    assert_eq!(requested, alias);
    let store = Store {
        directory: requested,
    };
    store.save_settings(&Settings::default()).unwrap();
    assert!(physical.join("settings.json").is_file());
    assert!(fs::symlink_metadata(&alias).unwrap().is_symlink());
    assert_eq!(store.load_settings().unwrap(), Settings::default());
}
