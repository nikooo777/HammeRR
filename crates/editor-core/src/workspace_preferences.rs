use std::path::PathBuf;

pub(crate) use crate::preference_file::ObservedSource as Source;
use crate::preference_file::{CoordinatedFile, Publication, SaveError, Source as RawSource};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "workspace-defaults.json";
pub const MAX_FILE_BYTES: u64 = 4096;
const FORMAT_VERSION: u32 = 1;

pub const MIN_DOCK_WIDTH: u16 = 240;
pub const MAX_DOCK_WIDTH: u16 = 640;
pub const DEFAULT_DOCK_WIDTH: u16 = 344;
pub const MIN_AGENT_HEIGHT: u16 = 96;
pub const MAX_AGENT_HEIGHT: u16 = 480;
pub const DEFAULT_AGENT_HEIGHT: u16 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub dock_visible: bool,
    #[schemars(range(min = 240, max = 640))]
    pub dock_width: u16,
    pub agent_visible: bool,
    #[schemars(range(min = 96, max = 480))]
    pub agent_height: u16,
    pub tool_rail_visible: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            dock_visible: true,
            dock_width: DEFAULT_DOCK_WIDTH,
            agent_visible: true,
            agent_height: DEFAULT_AGENT_HEIGHT,
            tool_rail_visible: true,
        }
    }
}

impl Layout {
    pub fn validate(self) -> Result<(), String> {
        if !(MIN_DOCK_WIDTH..=MAX_DOCK_WIDTH).contains(&self.dock_width) {
            return Err(format!(
                "Dock width must be between {MIN_DOCK_WIDTH} and {MAX_DOCK_WIDTH} logical pixels"
            ));
        }
        if !(MIN_AGENT_HEIGHT..=MAX_AGENT_HEIGHT).contains(&self.agent_height) {
            return Err(format!(
                "Agent feed height must be between {MIN_AGENT_HEIGHT} and {MAX_AGENT_HEIGHT} logical pixels"
            ));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u32,
    layout: Layout,
}

pub struct Store {
    pub directory: PathBuf,
}

impl Store {
    pub fn path(&self) -> PathBuf {
        self.directory.join(FILE_NAME)
    }

    pub fn load(&self) -> Result<Option<Layout>, String> {
        self.parse_source(&self.read_source()?)
    }

    fn target(&self) -> CoordinatedFile<'_> {
        CoordinatedFile {
            directory: &self.directory,
            file_name: FILE_NAME,
            lock_name: ".workspace-defaults.lock",
            label: "Workspace defaults",
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

    pub(crate) fn parse_source(&self, source: &Source) -> Result<Option<Layout>, String> {
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
                "Unsupported workspace defaults version {}; file is preserved",
                preferences.version
            ));
        }
        preferences.layout.validate()?;
        Ok(Some(preferences.layout))
    }

    pub fn save(&self, layout: Layout, reset: bool) -> Result<(), String> {
        let source = self.read_source()?;
        self.save_checked(layout, reset, &source)
            .map_err(|error| error.message)?
            .source
            .map(|_| ())
    }

    pub(crate) fn save_checked(
        &self,
        layout: Layout,
        reset: bool,
        expected: &Source,
    ) -> Result<Publication, SaveError> {
        layout.validate().map_err(SaveError::failure)?;
        if !reset {
            self.parse_source(expected).map_err(SaveError::failure)?;
        }
        let data = serde_json::to_vec_pretty(&Preferences {
            version: FORMAT_VERSION,
            layout,
        })
        .map_err(|error| SaveError::failure(error.to_string()))?;
        self.target().publish(&data, expected)
    }
}
