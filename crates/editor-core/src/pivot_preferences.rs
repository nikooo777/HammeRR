use std::path::PathBuf;

pub(crate) use crate::preference_file::ObservedSource as Source;
use crate::preference_file::{CoordinatedFile, Publication, SaveError, Source as RawSource};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "pivot-defaults.json";
pub const MAX_FILE_BYTES: u64 = 4096;
const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Pivot {
    SelectionCenter {},
    WorldOrigin {},
    Custom { point: [f64; 3] },
}

impl Default for Pivot {
    fn default() -> Self {
        Self::SelectionCenter {}
    }
}

pub fn validate_point(point: glam::DVec3) -> Result<(), String> {
    if !point.is_finite() {
        return Err("Transform pivot coordinates must be finite".into());
    }
    Ok(())
}

impl Pivot {
    pub fn from_point(point: Option<glam::DVec3>) -> Self {
        match point {
            None => Self::SelectionCenter {},
            Some(point) if point == glam::DVec3::ZERO => Self::WorldOrigin {},
            Some(point) => Self::Custom {
                point: point.to_array(),
            },
        }
    }

    pub fn point(self) -> Option<glam::DVec3> {
        match self {
            Self::SelectionCenter {} => None,
            Self::WorldOrigin {} => Some(glam::DVec3::ZERO),
            Self::Custom { point } => Some(glam::DVec3::from_array(point)),
        }
    }

    pub fn validate(self) -> Result<(), String> {
        if let Some(point) = self.point() {
            validate_point(point)?;
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u32,
    pivot: Pivot,
}

pub struct Store {
    pub directory: PathBuf,
}

impl Store {
    pub fn path(&self) -> PathBuf {
        self.directory.join(FILE_NAME)
    }

    pub fn load(&self) -> Result<Option<Pivot>, String> {
        self.parse_source(&self.read_source()?)
    }

    fn target(&self) -> CoordinatedFile<'_> {
        CoordinatedFile {
            directory: &self.directory,
            file_name: FILE_NAME,
            lock_name: ".pivot-defaults.lock",
            label: "Transform pivot defaults",
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

    pub(crate) fn parse_source(&self, source: &Source) -> Result<Option<Pivot>, String> {
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
                "Unsupported transform pivot defaults version {}; file is preserved",
                preferences.version
            ));
        }
        preferences.pivot.validate()?;
        Ok(Some(preferences.pivot))
    }

    pub fn save(&self, pivot: Pivot, reset: bool) -> Result<(), String> {
        let source = self.read_source()?;
        self.save_checked(pivot, reset, &source)
            .map_err(|error| error.message)?
            .source
            .map(|_| ())
    }

    pub(crate) fn save_checked(
        &self,
        pivot: Pivot,
        reset: bool,
        expected: &Source,
    ) -> Result<Publication, SaveError> {
        pivot.validate().map_err(SaveError::failure)?;
        if !reset {
            self.parse_source(expected).map_err(SaveError::failure)?;
        }
        let data = serde_json::to_vec_pretty(&Preferences {
            version: FORMAT_VERSION,
            pivot,
        })
        .map_err(|error| SaveError::failure(error.to_string()))?;
        self.target().publish(&data, expected)
    }
}
