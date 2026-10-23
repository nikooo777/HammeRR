use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::preference_file::{CoordinatedFile, ObservedSource, Publication, SaveError, Source};
pub use crate::session::selection_filters::SelectionFilters;

pub const FILE_NAME: &str = "selection-defaults.json";
pub const MAX_FILE_BYTES: u64 = 4096;
const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u32,
    filters: SelectionFilters,
}

pub struct Store {
    pub directory: PathBuf,
}

impl Store {
    pub fn path(&self) -> PathBuf {
        self.directory.join(FILE_NAME)
    }

    pub fn load(&self) -> Result<Option<SelectionFilters>, String> {
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
    ) -> Result<Option<SelectionFilters>, String> {
        let Source::File { bytes, length, .. } = &source.source else {
            return Ok(None);
        };
        if *length > MAX_FILE_BYTES {
            return Err(format!(
                "{} exceeds {MAX_FILE_BYTES} bytes; file is preserved",
                self.path().display()
            ));
        }
        let preferences: Preferences = serde_json::from_slice(bytes)
            .map_err(|error| format!("{}: {error}", self.path().display()))?;
        if preferences.version != FORMAT_VERSION {
            return Err(format!(
                "Unsupported selection defaults version {}; file is preserved",
                preferences.version
            ));
        }
        Ok(Some(preferences.filters))
    }

    pub fn save(&self, filters: SelectionFilters, reset: bool) -> Result<(), String> {
        let source = self.read_source()?;
        self.save_checked(filters, reset, &source)
            .map_err(|error| error.message)?
            .source
            .map(|_| ())
    }

    pub(crate) fn save_checked(
        &self,
        filters: SelectionFilters,
        reset: bool,
        expected: &ObservedSource,
    ) -> Result<Publication, SaveError> {
        if !reset {
            self.parse_source(expected).map_err(SaveError::failure)?;
        }
        let data = serde_json::to_vec_pretty(&Preferences {
            version: FORMAT_VERSION,
            filters,
        })
        .map_err(|error| SaveError::failure(error.to_string()))?;
        self.target().publish(&data, expected)
    }
}
