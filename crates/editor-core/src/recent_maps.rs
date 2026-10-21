use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod authority;
pub use authority::{Authority, Snapshot};

pub(crate) use crate::preference_file::ObservedSource;
#[cfg(test)]
use crate::preference_file::WriteLock;
use crate::preference_file::{CoordinatedFile, Publication, SaveError, Source};

pub const MAX_RECENT_MAPS: usize = 20;
pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_FILE_BYTES: u64 = 192 * 1024;
pub const FILE_NAME: &str = "recent-maps.json";
const LOCK_FILE_NAME: &str = ".recent-maps.lock";
pub const MAX_ERROR_BYTES: usize = 512;
const FORMAT_VERSION: u32 = 1;

pub use crate::build_settings::BuildSettings;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub path: PathBuf,
    pub game_directory: Option<PathBuf>,
    pub build_settings: BuildSettings,
}

impl Entry {
    fn validate(&self) -> Result<(), String> {
        validate_path(&self.path)?;
        if let Some(path) = &self.game_directory {
            validate_path(path)?;
        }
        self.build_settings.validate()
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u32,
    entries: Vec<Entry>,
}

pub struct Store {
    pub directory: PathBuf,
}

pub fn directory(recovery_directory: Result<PathBuf, String>) -> Result<PathBuf, String> {
    match std::env::var_os("HAMMERR_RECENT_MAPS_DIR") {
        Some(directory) => {
            let path = PathBuf::from(directory);
            validate_path(&path)?;
            Ok(path)
        }
        None => recovery_directory,
    }
}

fn validate_path(path: &Path) -> Result<(), String> {
    let text = path
        .to_str()
        .ok_or("Recent map paths must be valid UTF-8")?;
    if !path.is_absolute() || text.len() > MAX_PATH_BYTES || text.chars().any(char::is_control) {
        return Err(format!(
            "Recent map paths must be absolute, contain no control characters and fit in {MAX_PATH_BYTES} bytes"
        ));
    }
    Ok(())
}

pub fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    let path = fs::canonicalize(path).map_err(|error| format!("{}: {error}", path.display()))?;
    validate_path(&path)?;
    Ok(path)
}

impl Store {
    fn path(&self) -> PathBuf {
        self.directory.join(FILE_NAME)
    }

    fn target(&self) -> CoordinatedFile<'_> {
        CoordinatedFile {
            directory: &self.directory,
            file_name: FILE_NAME,
            lock_name: LOCK_FILE_NAME,
            label: "Recent maps",
            reload: "Reload saved list",
            saved_description: "saved list",
            max_bytes: MAX_FILE_BYTES,
        }
    }

    pub fn load(&self) -> Result<Vec<Entry>, String> {
        self.load_snapshot().map(|(entries, _)| entries)
    }

    pub(crate) fn load_snapshot(&self) -> Result<(Vec<Entry>, ObservedSource), String> {
        let source = self.target().observe()?;
        let path = self.path();
        let Source::File { bytes, length, .. } = &source.source else {
            return Ok((Vec::new(), source));
        };
        if *length > MAX_FILE_BYTES {
            return Err(format!(
                "{} must be a regular file of at most {MAX_FILE_BYTES} bytes",
                path.display()
            ));
        }
        let preferences: Preferences = serde_json::from_slice(bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if preferences.version != FORMAT_VERSION {
            return Err(format!(
                "{}: unsupported recent maps version {}",
                path.display(),
                preferences.version
            ));
        }
        validate_entries(&preferences.entries)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Ok((preferences.entries, source))
    }

    pub fn save(&self, entries: &[Entry]) -> Result<(), String> {
        let (_, source) = self.load_snapshot()?;
        self.save_checked(entries, &source)
            .map_err(|error| error.message)?
            .source
            .map(|_| ())
    }

    pub(crate) fn save_checked(
        &self,
        entries: &[Entry],
        expected: &ObservedSource,
    ) -> Result<Publication, SaveError> {
        self.save_checked_with(entries, expected, crate::recovery::atomic_write)
    }

    fn save_checked_with(
        &self,
        entries: &[Entry],
        expected: &ObservedSource,
        publish: impl FnOnce(&Path, &[u8]) -> Result<(), String>,
    ) -> Result<Publication, SaveError> {
        validate_entries(entries).map_err(SaveError::failure)?;
        let preferences = Preferences {
            version: FORMAT_VERSION,
            entries: entries.to_vec(),
        };
        let data = serde_json::to_vec_pretty(&preferences)
            .map_err(|error| SaveError::failure(error.to_string()))?;
        if data.len() as u64 > MAX_FILE_BYTES {
            return Err(SaveError::failure(format!(
                "Recent maps must fit in {MAX_FILE_BYTES} bytes"
            )));
        }
        self.target().publish_with(&data, expected, publish)
    }
}

