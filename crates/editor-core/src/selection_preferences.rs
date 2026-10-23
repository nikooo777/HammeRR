use std::{collections::HashSet, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::preference_file::{
    CoordinatedFile, ObservedSource, Publication, SaveError, Source, named_preference,
};
pub use crate::session::selection_filters::SelectionFilters;

pub const FILE_NAME: &str = "selection-defaults.json";
pub const MAX_FILE_BYTES: u64 = 16_384;
pub const MAX_PROFILES: usize = 32;
pub const MAX_NAME_BYTES: usize = 80;
const LEGACY_MAX_FILE_BYTES: u64 = 4096;
const LEGACY_VERSION: u32 = 1;
const FORMAT_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub filters: SelectionFilters,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SelectionDefaults {
    pub filters: SelectionFilters,
    pub profiles: Vec<Profile>,
    next_id: u64,
}

impl Default for SelectionDefaults {
    fn default() -> Self {
        Self {
            filters: SelectionFilters::default(),
            profiles: Vec::new(),
            next_id: 1,
        }
    }
}

impl From<SelectionFilters> for SelectionDefaults {
    fn from(filters: SelectionFilters) -> Self {
        Self {
            filters,
            ..Default::default()
        }
    }
}

pub enum Edit<'a> {
    Save {
        name: &'a str,
        replace: Option<&'a str>,
        filters: SelectionFilters,
    },
    Rename {
        id: &'a str,
        name: &'a str,
    },
    Delete {
        id: &'a str,
    },
}

impl SelectionDefaults {
    pub fn profile(&self, id: &str) -> Result<&Profile, String> {
        self.profiles
            .iter()
            .find(|profile| profile.id == id)
            .ok_or_else(|| "Selection profile no longer exists; refresh the list".into())
    }

    fn validate(&self) -> Result<(), String> {
        if self.profiles.len() > MAX_PROFILES || self.next_id == 0 {
            return Err("Unsupported or oversized selection profile catalogue".into());
        }
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for profile in &self.profiles {
            let id = profile
                .id
                .strip_prefix("profile:")
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|id| *id > 0 && *id < self.next_id)
                .ok_or("Invalid selection profile identifier")?;
            if profile.id != format!("profile:{id}")
                || !ids.insert(id)
                || !names.insert(profile.name.to_lowercase())
            {
                return Err("Duplicate or invalid selection profile identifier or name".into());
            }
            if named_preference(&profile.name, MAX_NAME_BYTES, "Selection profile")? != profile.name
            {
                return Err(
                    "Stored selection profile names cannot have surrounding whitespace".into(),
                );
            }
        }
        Ok(())
    }

    pub fn edit(&mut self, edit: Edit<'_>) -> Result<String, String> {
        let id = match edit {
            Edit::Save {
                name,
                replace,
                filters,
            } => {
                let name = named_preference(name, MAX_NAME_BYTES, "Selection profile")?;
                if self.profiles.iter().any(|profile| {
                    Some(profile.id.as_str()) != replace
                        && profile.name.to_lowercase() == name.to_lowercase()
                }) {
                    return Err("Choose a unique selection profile name".into());
                }
                match replace {
                    Some(id) => {
                        let profile = self
                            .profiles
                            .iter_mut()
                            .find(|profile| profile.id == id)
                            .ok_or("Selection profile no longer exists; refresh the list")?;
                        profile.name = name;
                        profile.filters = filters;
                        id.to_owned()
                    }
                    None => {
                        if self.profiles.len() == MAX_PROFILES {
                            return Err(format!("Keep at most {MAX_PROFILES} selection profiles"));
                        }
                        let next = self
                            .next_id
                            .checked_add(1)
                            .ok_or("Selection profile identifiers exhausted")?;
                        let id = format!("profile:{}", self.next_id);
                        self.profiles.push(Profile {
                            id: id.clone(),
                            name,
                            filters,
                        });
                        self.next_id = next;
                        id
                    }
                }
            }
            Edit::Rename { id, name } => {
                let name = named_preference(name, MAX_NAME_BYTES, "Selection profile")?;
                if self.profiles.iter().any(|profile| {
                    profile.id != id && profile.name.to_lowercase() == name.to_lowercase()
                }) {
                    return Err("Choose a unique selection profile name".into());
                }
                let profile = self
                    .profiles
                    .iter_mut()
                    .find(|profile| profile.id == id)
                    .ok_or("Selection profile no longer exists; refresh the list")?;
                profile.name = name;
                id.to_owned()
            }
            Edit::Delete { id } => {
                let index = self
                    .profiles
                    .iter()
                    .position(|profile| profile.id == id)
                    .ok_or("Selection profile no longer exists; refresh the list")?;
                self.profiles.remove(index).id
            }
        };
        self.validate()?;
        Ok(id)
    }
}

