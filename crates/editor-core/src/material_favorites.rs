use std::collections::BTreeSet;
#[cfg(test)]
use std::fs;

use crate::preference_file::{CoordinatedFile, ObservedSource, Publication, SaveError, Source};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const MAX_FAVORITES: usize = 512;
pub const MAX_MATERIAL_BYTES: usize = 256;
pub const MAX_FILE_BYTES: u64 = 192 * 1024;
pub const FILE_NAME: &str = "material-favorites.json";
const FORMAT_VERSION: u32 = 1;
const LOCK_FILE_NAME: &str = ".material-favorites.lock";
pub const MAX_ERROR_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Persistence {
    Session,
    Persistent,
    Blocked,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub persistence: Persistence,
    pub directory: Option<PathBuf>,
    pub error: Option<String>,
    pub count: usize,
    pub reload_available: bool,
    pub max_favorites: usize,
    pub max_material_bytes: usize,
    pub max_file_bytes: u64,
}

impl Status {
    pub fn session(count: usize) -> Self {
        Self {
            persistence: Persistence::Session,
            directory: None,
            error: None,
            count,
            reload_available: false,
            max_favorites: MAX_FAVORITES,
            max_material_bytes: MAX_MATERIAL_BYTES,
            max_file_bytes: MAX_FILE_BYTES,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u32,
    materials: Vec<String>,
}

pub struct Store {
    pub directory: PathBuf,
}

pub fn canonical_name(material: &str) -> Result<String, String> {
    if material.chars().any(char::is_control) {
        return Err("Material favorite paths cannot contain control characters".into());
    }
    let trimmed = material.trim();
    if trimmed.len() > MAX_MATERIAL_BYTES + "materials/".len() + ".vmt".len() {
        return Err(format!(
            "Material favorite paths must be at most {MAX_MATERIAL_BYTES} bytes"
        ));
    }
    let name = crate::session::material_search::material_name(trimmed)?;
    if crate::session::material_search::material_name(&name)? != name {
        return Err(
            "Use one optional materials/ prefix and .vmt suffix, without spaces inside those wrappers"
                .into(),
        );
    }
    source_formats::gamefs::asset_relative(&name).map_err(|error| error.to_string())?;
    if name.len() > MAX_MATERIAL_BYTES {
        return Err(format!(
            "Material favorite paths must be at most {MAX_MATERIAL_BYTES} bytes"
        ));
    }
    Ok(name)
}

impl Store {
    fn path(&self) -> PathBuf {
        self.directory.join(FILE_NAME)
    }

    pub fn load(&self) -> Result<BTreeSet<String>, String> {
        self.load_snapshot().map(|(materials, _)| materials)
    }

    pub(crate) fn load_snapshot(&self) -> Result<(BTreeSet<String>, ObservedSource), String> {
        let source = self.target().observe()?;
        let path = self.path();
        let Source::File { bytes, length, .. } = &source.source else {
            return Ok((BTreeSet::new(), source));
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
                "{}: unsupported material favorites version {}",
                path.display(),
                preferences.version
            ));
        }
        if preferences.materials.len() > MAX_FAVORITES {
            return Err(format!(
                "{} contains more than {MAX_FAVORITES} material favorites",
                path.display()
            ));
        }
        let materials = preferences
            .materials
            .iter()
            .map(|name| {
                canonical_name(name).map_err(|error| format!("{}: {error}", path.display()))
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        Ok((materials, source))
    }

    fn target(&self) -> CoordinatedFile<'_> {
        CoordinatedFile {
            directory: &self.directory,
            file_name: FILE_NAME,
            lock_name: LOCK_FILE_NAME,
            label: "Material favorites",
            reload: "Reload favorites",
            saved_description: "saved list",
            max_bytes: MAX_FILE_BYTES,
        }
    }

    pub fn save(&self, materials: &BTreeSet<String>) -> Result<(), String> {
        let (_, source) = self.load_snapshot()?;
        self.save_checked(materials, &source)
            .map_err(|error| error.message)?
            .source
            .map(|_| ())
    }

    pub(crate) fn save_checked(
        &self,
        materials: &BTreeSet<String>,
        expected: &ObservedSource,
    ) -> Result<Publication, SaveError> {
        let data = self.encode(materials).map_err(SaveError::failure)?;
        self.target().publish(&data, expected)
    }

    fn encode(&self, materials: &BTreeSet<String>) -> Result<Vec<u8>, String> {
        if materials.len() > MAX_FAVORITES {
            return Err(format!("Keep at most {MAX_FAVORITES} material favorites"));
        }
        let preferences = Preferences {
            version: FORMAT_VERSION,
            materials: materials
                .iter()
                .map(|name| canonical_name(name))
                .collect::<Result<BTreeSet<_>, _>>()?
                .into_iter()
                .collect(),
        };
        let data = serde_json::to_vec_pretty(&preferences).map_err(|error| error.to_string())?;
        if data.len() as u64 > MAX_FILE_BYTES {
            return Err(format!(
                "Material favorites must fit in {MAX_FILE_BYTES} bytes"
            ));
        }
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().join("preferences"),
        };
        (directory, store)
    }

