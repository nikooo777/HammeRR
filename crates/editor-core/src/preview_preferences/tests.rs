use super::*;

fn store(directory: &tempfile::TempDir) -> Store {
    Store {
        directory: directory.path().join("preferences"),
    }
}

#[test]
fn all_boolean_combinations_roundtrip_with_only_the_five_supported_fields() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    assert_eq!(store.load().unwrap(), None);
    assert!(!store.directory.exists());
    for bits in 0..32 {
        let preview = Preview {
            selection_outlines: bits & 1 != 0,
            entity_images: bits & 2 != 0,
            entity_model_guides: bits & 4 != 0,
            rope_preview: bits & 8 != 0,
            sky_preview: bits & 16 != 0,
        };
        store.save(preview, false).unwrap();
        assert_eq!(store.load().unwrap(), Some(preview));
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(value, serde_json::json!({"version": 1, "preview": preview}));
        assert_eq!(value["preview"].as_object().unwrap().len(), 5);
    }
}

#[test]
fn malformed_future_unknown_incomplete_and_oversized_files_require_explicit_reset() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    fs::create_dir_all(&store.directory).unwrap();
    let valid = serde_json::json!({"version":1,"preview":Preview::default()});
    let mut future = valid.clone();
    future["version"] = serde_json::json!(2);
    let mut unknown = valid.clone();
    unknown["preview"]["sky_exposure_ev"] = serde_json::json!(2);
    let mut incomplete = valid.clone();
    incomplete["preview"]
        .as_object_mut()
        .unwrap()
        .remove("entity_images");
    let mut wrong_type = valid.clone();
    wrong_type["preview"]["rope_preview"] = serde_json::json!("true");
    for bytes in [
        b"{bad".to_vec(),
        serde_json::to_vec(&future).unwrap(),
        serde_json::to_vec(&unknown).unwrap(),
        serde_json::to_vec(&incomplete).unwrap(),
        serde_json::to_vec(&wrong_type).unwrap(),
        br#"{"version":1,"preview":{"selection_outlines":true,"selection_outlines":false,"entity_images":true,"entity_model_guides":true,"rope_preview":false,"sky_preview":false}}"#.to_vec(),
        vec![b' '; MAX_FILE_BYTES as usize + 1],
    ] {
        fs::write(store.path(), &bytes).unwrap();
        assert!(store.load().is_err());
        assert!(store.save(Preview::default(), false).is_err());
        assert_eq!(fs::read(store.path()).unwrap(), bytes);
        store.save(Preview::default(), true).unwrap();
        assert_eq!(store.load().unwrap(), Some(Preview::default()));
    }
}

#[test]
fn nonregular_and_unwritable_destinations_preserve_their_contents() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    fs::create_dir_all(store.path()).unwrap();
    assert!(store.load().is_err());
    assert!(store.save(Preview::default(), true).is_err());
    assert!(store.path().is_dir());
    fs::remove_dir(store.path()).unwrap();
    fs::remove_dir(&store.directory).unwrap();
    fs::write(&store.directory, b"keep").unwrap();
    assert!(store.save(Preview::default(), true).is_err());
    assert_eq!(fs::read(&store.directory).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn symlinked_defaults_are_errors_and_reset_never_writes_the_target() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    fs::create_dir_all(&store.directory).unwrap();
    let target = directory.path().join("outside.json");
    fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, store.path()).unwrap();
    assert!(store.load().is_err());
    assert!(store.save(Preview::default(), false).is_err());
    assert!(store.save(Preview::default(), true).is_err());
    assert_eq!(fs::read(target).unwrap(), b"keep");
}
