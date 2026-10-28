use super::*;
use std::fs;

fn store(directory: &tempfile::TempDir) -> Store {
    Store {
        directory: directory.path().join("preferences"),
    }
}

#[test]
fn all_pivot_modes_roundtrip_with_exact_finite_world_coordinates() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    assert_eq!(store.load().unwrap(), None);
    assert!(!store.directory.exists());
    for pivot in [
        Pivot::SelectionCenter {},
        Pivot::WorldOrigin {},
        Pivot::Custom {
            point: [1.125, -2.375, 3.0625],
        },
        Pivot::Custom {
            point: [-f64::MAX, f64::from_bits(1), f64::MAX],
        },
    ] {
        store.save(pivot, false).unwrap();
        assert_eq!(store.load().unwrap(), Some(pivot));
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 2);
        assert!(value["pivot"].get("target").is_none());
    }
    let bytes = fs::read(store.path()).unwrap();
    for coordinate in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            store
                .save(
                    Pivot::Custom {
                        point: [coordinate, 0.0, 0.0]
                    },
                    false
                )
                .is_err()
        );
        assert_eq!(fs::read(store.path()).unwrap(), bytes);
    }
}

#[test]
fn corrupt_future_unknown_missing_and_oversized_files_require_explicit_reset() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    fs::create_dir_all(&store.directory).unwrap();
    for bytes in [
        b"{bad".to_vec(),
        br#"{"version":2,"pivot":{"mode":"selection_center"}}"#.to_vec(),
        br#"{"version":1,"pivot":{"mode":"unknown"}}"#.to_vec(),
        br#"{"version":1,"pivot":{"mode":"custom"}}"#.to_vec(),
        br#"{"version":1,"pivot":{"mode":"custom","point":[1e999,0,0]}}"#.to_vec(),
        br#"{"version":1,"pivot":{"mode":"custom","point":[0,0,0],"target":"solid:1"}}"#.to_vec(),
        br#"{"version":1,"pivot":{"mode":"world_origin","point":[0,0,0]}}"#.to_vec(),
        br#"{"version":1,"pivot":{"mode":"custom","point":[0,0,0],"point":[1,2,3]}}"#.to_vec(),
        br#"{"version":1,"pivot":{"mode":"selection_center"},"camera":{}}"#.to_vec(),
        vec![b' '; MAX_FILE_BYTES as usize + 1],
    ] {
        fs::write(store.path(), &bytes).unwrap();
        assert!(store.load().is_err(), "{bytes:?}");
        assert!(store.save(Pivot::default(), false).is_err());
        assert_eq!(fs::read(store.path()).unwrap(), bytes);
        store.save(Pivot::default(), true).unwrap();
        assert_eq!(store.load().unwrap(), Some(Pivot::SelectionCenter {}));
    }
}
