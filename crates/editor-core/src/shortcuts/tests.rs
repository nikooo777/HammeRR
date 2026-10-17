use std::fs;

use super::*;

#[test]
fn factory_aliases_and_exact_modifiers_resolve_without_reserved_key_collisions() {
    let bindings = Bindings::default().validated().unwrap();
    for (key, control, shift, expected) in [
        ("Z", true, false, Some(Command::Undo)),
        ("Z", true, true, Some(Command::Redo)),
        ("Y", true, false, Some(Command::Redo)),
        ("Delete", false, false, Some(Command::DeleteSelection)),
        ("Backspace", false, false, Some(Command::DeleteSelection)),
        ("D", true, false, Some(Command::DuplicateSelection)),
        ("F", false, false, Some(Command::FrameSelection)),
        ("F", false, true, None),
        ("Z", false, false, None),
    ] {
        assert_eq!(
            bindings.resolve(&Chord {
                key: key.into(),
                control,
                shift
            }),
            expected
        );
    }
    for (key, control) in [
        ("W", false),
        ("A", false),
        ("S", false),
        ("D", false),
        ("Q", false),
        ("E", false),
        ("N", true),
        ("O", true),
        ("S", true),
        ("C", true),
        ("X", true),
        ("V", true),
    ] {
        for shift in [false, true] {
            let mut invalid = bindings.clone();
            invalid.undo[0] = chord(key, control, shift);
            assert!(
                invalid.validated().is_err(),
                "{key} control={control} shift={shift}"
            );
        }
    }
}

#[test]
fn rebinding_unbinding_and_atomic_swaps_reject_duplicate_and_unsupported_chords() {
    let bindings = Bindings {
        undo: [chord("f6", false, true), None],
        duplicate_selection: [None, None],
        ..Default::default()
    };
    let bindings = bindings.validated().unwrap();
    assert_eq!(bindings.undo[0].as_ref().unwrap().key, "F6");
    assert_eq!(bindings.label(Command::DuplicateSelection), "Unbound");
    assert_eq!(
        bindings.resolve(&Chord {
            key: "Z".into(),
            control: true,
            shift: false
        }),
        None
    );
    let mut duplicate = bindings.clone();
    duplicate.redo[0] = bindings.undo[0].clone();
    assert!(duplicate.validated().is_err());
    let mut same_command = bindings.clone();
    same_command.undo[1] = same_command.undo[0].clone();
    assert!(same_command.validated().is_err());
    let mut swapped = Bindings::default();
    std::mem::swap(&mut swapped.undo, &mut swapped.redo);
    assert!(swapped.validated().is_ok());
    for key in [
        "Escape", "Tab", " ", "Up", "PageUp", "Shift", "F0", "F13", "Ctrl+Z", "é", "",
    ] {
        let mut invalid = bindings.clone();
        invalid.undo[0] = chord(key, false, false);
        assert!(invalid.validated().is_err(), "{key}");
    }
    let mut invalid = serde_json::to_value(&bindings).unwrap();
    invalid["undo"][0]["alt"] = serde_json::json!(false);
    assert!(serde_json::from_value::<Bindings>(invalid).is_err());
}

#[test]
fn storage_is_versioned_bounded_and_preserves_invalid_or_nonregular_sources() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store {
        directory: directory.path().to_owned(),
    };
    let expected = Bindings::default();
    assert_eq!(store.parse(&store.read_source().unwrap()).unwrap(), None);
    store.save(expected.clone()).unwrap();
    assert_eq!(
        store.parse(&store.read_source().unwrap()).unwrap(),
        Some(expected)
    );
    let valid = serde_json::json!({"version":1,"bindings":Bindings::default()});
    let mut future = valid.clone();
    future["version"] = serde_json::json!(2);
    let mut unknown = valid.clone();
    unknown["extra"] = serde_json::json!(true);
    let mut incomplete = valid.clone();
    incomplete["bindings"]
        .as_object_mut()
        .unwrap()
        .remove("undo");
    let mut extra_slot = valid.clone();
    extra_slot["bindings"]["undo"] = serde_json::json!([null, null, null]);
    for bytes in [
        b"not json".to_vec(),
        serde_json::to_vec(&future).unwrap(),
        serde_json::to_vec(&unknown).unwrap(),
        serde_json::to_vec(&incomplete).unwrap(),
        serde_json::to_vec(&extra_slot).unwrap(),
        vec![b' '; MAX_FILE_BYTES as usize + 1],
    ] {
        fs::write(store.path(), &bytes).unwrap();
        assert!(store.parse(&store.read_source().unwrap()).is_err());
        assert_eq!(fs::read(store.path()).unwrap(), bytes);
    }
    #[cfg(unix)]
    {
        fs::remove_file(store.path()).unwrap();
        let target = directory.path().join("outside.json");
        fs::write(&target, b"preserve").unwrap();
        std::os::unix::fs::symlink(&target, store.path()).unwrap();
        assert!(store.read_source().is_err());
        assert!(store.save(Bindings::default()).is_err());
        assert_eq!(fs::read(target).unwrap(), b"preserve");
    }
}
