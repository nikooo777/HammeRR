use std::path::PathBuf;

pub(crate) use crate::preference_file::ObservedSource as Source;
use crate::preference_file::{CoordinatedFile, Publication, SaveError, Source as RawSource};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "navigation-defaults.json";
pub const MAX_FILE_BYTES: u64 = 4096;
const FORMAT_VERSION: u32 = 1;

pub const MIN_FLY_SPEED: f32 = 25.0;
pub const MAX_FLY_SPEED: f32 = 20_000.0;
pub const DEFAULT_FLY_SPEED: f32 = 600.0;
pub const FAST_MULTIPLIER: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Navigation {
    pub fly_speed: f32,
}

impl Default for Navigation {
    fn default() -> Self {
        Self {
            fly_speed: DEFAULT_FLY_SPEED,
        }
    }
}

impl Navigation {
    pub fn validate(self) -> Result<(), String> {
        if !self.fly_speed.is_finite() || !(MIN_FLY_SPEED..=MAX_FLY_SPEED).contains(&self.fly_speed)
        {
            return Err(format!(
                "Fly speed must be finite and between {MIN_FLY_SPEED} and {MAX_FLY_SPEED} units per second"
            ));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u32,
    navigation: Navigation,
}

pub struct Store {
    pub directory: PathBuf,
}

impl Store {
    pub fn path(&self) -> PathBuf {
        self.directory.join(FILE_NAME)
    }

    pub fn load(&self) -> Result<Option<Navigation>, String> {
        self.parse_source(&self.read_source()?)
    }

    fn target(&self) -> CoordinatedFile<'_> {
        CoordinatedFile {
            directory: &self.directory,
            file_name: FILE_NAME,
            lock_name: ".navigation-defaults.lock",
            label: "Navigation defaults",
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

    pub(crate) fn parse_source(&self, source: &Source) -> Result<Option<Navigation>, String> {
        let RawSource::File { bytes, length, .. } = &source.source else {
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
                "Unsupported navigation defaults version {}; file is preserved",
                preferences.version
            ));
        }
        preferences.navigation.validate()?;
        Ok(Some(preferences.navigation))
    }

    pub fn save(&self, navigation: Navigation, reset: bool) -> Result<(), String> {
        let source = self.read_source()?;
        self.save_checked(navigation, reset, &source)
            .map_err(|error| error.message)?
            .source
            .map(|_| ())
    }

    pub(crate) fn save_checked(
        &self,
        navigation: Navigation,
        reset: bool,
        expected: &Source,
    ) -> Result<Publication, SaveError> {
        navigation.validate().map_err(SaveError::failure)?;
        if !reset {
            self.parse_source(expected).map_err(SaveError::failure)?;
        }
        let data = serde_json::to_vec_pretty(&Preferences {
            version: FORMAT_VERSION,
            navigation,
        })
        .map_err(|error| SaveError::failure(error.to_string()))?;
        self.target().publish(&data, expected)
    }
}
