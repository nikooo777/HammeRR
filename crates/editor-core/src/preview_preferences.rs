use std::{fs, path::PathBuf};

pub(crate) use crate::preference_file::Source;

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "preview-defaults.json";
pub const MAX_FILE_BYTES: u64 = 4096;
const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preview {
    pub selection_outlines: bool,
    pub entity_images: bool,
    pub entity_model_guides: bool,
    pub rope_preview: bool,
    pub sky_preview: bool,
}

impl Default for Preview {
    fn default() -> Self {
        Self::from(render::ViewOptions::default())
    }
}

impl From<render::ViewOptions> for Preview {
    fn from(view: render::ViewOptions) -> Self {
        Self {
            selection_outlines: view.selection_outlines,
            entity_images: view.entity_images,
            entity_model_guides: view.entity_model_guides,
            rope_preview: view.rope_preview,
            sky_preview: view.sky_preview,
        }
    }
}

impl Preview {
    pub fn update(self) -> crate::session::ViewUpdate<'static> {
        crate::session::ViewUpdate {
            selection_outlines: Some(self.selection_outlines),
            entity_images: Some(self.entity_images),
            entity_model_guides: Some(self.entity_model_guides),
            rope_preview: Some(self.rope_preview),
            sky_preview: Some(self.sky_preview),
            ..Default::default()
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u32,
    preview: Preview,
}

pub struct Store {
    pub directory: PathBuf,
}

impl Store {
    pub fn path(&self) -> PathBuf {
        self.directory.join(FILE_NAME)
    }

    pub fn load(&self) -> Result<Option<Preview>, String> {
        self.parse_source(&self.read_source()?)
    }

    pub(crate) fn read_source(&self) -> Result<Source, String> {
        crate::preference_file::read_source(&self.path(), MAX_FILE_BYTES, "Preview defaults")
    }

    pub(crate) fn parse_source(&self, source: &Source) -> Result<Option<Preview>, String> {
        let Source::File { bytes, length, .. } = source else {
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
                "Unsupported preview defaults version {}; file is preserved",
                preferences.version
            ));
        }
        Ok(Some(preferences.preview))
    }

    pub fn save(&self, preview: Preview, reset: bool) -> Result<(), String> {
        if !reset {
            self.load()?;
        }
        let path = self.path();
        crate::preference_file::ensure_regular_target(&path)?;
        let data = serde_json::to_vec_pretty(&Preferences {
            version: FORMAT_VERSION,
            preview,
        })
        .map_err(|error| error.to_string())?;
        fs::create_dir_all(&self.directory)
            .map_err(|error| format!("{}: {error}", self.directory.display()))?;
        crate::recovery::atomic_write(&path, &data)
    }
}

#[cfg(test)]
mod tests;
