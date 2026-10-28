use std::collections::BTreeSet;

use crate::preference_file::{CoordinatedFile, ObservedSource, Publication, SaveError, Source};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

mod authority;
pub use authority::{Authority, Snapshot};

pub const MAX_FAVORITES: usize = 512;
pub const MAX_MATERIAL_BYTES: usize = 256;
pub const MAX_FILE_BYTES: u64 = 192 * 1024;
pub const FILE_NAME: &str = "material-favorites.json";
const FORMAT_VERSION: u32 = 1;
const LOCK_FILE_NAME: &str = ".material-favorites.lock";
pub const MAX_ERROR_BYTES: usize = 512;
pub(crate) const RELOAD_UNAVAILABLE: &str =
    "Material favorites storage is unavailable; session-only favorites remain editable";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Persistence {
    Session,
    Persistent,
    Blocked,
}

#[derive(Clone, Debug, Serialize)]
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