#[derive(Deserialize)]
struct Version {
    version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyPreferences {
    version: u32,
    filters: SelectionFilters,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u32,
    filters: SelectionFilters,
    next_id: u64,
    profiles: Vec<Profile>,
}

pub struct Store {
    pub directory: PathBuf,
}

impl Store {
    pub fn path(&self) -> PathBuf {
        self.directory.join(FILE_NAME)
    }

    pub fn load(&self) -> Result<Option<SelectionDefaults>, String> {
        self.parse_source(&self.read_source()?)
    }

    fn target(&self) -> CoordinatedFile<'_> {
        CoordinatedFile {
            directory: &self.directory,
            file_name: FILE_NAME,
            lock_name: ".selection-defaults.lock",
            label: "Selection defaults",
            reload: "Reload defaults",
            saved_description: "saved defaults",
            max_bytes: MAX_FILE_BYTES,
        }
    }

    pub(crate) fn read_source(&self) -> Result<ObservedSource, String> {
        self.target().observe()
    }

    pub(crate) fn read_current(&self, expected: &ObservedSource) -> Result<ObservedSource, String> {
        self.target().observe_from(expected)
    }

    pub(crate) fn parse_source(
        &self,
        source: &ObservedSource,
    ) -> Result<Option<SelectionDefaults>, String> {
        let Source::File { bytes, length, .. } = &source.source else {
            return Ok(None);
        };
        if *length > MAX_FILE_BYTES {
            return Err(format!(
                "{} exceeds {MAX_FILE_BYTES} bytes; file is preserved",
                self.path().display()
            ));
        }
        let version: Version = serde_json::from_slice(bytes)
            .map_err(|error| format!("{}: {error}", self.path().display()))?;
        let settings = match version.version {
            LEGACY_VERSION => {
                if *length > LEGACY_MAX_FILE_BYTES {
                    return Err(format!(
                        "{} exceeds {LEGACY_MAX_FILE_BYTES} bytes for version 1; file is preserved",
                        self.path().display()
                    ));
                }
                let legacy: LegacyPreferences = serde_json::from_slice(bytes)
                    .map_err(|error| format!("{}: {error}", self.path().display()))?;
                if legacy.version != LEGACY_VERSION {
                    return Err("Invalid selection defaults version".into());
                }
                SelectionDefaults::from(legacy.filters)
            }
            FORMAT_VERSION => {
                let preferences: Preferences = serde_json::from_slice(bytes)
                    .map_err(|error| format!("{}: {error}", self.path().display()))?;
                SelectionDefaults {
                    filters: preferences.filters,
                    profiles: preferences.profiles,
                    next_id: preferences.next_id,
                }
            }
            version => {
                return Err(format!(
                    "Unsupported selection defaults version {version}; file is preserved"
                ));
            }
        };
        settings.validate()?;
        Ok(Some(settings))
    }

    pub fn save(&self, filters: SelectionFilters, reset: bool) -> Result<(), String> {
        let source = self.read_source()?;
        let mut settings = if reset {
            self.parse_source(&source).ok().flatten()
        } else {
            self.parse_source(&source)?
        }
        .unwrap_or_default();
        settings.filters = filters;
        self.save_checked(settings, reset, &source)
            .map_err(|error| error.message)?
            .source
            .map(|_| ())
    }

    pub(crate) fn save_checked(
        &self,
        settings: SelectionDefaults,
        reset: bool,
        expected: &ObservedSource,
    ) -> Result<Publication, SaveError> {
        settings.validate().map_err(SaveError::failure)?;
        if !reset {
            self.parse_source(expected).map_err(SaveError::failure)?;
        }
        let data = serde_json::to_vec_pretty(&Preferences {
            version: FORMAT_VERSION,
            filters: settings.filters,
            next_id: settings.next_id,
            profiles: settings.profiles,
        })
        .map_err(|error| SaveError::failure(error.to_string()))?;
        if data.len() as u64 > MAX_FILE_BYTES {
            return Err(SaveError::failure(
                "Selection profiles exceed the storage limit",
            ));
        }
        self.target().publish(&data, expected)
    }
}
