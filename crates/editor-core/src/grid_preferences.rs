use std::path::PathBuf;

pub(crate) use crate::preference_file::ObservedSource as Source;
use crate::preference_file::{CoordinatedFile, Publication, SaveError, Source as RawSource};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "grid-defaults.json";
pub const MAX_FILE_BYTES: u64 = 4096;
const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grid {
    pub grid_size: f64,
    pub snap: bool,
    pub grid_visible: bool,
}

impl Default for Grid {
    fn default() -> Self {
        Self::from(crate::spatial::Settings::default())
    }
}

impl From<crate::spatial::Settings> for Grid {
    fn from(settings: crate::spatial::Settings) -> Self {
        Self {
            grid_size: settings.grid_size,
            snap: settings.snap,
            grid_visible: settings.grid_visible,
        }
    }
}

impl Grid {
    pub fn update(self) -> crate::spatial::Update {
        crate::spatial::Update {
            grid_size: Some(self.grid_size),
            snap: Some(self.snap),
            grid_visible: Some(self.grid_visible),
            ..Default::default()
        }
    }

    pub fn validate(self) -> Result<(), String> {
        crate::spatial::Settings::default().updated(self.update())?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u32,
    grid: Grid,
}

pub struct Store {
    pub directory: PathBuf,
}

impl Store {
    pub fn path(&self) -> PathBuf {
        self.directory.join(FILE_NAME)
    }

    pub fn load(&self) -> Result<Option<Grid>, String> {
        self.parse_source(&self.read_source()?)
    }

    fn target(&self) -> CoordinatedFile<'_> {
        CoordinatedFile {
            directory: &self.directory,
            file_name: FILE_NAME,
            lock_name: ".grid-defaults.lock",
            label: "Grid defaults",
            reload: "Reload defaults",
            saved_description: "saved defaults",
            max_bytes: MAX_FILE_BYTES,
        }
    }

    pub(crate) fn read_source(&self) -> Result<Source, String> {
        self.target().observe()
    }

    pub(crate) fn read_current(&self, expected: &Source) -> Result<Source, String> {
        self.target().observe_from(expected)
    }

    pub(crate) fn parse_source(&self, source: &Source) -> Result<Option<Grid>, String> {
        let RawSource::File { bytes, length, .. } = &source.source else {
            return Ok(None);
        };
        if *length > MAX_FILE_BYTES {
            return Err(format!(
                "{} exceeds {MAX_FILE_BYTES} bytes; file is preserved",
                self.path().display()
            ));
        }
        let path = self.path();
        let preferences: Preferences = serde_json::from_slice(bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if preferences.version != FORMAT_VERSION {
            return Err(format!(
                "Unsupported grid defaults version {}; file is preserved",
                preferences.version
            ));
        }
        preferences.grid.validate()?;
        Ok(Some(preferences.grid))
    }

    pub fn save(&self, grid: Grid, reset: bool) -> Result<(), String> {
        let source = self.read_source()?;
        self.save_checked(grid, reset, &source)
            .map_err(|error| error.message)?
            .source
            .map(|_| ())
    }

    pub(crate) fn save_checked(
        &self,
        grid: Grid,
        reset: bool,
        expected: &Source,
    ) -> Result<Publication, SaveError> {
        grid.validate().map_err(SaveError::failure)?;
        if !reset {
            self.parse_source(expected).map_err(SaveError::failure)?;
        }
        let data = serde_json::to_vec_pretty(&Preferences {
            version: FORMAT_VERSION,
            grid,
        })
        .map_err(|error| SaveError::failure(error.to_string()))?;
        self.target().publish(&data, expected)
    }
}

#[cfg(test)]
mod tests;