    #[test]
    fn material_favorite_paths_are_canonical_and_bounded() {
        for name in [
            "brick/wall",
            " BRICK/WALL ",
            "Materials/Brick/Wall.VMT",
            "materials\\brick\\wall.vmt",
        ] {
            let canonical = canonical_name(name).unwrap();
            assert_eq!(canonical, "brick/wall");
            assert_eq!(canonical_name(&canonical).unwrap(), canonical);
        }
        for name in [
            "",
            " ",
            "materials/",
            "/brick/wall",
            "\\brick\\wall",
            "../wall",
            "brick/../wall",
            "brick/./wall",
            "brick//wall",
            "brick/",
            "C:\\brick\\wall",
            "brick/wall:other",
            "brick/*",
            "brick/?",
            "brick/wall\n",
            "\tbrick/wall",
            "brick/\0wall",
            "materials/materials/brick/wall",
            "brick/wall.vmt.vmt",
            "materials/ brick/wall",
            "brick/wall .vmt",
        ] {
            assert!(canonical_name(name).is_err(), "{name:?}");
        }
        let longest = "x".repeat(MAX_MATERIAL_BYTES);
        assert_eq!(canonical_name(&longest).unwrap(), longest);
        assert_eq!(
            canonical_name(&format!("materials/{longest}.vmt")).unwrap(),
            longest
        );
        assert!(canonical_name(&"x".repeat(MAX_MATERIAL_BYTES + 1)).is_err());
        assert!(canonical_name(&"é".repeat(MAX_MATERIAL_BYTES)).is_err());
    }

    #[test]
    fn store_roundtrips_sorted_canonical_paths_without_creating_on_load() {
        let (_directory, store) = fixture();
        assert!(store.load().unwrap().is_empty());
        assert!(!store.directory.exists());
        let materials = BTreeSet::from(["wood/floor".into(), "brick/wall".into()]);
        store.save(&materials).unwrap();
        assert_eq!(store.load().unwrap(), materials);
        let preferences: Preferences =
            serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(preferences.version, FORMAT_VERSION);
        assert_eq!(preferences.materials, vec!["brick/wall", "wood/floor"]);
        let aliases =
            br#"{"version":1,"materials":["BRICK/WALL","materials/brick/wall.vmt","wood/floor"]}"#;
        fs::write(store.path(), aliases).unwrap();
        assert_eq!(store.load().unwrap(), materials);
        assert_eq!(fs::read(store.path()).unwrap(), aliases);
    }

    #[test]
    fn store_rejects_malformed_unsupported_oversized_and_nonregular_files() {
        let (_directory, store) = fixture();
        fs::create_dir_all(&store.directory).unwrap();
        for data in [
            "broken",
            r#"{"version":0,"materials":[]}"#,
            r#"{"version":2,"materials":[]}"#,
            r#"{"version":1,"materials":"brick/wall"}"#,
            r#"{"version":1,"materials":["../wall"]}"#,
            r#"{"version":1,"materials":[],"other":true}"#,
        ] {
            fs::write(store.path(), data).unwrap();
            assert!(store.load().is_err(), "{data}");
            assert_eq!(fs::read_to_string(store.path()).unwrap(), data);
        }
        let too_many = Preferences {
            version: FORMAT_VERSION,
            materials: vec!["brick/wall".into(); MAX_FAVORITES + 1],
        };
        fs::write(store.path(), serde_json::to_vec(&too_many).unwrap()).unwrap();
        assert!(store.load().unwrap_err().contains("more than"));
        fs::write(store.path(), vec![b' '; MAX_FILE_BYTES as usize + 1]).unwrap();
        assert!(store.load().unwrap_err().contains("at most"));
        fs::remove_file(store.path()).unwrap();
        fs::create_dir(store.path()).unwrap();
        assert!(store.load().unwrap_err().contains("regular file"));
        assert!(store.save(&BTreeSet::new()).is_err());
    }

    #[test]
    fn store_accepts_limits_and_rejects_invalid_saves_without_replacing_file() {
        let (_directory, store) = fixture();
        let materials: BTreeSet<_> = (0..MAX_FAVORITES)
            .map(|index| format!("{index:03}/{}", "x".repeat(MAX_MATERIAL_BYTES - 4)))
            .collect();
        store.save(&materials).unwrap();
        let original = fs::read(store.path()).unwrap();
        assert_eq!(store.load().unwrap(), materials);
        assert!(original.len() as u64 <= MAX_FILE_BYTES);
        let mut extra = materials.clone();
        extra.insert("extra/wall".into());
        assert!(store.save(&extra).is_err());
        assert!(store.save(&BTreeSet::from(["../wall".into()])).is_err());
        assert_eq!(fs::read(store.path()).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn store_rejects_symlinks_without_reading_or_replacing_the_target() {
        let (directory, store) = fixture();
        fs::create_dir_all(&store.directory).unwrap();
        let target = directory.path().join("unrelated.json");
        let original = br#"{"version":1,"materials":["brick/wall"]}"#;
        fs::write(&target, original).unwrap();
        std::os::unix::fs::symlink(&target, store.path()).unwrap();
        assert!(store.load().is_err());
        assert!(store.save(&BTreeSet::new()).is_err());
        assert!(fs::symlink_metadata(store.path()).unwrap().is_symlink());
        assert_eq!(fs::read(target).unwrap(), original);
    }
}