fn validate_entries(entries: &[Entry]) -> Result<(), String> {
    if entries.len() > MAX_RECENT_MAPS {
        return Err(format!("Keep at most {MAX_RECENT_MAPS} recent maps"));
    }
    let mut paths = BTreeSet::new();
    for entry in entries {
        entry.validate()?;
        if !paths.insert(&entry.path) {
            return Err("Recent maps contain a duplicate path".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: PathBuf) -> Entry {
        Entry {
            path,
            game_directory: None,
            build_settings: BuildSettings::from_options(compiler::Options::default()),
        }
    }

    #[test]
    fn storage_roundtrips_order_without_requiring_maps_to_exist() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().join("state"),
        };
        assert!(store.load().unwrap().is_empty());
        assert!(!store.directory.exists());
        let entries = vec![
            entry(directory.path().join("z.vmf")),
            entry(directory.path().join("a.vmf")),
        ];
        store.save(&entries).unwrap();
        assert_eq!(store.load().unwrap(), entries);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn participating_writers_lock_a_stable_inode_and_refuse_stale_snapshots() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().into(),
        };
        let (_, source) = store.load_snapshot().unwrap();
        let guard = WriteLock::acquire(
            &store.directory,
            LOCK_FILE_NAME,
            "Recent maps",
            "Reload saved list",
        )
        .unwrap();
        let lock_path = store.directory.join(LOCK_FILE_NAME);
        let error = store.save_checked(&[], &source).unwrap_err();
        assert!(!error.conflict && error.message.contains("busy"));
        assert!(!store.path().exists());
        drop(guard);
        let first = vec![entry(directory.path().join("first.vmf"))];
        let publication = store.save_checked(&first, &source).unwrap();
        let saved = publication.source.unwrap();
        let disk = fs::read(store.path()).unwrap();
        let error = store.save_checked(&[], &source).unwrap_err();
        assert!(error.conflict);
        assert_eq!(fs::read(store.path()).unwrap(), disk);
        let guard = WriteLock::acquire(
            &store.directory,
            LOCK_FILE_NAME,
            "Recent maps",
            "Reload saved list",
        )
        .unwrap();
        guard.verify().unwrap();
        drop(guard);
        store.save_checked(&[], &saved).unwrap().source.unwrap();
        assert!(lock_path.is_file());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn lock_replacement_symlinks_and_referring_directory_changes_fail_closed() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let state = directory.path().join("state");
        fs::create_dir(&state).unwrap();
        let lock = state.join(LOCK_FILE_NAME);
        let target = directory.path().join("target");
        fs::write(&target, b"unrelated").unwrap();
        symlink(&target, &lock).unwrap();
        assert!(
            WriteLock::acquire(&state, LOCK_FILE_NAME, "Recent maps", "Reload saved list").is_err()
        );
        assert_eq!(fs::read(&target).unwrap(), b"unrelated");
        fs::remove_file(&lock).unwrap();
        let guard =
            WriteLock::acquire(&state, LOCK_FILE_NAME, "Recent maps", "Reload saved list").unwrap();
        fs::rename(&lock, state.join("old-lock")).unwrap();
        fs::write(&lock, b"").unwrap();
        assert!(guard.verify().is_err());
        drop(guard);
        let alias = directory.path().join("alias");
        symlink(&state, &alias).unwrap();
        let guard =
            WriteLock::acquire(&alias, LOCK_FILE_NAME, "Recent maps", "Reload saved list").unwrap();
        fs::remove_file(&alias).unwrap();
        symlink(directory.path(), &alias).unwrap();
        assert!(guard.verify().is_err());
        assert!(lock.is_file());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn missing_json_does_not_erase_the_loaded_directory_identity() {
        use std::os::unix::fs::symlink;
        for replace_directory in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let first = directory.path().join("first");
            let second = directory.path().join("second");
            fs::create_dir(&first).unwrap();
            fs::create_dir(&second).unwrap();
            let alias = directory.path().join("alias");
            symlink(&first, &alias).unwrap();
            let store = Store {
                directory: alias.clone(),
            };
            let (_, source) = store.load_snapshot().unwrap();
            if replace_directory {
                fs::rename(&first, directory.path().join("old-first")).unwrap();
                fs::create_dir(&first).unwrap();
            } else {
                fs::remove_file(&alias).unwrap();
                symlink(&second, &alias).unwrap();
            }
            assert!(store.save_checked(&[], &source).unwrap_err().conflict);
            assert!(!first.join(FILE_NAME).exists());
            assert!(!second.join(FILE_NAME).exists());
            let (_, fresh) = store.load_snapshot().unwrap();
            store.save_checked(&[], &fresh).unwrap().source.unwrap();
            assert!(alias.join(FILE_NAME).is_file());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn external_replacement_after_commit_is_reported_as_written_but_unverified() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().into(),
        };
        let (_, source) = store.load_snapshot().unwrap();
        let entries = vec![entry(directory.path().join("map.vmf"))];
        let publication = store
            .save_checked_with(&entries, &source, |path, data| {
                crate::recovery::atomic_write(path, data)?;
                fs::write(path, b"external writer after commit").map_err(|error| error.to_string())
            })
            .unwrap();
        assert!(publication.source.unwrap_err().contains("were written"));
        assert_eq!(
            fs::read(store.path()).unwrap(),
            b"external writer after commit"
        );
    }

    #[test]
    fn reflection_context_roundtrips_and_legacy_context_defaults_off() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().to_owned(),
        };
        let mut saved = entry(directory.path().join("reflections.vmf"));
        saved.build_settings.lighting = "direct_ldr".into();
        saved.build_settings.generate_reflections = true;
        store.save(std::slice::from_ref(&saved)).unwrap();
        assert_eq!(store.load().unwrap(), [saved.clone()]);
        let path = directory.path().join(FILE_NAME);
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        legacy["entries"][0]["build_settings"]
            .as_object_mut()
            .unwrap()
            .remove("generate_reflections");
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let loaded = store.load().unwrap();
        assert!(!loaded[0].build_settings.generate_reflections);
        assert_eq!(loaded[0].build_settings.lighting, "direct_ldr");
        saved.build_settings.lighting = "direct_ldr_hdr".into();
        store.save(std::slice::from_ref(&saved)).unwrap();
        assert_eq!(store.load().unwrap(), [saved.clone()]);
        let original = fs::read(&path).unwrap();
        saved.build_settings.lighting = "fullbright".into();
        assert!(store.save(&[saved]).is_err());
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[test]
    fn prop_vertex_lighting_recent_context_preserves_legacy_bytes_and_validates_saved_settings() {
        let legacy = br#"{"lighting":"fullbright","physics":"none","sky_quality":"fast","visibility":"fast","bounces":0,"smoothing_angle":0,"generate_reflections":false}"#;
        let settings: BuildSettings = serde_json::from_slice(legacy).unwrap();
        assert!(!settings.bake_static_prop_vertices);
        assert_eq!(serde_json::to_vec(&settings).unwrap(), legacy.as_slice());
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().to_owned(),
        };
        let path = directory.path().join(FILE_NAME);
        let mut saved = entry(directory.path().join("lit-prop.vmf"));
        store.save(std::slice::from_ref(&saved)).unwrap();
        let disabled = fs::read(&path).unwrap();
        assert!(
            !String::from_utf8(disabled.clone())
                .unwrap()
                .contains("bake_static_prop_vertices")
        );
        for lighting in ["direct_ldr", "direct_ldr_hdr"] {
            saved.build_settings = BuildSettings::from_options(compiler::Options {
                lighting: compiler::LightingMode::parse(lighting).unwrap(),
                bake_static_prop_vertices: true,
                bounces: compiler::MAX_LIGHT_BOUNCES,
                ..Default::default()
            });
            store.save(std::slice::from_ref(&saved)).unwrap();
            assert_eq!(store.load().unwrap(), [saved.clone()]);
        }
        let bytes = fs::read(&path).unwrap();
        saved.build_settings.bounces = compiler::MAX_LIGHT_BOUNCES + 1;
        assert!(store.save(std::slice::from_ref(&saved)).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let mut invalid: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        invalid["entries"][0]["build_settings"]["lighting"] = "fullbright".into();
        let invalid = serde_json::to_vec(&invalid).unwrap();
        fs::write(&path, &invalid).unwrap();
        assert!(store.load().is_err());
        assert_eq!(fs::read(&path).unwrap(), invalid);
        fs::write(&path, &disabled).unwrap();
        let loaded = store.load().unwrap();
        assert!(!loaded[0].build_settings.bake_static_prop_vertices);
        store.save(&loaded).unwrap();
        assert_eq!(fs::read(path).unwrap(), disabled);
    }

    #[test]
    fn asset_packing_context_roundtrips_and_legacy_bytes_stay_compatible() {
        let legacy = br#"{"lighting":"fullbright","physics":"none","sky_quality":"fast","visibility":"fast","bounces":0,"smoothing_angle":0,"generate_reflections":false}"#;
        let settings: BuildSettings = serde_json::from_slice(legacy).unwrap();
        settings.validate().unwrap();
        assert!(!settings.pack_water_assets);
        assert!(!settings.pack_static_prop_assets);
        assert_eq!(
            settings,
            BuildSettings::from_options(compiler::Options::default())
        );
        assert_eq!(serde_json::to_vec(&settings).unwrap(), legacy.as_slice());
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().to_owned(),
        };
        let mut saved = entry(directory.path().join("portable.vmf"));
        store.save(std::slice::from_ref(&saved)).unwrap();
        let path = directory.path().join(FILE_NAME);
        let original = fs::read(&path).unwrap();
        assert!(
            !String::from_utf8(original.clone())
                .unwrap()
                .contains("pack_sky_assets")
        );
        assert_eq!(store.load().unwrap(), [saved.clone()]);
        for lighting in ["fullbright", "direct_ldr", "direct_ldr_hdr"] {
            saved.build_settings = BuildSettings::from_options(compiler::Options {
                lighting: compiler::LightingMode::parse(lighting).unwrap(),
                pack_sky_assets: true,
                pack_water_assets: true,
                pack_static_prop_assets: true,
                ..Default::default()
            });
            store.save(std::slice::from_ref(&saved)).unwrap();
            let bytes = fs::read(&path).unwrap();
            let file: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(file["version"], FORMAT_VERSION);
            assert_eq!(
                file["entries"][0]["build_settings"]["pack_sky_assets"],
                true
            );
            assert_eq!(
                file["entries"][0]["build_settings"]["pack_water_assets"],
                true
            );
            assert_eq!(
                file["entries"][0]["build_settings"]["pack_static_prop_assets"],
                true
            );
            let restarted = Store {
                directory: directory.path().to_owned(),
            };
            assert_eq!(restarted.load().unwrap(), [saved.clone()]);
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        fs::write(&path, &original).unwrap();
        let loaded = store.load().unwrap();
        assert!(!loaded[0].build_settings.pack_sky_assets);
        assert!(!loaded[0].build_settings.pack_water_assets);
        assert!(!loaded[0].build_settings.pack_static_prop_assets);
        store.save(&loaded).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn invalid_asset_packing_context_is_rejected_without_rewriting() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().to_owned(),
        };
        let mut saved = entry(directory.path().join("portable.vmf"));
        saved.build_settings.pack_sky_assets = true;
        saved.build_settings.pack_water_assets = true;
        saved.build_settings.pack_static_prop_assets = true;
        store.save(std::slice::from_ref(&saved)).unwrap();
        let path = directory.path().join(FILE_NAME);
        let original = fs::read(&path).unwrap();
        let valid: serde_json::Value = serde_json::from_slice(&original).unwrap();
        let mut invalid = Vec::new();
        for value in [
            serde_json::json!(null),
            serde_json::json!("true"),
            serde_json::json!(1),
            serde_json::json!([]),
            serde_json::json!({}),
        ] {
            for field in [
                "pack_sky_assets",
                "pack_water_assets",
                "pack_static_prop_assets",
            ] {
                let mut file = valid.clone();
                file["entries"][0]["build_settings"][field] = value.clone();
                invalid.push(serde_json::to_vec(&file).unwrap());
            }
        }
        let mut unknown = valid;
        unknown["entries"][0]["build_settings"]["pack_sky_asset"] = serde_json::json!(true);
        invalid.push(serde_json::to_vec(&unknown).unwrap());
        for field in [
            "pack_sky_assets",
            "pack_water_assets",
            "pack_static_prop_assets",
        ] {
            let duplicate = String::from_utf8(original.clone()).unwrap().replace(
                &format!("\"{field}\": true"),
                &format!("\"{field}\": true, \"{field}\": false"),
            );
            invalid.push(duplicate.into_bytes());
        }
        for bytes in invalid {
            fs::write(&path, &bytes).unwrap();
            assert!(store.load().is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn invalid_or_oversized_preferences_are_rejected_without_rewriting() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().to_owned(),
        };
        let path = directory.path().join(FILE_NAME);
        let valid = entry(directory.path().join("map.vmf"));
        store.save(std::slice::from_ref(&valid)).unwrap();
        let original = fs::read(&path).unwrap();
        assert!(
            store
                .save(&vec![valid.clone(); MAX_RECENT_MAPS + 1])
                .is_err()
        );
        assert!(store.save(&[valid.clone(), valid.clone()]).is_err());
        let mut invalid = valid;
        invalid.build_settings.smoothing_angle = 91;
        assert!(store.save(&[invalid]).is_err());
        assert!(store.save(&[entry(PathBuf::from("relative.vmf"))]).is_err());
        assert!(
            store
                .save(&[entry(PathBuf::from(format!(
                    "/{}",
                    "x".repeat(MAX_PATH_BYTES)
                )))])
                .is_err()
        );
        assert!(
            store
                .save(&[entry(directory.path().join("bad\n.vmf"))])
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        for bytes in [b"broken".to_vec(), vec![b' '; MAX_FILE_BYTES as usize + 1]] {
            fs::write(&path, &bytes).unwrap();
            assert!(store.load().is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[cfg(unix)]
    #[test]
    fn storage_rejects_symlinks_without_reading_or_replacing_the_target() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().join("state"),
        };
        fs::create_dir(&store.directory).unwrap();
        let target = directory.path().join("target.json");
        let original = b"unrelated target data";
        fs::write(&target, original).unwrap();
        std::os::unix::fs::symlink(&target, store.directory.join(FILE_NAME)).unwrap();
        assert!(store.load().is_err());
        assert!(store.save(&[]).is_err());
        assert_eq!(fs::read(target).unwrap(), original);
    }
}
