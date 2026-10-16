use std::{
    fs::{self, File},
    io::Read,
    path::PathBuf,
};

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

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Source {
    Missing,
    File {
        bytes: Vec<u8>,
        length: u64,
        modified: std::time::SystemTime,
        #[cfg(unix)]
        identity: (u64, u64),
    },
}

impl Store {
    pub fn path(&self) -> PathBuf {
        self.directory.join(FILE_NAME)
    }

    pub fn load(&self) -> Result<Option<Grid>, String> {
        self.parse_source(&self.read_source()?)
    }

    pub(crate) fn read_source(&self) -> Result<Source, String> {
        let path = self.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Source::Missing);
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        if !metadata.is_file() {
            return Err(format!("{} must be a regular file", path.display()));
        }
        let file = File::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let opened = file.metadata().map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if (opened.dev(), opened.ino()) != (metadata.dev(), metadata.ino()) {
                return Err("Grid defaults were replaced while loading".into());
            }
        }
        let modified = metadata
            .modified()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if !opened.is_file()
            || opened.len() != metadata.len()
            || opened.modified().map_err(|error| error.to_string())? != modified
        {
            return Err("Grid defaults changed while loading".into());
        }
        let mut bytes = Vec::new();
        (&file)
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let after = file.metadata().map_err(|error| error.to_string())?;
        if bytes.len() as u64 != metadata.len().min(MAX_FILE_BYTES + 1)
            || after.len() != metadata.len()
            || after.modified().map_err(|error| error.to_string())? != modified
        {
            return Err("Grid defaults changed size while loading".into());
        }
        Ok(Source::File {
            bytes,
            length: metadata.len(),
            modified,
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (metadata.dev(), metadata.ino())
            },
        })
    }

    pub(crate) fn parse_source(&self, source: &Source) -> Result<Option<Grid>, String> {
        let Source::File { bytes, length, .. } = source else {
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
        grid.validate()?;
        if !reset {
            self.load()?;
        }
        let path = self.path();
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_file() => {
                return Err(format!("{} must be a regular file", path.display()));
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
        let data = serde_json::to_vec_pretty(&Preferences {
            version: FORMAT_VERSION,
            grid,
        })
        .map_err(|error| error.to_string())?;
        fs::create_dir_all(&self.directory)
            .map_err(|error| format!("{}: {error}", self.directory.display()))?;
        crate::recovery::atomic_write(&path, &data)
    }
}

#[cfg(test)]
mod tests;
