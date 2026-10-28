use std::fs;

use super::*;

#[test]
fn speed_limits_roundtrip_and_invalid_values_never_replace_valid_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store {
        directory: directory.path().join("preferences"),
    };
    assert_eq!(store.load().unwrap(), None);
    assert!(!store.directory.exists());
    for fly_speed in [MIN_FLY_SPEED, DEFAULT_FLY_SPEED, 987.625, MAX_FLY_SPEED] {
        let navigation = Navigation { fly_speed };
        store.save(navigation, false).unwrap();
        assert_eq!(store.load().unwrap(), Some(navigation));
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"version":1,"navigation":{"fly_speed":fly_speed}})
        );
    }
    let bytes = fs::read(store.path()).unwrap();
    for fly_speed in [
        0.0,
        24.999,
        20_001.0,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ] {
        for reset in [false, true] {
            assert!(store.save(Navigation { fly_speed }, reset).is_err());
            assert_eq!(fs::read(store.path()).unwrap(), bytes);
        }
    }
}

#[test]
fn invalid_schema_and_speed_files_are_preserved_until_explicit_reset() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store {
        directory: directory.path().into(),
    };
    for bytes in [
        b"broken".to_vec(),
        br#"{"version":2,"navigation":{"fly_speed":600}}"#.to_vec(),
        br#"{"version":1,"navigation":{"fly_speed":24}}"#.to_vec(),
        br#"{"version":1,"navigation":{"fly_speed":20001}}"#.to_vec(),
        br#"{"version":1,"navigation":{"fly_speed":1e100}}"#.to_vec(),
        br#"{"version":1,"navigation":{"fly_speed":"600"}}"#.to_vec(),
        br#"{"version":1,"navigation":{"fly_speed":600,"fly_speed":700}}"#.to_vec(),
        br#"{"version":1,"navigation":{"fly_speed":600,"sensitivity":1}}"#.to_vec(),
        br#"{"version":1,"navigation":{}}"#.to_vec(),
        br#"{"version":1,"navigation":{"fly_speed":600},"camera":{}}"#.to_vec(),
        vec![b' '; MAX_FILE_BYTES as usize + 1],
    ] {
        fs::write(store.path(), &bytes).unwrap();
        assert!(store.load().is_err());
        assert!(store.save(Navigation::default(), false).is_err());
        assert_eq!(fs::read(store.path()).unwrap(), bytes);
        store.save(Navigation::default(), true).unwrap();
        assert_eq!(store.load().unwrap(), Some(Navigation::default()));
    }
}
