use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const MAX_RECENT_MAPS: usize = 20;
pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_FILE_BYTES: u64 = 192 * 1024;
pub const FILE_NAME: &str = "recent-maps.json";
const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSettings {
    pub lighting: String,
    pub physics: String,
    pub sky_quality: String,
    pub visibility: String,
    pub bounces: u8,
    pub smoothing_angle: u8,
    #[serde(default)]
    pub generate_reflections: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pack_sky_assets: bool,
}

impl BuildSettings {
    pub fn from_options(options: compiler::Options) -> Self {
        Self {
            lighting: options.lighting.name().into(),
            physics: options.physics.name().into(),
            sky_quality: options.sky_quality.name().into(),
            visibility: options.visibility.name().into(),
            bounces: options.bounces,
            smoothing_angle: options.smoothing_angle,
            generate_reflections: options.generate_reflections,
            pack_sky_assets: options.pack_sky_assets,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        compiler::Options {
            lighting: compiler::LightingMode::parse(&self.lighting)?,
            physics: compiler::PhysicsMode::parse(&self.physics)?,
            sky_quality: compiler::SkyQuality::parse(&self.sky_quality)?,
            visibility: compiler::VisibilityMode::parse(&self.visibility)?,
            bounces: self.bounces,
            smoothing_angle: self.smoothing_angle,
            generate_reflections: self.generate_reflections,
            pack_sky_assets: self.pack_sky_assets,
        }
        .validate()?;
        Ok(())
    }
}

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
    pub fn load(&self) -> Result<Vec<Entry>, String> {
        let path = self.directory.join(FILE_NAME);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            return Err(format!(
                "{} must be a regular file of at most {MAX_FILE_BYTES} bytes",
                path.display()
            ));
        }
        let file = File::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut data = Vec::new();
        file.take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut data)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if data.len() as u64 > MAX_FILE_BYTES || data.len() as u64 != metadata.len() {
            return Err(format!("{} changed size while loading", path.display()));
        }
        let preferences: Preferences = serde_json::from_slice(&data)
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
        Ok(preferences.entries)
    }

    pub fn save(&self, entries: &[Entry]) -> Result<(), String> {
        validate_entries(entries)?;
        let preferences = Preferences {
            version: FORMAT_VERSION,
            entries: entries.to_vec(),
        };
        let data = serde_json::to_vec_pretty(&preferences).map_err(|error| error.to_string())?;
        if data.len() as u64 > MAX_FILE_BYTES {
            return Err(format!("Recent maps must fit in {MAX_FILE_BYTES} bytes"));
        }
        fs::create_dir_all(&self.directory)
            .map_err(|error| format!("{}: {error}", self.directory.display()))?;
        let path = self.directory.join(FILE_NAME);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_file() => {
                return Err(format!("{} must be a regular file", path.display()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
        crate::recovery::atomic_write(&path, &data)
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
    fn sky_packing_context_roundtrips_and_legacy_bytes_stay_compatible() {
        let legacy = br#"{"lighting":"fullbright","physics":"none","sky_quality":"fast","visibility":"fast","bounces":0,"smoothing_angle":0,"generate_reflections":false}"#;
        let settings: BuildSettings = serde_json::from_slice(legacy).unwrap();
        settings.validate().unwrap();
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
            let restarted = Store {
                directory: directory.path().to_owned(),
            };
            assert_eq!(restarted.load().unwrap(), [saved.clone()]);
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        fs::write(&path, &original).unwrap();
        let loaded = store.load().unwrap();
        assert!(!loaded[0].build_settings.pack_sky_assets);
        store.save(&loaded).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn invalid_sky_packing_context_is_rejected_without_rewriting() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().to_owned(),
        };
        let mut saved = entry(directory.path().join("portable.vmf"));
        saved.build_settings.pack_sky_assets = true;
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
            let mut file = valid.clone();
            file["entries"][0]["build_settings"]["pack_sky_assets"] = value;
            invalid.push(serde_json::to_vec(&file).unwrap());
        }
        let mut unknown = valid;
        unknown["entries"][0]["build_settings"]["pack_sky_asset"] = serde_json::json!(true);
        invalid.push(serde_json::to_vec(&unknown).unwrap());
        invalid.push(
            String::from_utf8(original)
                .unwrap()
                .replace(
                    "\"pack_sky_assets\": true",
                    "\"pack_sky_assets\": true, \"pack_sky_assets\": false",
                )
                .into_bytes(),
        );
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
