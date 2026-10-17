use std::fs;

use super::*;

fn store(directory: &tempfile::TempDir) -> Store {
    Store {
        directory: directory.path().join("preferences"),
    }
}

#[test]
fn exact_custom_sizes_and_existing_boundaries_roundtrip_without_transient_settings() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    assert_eq!(store.load().unwrap(), None);
    assert!(!store.directory.exists());
    for grid_size in [crate::spatial::MIN_GRID, 17.375, crate::spatial::MAX_GRID] {
        let grid = Grid {
            grid_size,
            snap: false,
            grid_visible: false,
        };
        store.save(grid, false).unwrap();
        assert_eq!(store.load().unwrap(), Some(grid));
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(value["grid"].as_object().unwrap().len(), 3);
        assert!(value["grid"].get("elevation").is_none());
        assert!(value["grid"].get("draw_block").is_none());
    }
    let bytes = fs::read(store.path()).unwrap();
    for grid_size in [0.0, 0.124, 4096.001, f64::INFINITY, f64::NAN] {
        assert!(
            store
                .save(
                    Grid {
                        grid_size,
                        ..Grid::default()
                    },
                    false
                )
                .is_err()
        );
        assert_eq!(fs::read(store.path()).unwrap(), bytes);
    }
}

#[test]
fn malformed_future_unknown_and_oversized_files_are_preserved_until_explicit_reset() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    fs::create_dir_all(&store.directory).unwrap();
    for bytes in [
        b"{bad".to_vec(),
        br#"{"version":2,"grid":{"grid_size":16,"snap":true,"grid_visible":true}}"#.to_vec(),
        br#"{"version":1,"grid":{"grid_size":0,"snap":true,"grid_visible":true}}"#.to_vec(),
        br#"{"version":1,"grid":{"grid_size":16,"snap":true,"grid_visible":true,"elevation":32}}"#
            .to_vec(),
        br#"{"version":1,"grid":{"grid_size":16,"grid_size":32,"snap":true,"grid_visible":true}}"#
            .to_vec(),
        vec![b' '; MAX_FILE_BYTES as usize + 1],
    ] {
        fs::write(store.path(), &bytes).unwrap();
        assert!(store.load().is_err());
        assert!(store.save(Grid::default(), false).is_err());
        assert_eq!(fs::read(store.path()).unwrap(), bytes);
        store.save(Grid::default(), true).unwrap();
        assert_eq!(store.load().unwrap(), Some(Grid::default()));
    }
}

#[test]
fn nonregular_and_unwritable_destinations_fail_without_a_persistence_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    fs::create_dir_all(store.path()).unwrap();
    assert!(store.load().is_err());
    assert!(store.save(Grid::default(), true).is_err());
    assert!(store.path().is_dir());
    fs::remove_dir(store.path()).unwrap();
    fs::remove_dir(&store.directory).unwrap();
    fs::write(&store.directory, b"keep").unwrap();
    assert!(store.save(Grid::default(), true).is_err());
    assert_eq!(fs::read(&store.directory).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn symlinked_preferences_are_inspectable_errors_and_reset_does_not_follow_them() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    fs::create_dir_all(&store.directory).unwrap();
    let target = directory.path().join("outside.json");
    fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, store.path()).unwrap();
    assert!(store.load().is_err());
    assert!(store.save(Grid::default(), false).is_err());
    assert!(store.save(Grid::default(), true).is_err());
    assert_eq!(fs::read(target).unwrap(), b"keep");
}
