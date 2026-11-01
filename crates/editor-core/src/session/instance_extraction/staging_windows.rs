use super::*;
use source_formats::file_publication::{
    FileIdentity,
    windows::{open_directory, open_file},
};
use std::fs::{self, File};
use std::io::Write;
use std::sync::Mutex;

pub(super) type Cleanup = Arc<Mutex<Option<String>>>;

pub(super) struct Stage {
    temporary: tempfile::NamedTempFile,
    directory: File,
    cleanup: Cleanup,
}

impl Stage {
    pub(super) fn create(parent: &Path, cleanup: Cleanup) -> Result<Self, String> {
        let directory = open_directory(parent).map_err(|error| error.to_string())?;
        let temporary = tempfile::Builder::new()
            .prefix(".hammerr-extract-")
            .suffix(".vmf")
            .disable_cleanup(true)
            .tempfile_in(parent)
            .map_err(|error| error.to_string())?;
        let stage = Self {
            temporary,
            directory,
            cleanup,
        };
        stage.verify_parent(parent)?;
        Ok(stage)
    }

    pub(super) fn path(&self) -> &Path {
        self.temporary.path()
    }

    fn verify_parent(&self, parent: &Path) -> Result<(), String> {
        let current = open_directory(parent).map_err(|error| error.to_string())?;
        if FileIdentity::from_file(&current).map_err(|error| error.to_string())?
            != FileIdentity::from_file(&self.directory).map_err(|error| error.to_string())?
        {
            return Err(
                "Private staging directory no longer matches the guarded child directory".into(),
            );
        }
        Ok(())
    }

    pub(super) fn verify_owned(&self) -> Result<(), String> {
        let current = open_file(self.path()).map_err(|error| error.to_string())?;
        if FileIdentity::from_file(&current).map_err(|error| error.to_string())?
            != FileIdentity::from_file(self.temporary.as_file())
                .map_err(|error| error.to_string())?
        {
            return Err(
                "Private extraction staging name no longer identifies the owned file".into(),
            );
        }
        Ok(())
    }

    pub(super) fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.temporary
            .write_all(bytes)
            .map_err(|error| error.to_string())?;
        self.temporary
            .as_file()
            .sync_all()
            .map_err(|error| error.to_string())
    }

    pub(super) fn publish(self, destination: &Path) -> Result<(), EditError> {
        self.verify_owned().map_err(EditError::Invalid)?;
        self.verify_parent(
            destination
                .parent()
                .ok_or_else(|| EditError::Invalid("Child directory is missing".into()))?,
        )
        .map_err(EditError::Invalid)?;
        fs::hard_link(self.path(), destination)
            .map_err(|error| EditError::Invalid(error.to_string()))
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        if !self.path().exists() {
            return;
        }
        let result = self
            .verify_owned()
            .and_then(|()| fs::remove_file(self.path()).map_err(|error| error.to_string()));
        if let Err(error) = result {
            let mut report = self
                .cleanup
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *report = Some(map_core::entity_helpers::bounded_preview(&error, 512));
        }
    }
}
