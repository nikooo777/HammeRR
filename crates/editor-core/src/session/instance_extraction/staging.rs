use super::*;
use std::io::Write;
use std::sync::Mutex;
use tempfile::NamedTempFile;

pub(super) type Cleanup = Arc<Mutex<Option<String>>>;

pub(super) struct Stage {
    temporary: NamedTempFile,
    #[cfg(target_os = "linux")]
    directory: std::fs::File,
    basename: std::ffi::OsString,
    cleanup: Cleanup,
}

impl Stage {
    pub(super) fn create(parent: &Path, cleanup: Cleanup) -> Result<Self, String> {
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::{Mode, OFlags};
            let directory = rustix::fs::open(
                parent,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map(std::fs::File::from)
            .map_err(|error| error.to_string())?;
            let temporary = tempfile::Builder::new()
                .prefix(".hammerr-extract-")
                .suffix(".vmf")
                .disable_cleanup(true)
                .make_in(parent, |path| {
                    let name = path
                        .file_name()
                        .ok_or_else(|| std::io::Error::other("Staging basename is missing"))?;
                    rustix::fs::openat(
                        &directory,
                        name,
                        OFlags::CREATE
                            | OFlags::EXCL
                            | OFlags::RDWR
                            | OFlags::CLOEXEC
                            | OFlags::NOFOLLOW,
                        Mode::RUSR | Mode::WUSR,
                    )
                    .map(std::fs::File::from)
                    .map_err(std::io::Error::from)
                })
                .map_err(|error| error.to_string())?;
            let basename = temporary
                .path()
                .file_name()
                .ok_or("Staging basename is missing")?
                .to_owned();
            let stage = Self {
                temporary,
                directory,
                basename,
                cleanup,
            };
            stage.verify_parent(parent)?;
            Ok(stage)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (parent, cleanup);
            Err("Instance extraction requires Linux directory-relative staging ownership".into())
        }
    }

    pub(super) fn path(&self) -> &Path {
        self.temporary.path()
    }

    #[cfg(target_os = "linux")]
    fn verify_parent(&self, parent: &Path) -> Result<(), String> {
        use std::os::unix::fs::MetadataExt;
        let retained = self
            .directory
            .metadata()
            .map_err(|error| error.to_string())?;
        let current = std::fs::symlink_metadata(parent).map_err(|error| error.to_string())?;
        if !current.is_dir()
            || current.file_type().is_symlink()
            || retained.dev() != current.dev()
            || retained.ino() != current.ino()
        {
            return Err(
                "Private staging directory no longer matches the guarded child directory".into(),
            );
        }
        Ok(())
    }

    pub(super) fn verify_owned(&self) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            let expected = self
                .temporary
                .as_file()
                .metadata()
                .map_err(|error| error.to_string())?;
            let actual = rustix::fs::statat(
                &self.directory,
                &self.basename,
                rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
            )
            .map_err(|error| error.to_string())?;
            if actual.st_dev != expected.dev() || actual.st_ino != expected.ino() {
                return Err(
                    "Private extraction staging name no longer identifies the owned file".into(),
                );
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err("Instance extraction requires Linux staging ownership".into())
        }
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
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::{AtFlags, RenameFlags};
            self.verify_owned().map_err(EditError::Invalid)?;
            self.verify_parent(
                destination
                    .parent()
                    .ok_or_else(|| EditError::Invalid("Child directory is missing".into()))?,
            )
            .map_err(EditError::Invalid)?;
            let name = destination
                .file_name()
                .ok_or_else(|| EditError::Invalid("Child basename is missing".into()))?;
            match rustix::fs::renameat_with(
                &self.directory,
                &self.basename,
                &self.directory,
                name,
                RenameFlags::NOREPLACE,
            ) {
                Ok(()) => Ok(()),
                Err(error)
                    if matches!(
                        error,
                        rustix::io::Errno::NOSYS
                            | rustix::io::Errno::INVAL
                            | rustix::io::Errno::OPNOTSUPP
                    ) =>
                {
                    rustix::fs::linkat(
                        &self.directory,
                        &self.basename,
                        &self.directory,
                        name,
                        AtFlags::empty(),
                    )
                    .map_err(|error| EditError::Invalid(error.to_string()))
                }
                Err(error) => Err(EditError::Invalid(error.to_string())),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = destination;
            Err(EditError::Invalid(
                "Instance extraction requires Linux directory-relative publication".into(),
            ))
        }
    }

    fn warn(&self, error: String) {
        let mut report = self
            .cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *report = Some(map_core::entity_helpers::bounded_preview(&error, 512));
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::AtFlags;
            use std::os::unix::fs::MetadataExt;
            let expected = match self.temporary.as_file().metadata() {
                Ok(metadata) => metadata,
                Err(error) => {
                    self.warn(format!(
                        "Private extraction stage ownership check failed: {error}"
                    ));
                    return;
                }
            };
            match rustix::fs::statat(&self.directory,&self.basename,AtFlags::SYMLINK_NOFOLLOW) {
                Ok(actual) if actual.st_dev==expected.dev() && actual.st_ino==expected.ino()=> {
                    if let Err(error)=rustix::fs::unlinkat(&self.directory,&self.basename,AtFlags::empty()) {
                        self.warn(format!("Private extraction staging cleanup failed for {}: {error}",self.path().display()));
                    }
                }
                Ok(_)=>self.warn(format!("Private extraction staging name {} was replaced; unrelated replacement preserved",self.path().display())),
                Err(rustix::io::Errno::NOENT)=>{},
                Err(error)=>self.warn(format!("Private extraction staging cleanup could not check {}: {error}",self.path().display())),
            }
        }
    }
}
