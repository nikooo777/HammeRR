use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod authority;
pub use authority::{Authority, Snapshot};

pub(crate) use crate::preference_file::ObservedSource;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reopen_path: Option<PathBuf>,
    pub game_directory: Option<PathBuf>,
    pub build_settings: BuildSettings,
}

impl Entry {
    pub fn reopen_path(&self) -> &Path {
        self.reopen_path.as_deref().unwrap_or(&self.path)
    }

    fn validate(&self) -> Result<(), String> {
        validate_path(&self.path)?;
        validate_path(self.reopen_path())?;
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

pub(crate) fn source_paths(path: &Path) -> Result<(PathBuf, Option<PathBuf>), String> {
    use source_formats::file_publication::{BoundDestination, Policy};

    let destination = BoundDestination::capture(path, Policy::FollowSource)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let physical = destination.physical_path();
    let authored = destination.authored_path();
    validate_path(physical)?;
    validate_path(authored)?;
    Ok((
        physical.to_owned(),
        (authored != physical).then(|| authored.to_owned()),
    ))
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
