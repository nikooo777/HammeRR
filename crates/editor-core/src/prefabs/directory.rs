use super::*;
use crate::preference_file::DirectoryBinding;
use std::{ffi::OsString, path::Path, sync::Arc};

#[derive(Clone, Debug)]
pub(super) struct SourceDirectory {
    pub binding: DirectoryBinding,
    _retained: Arc<File>,
}

impl PartialEq for SourceDirectory {
    fn eq(&self, other: &Self) -> bool {
        self.binding == other.binding
    }
}

impl Eq for SourceDirectory {}

impl SourceDirectory {
    pub fn verify(&self) -> Result<(), String> {
        self.binding.verify()
    }
}

pub(super) struct Directory {
    pub binding: DirectoryBinding,
    requested: PathBuf,
    ancestor: File,
    file: Option<Arc<File>>,
}

impl Directory {
    #[cfg(target_os = "linux")]
    pub fn capture(requested: &Path) -> Result<Self, String> {
        let requested: PathBuf = requested.components().collect();
        let binding = DirectoryBinding::capture(
            &requested,
            "Prefab library",
            "refresh it before continuing",
        )?;
        let ancestor = {
            use rustix::fs::{Mode, OFlags};
            rustix::fs::open(
                binding.physical_ancestor(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map(File::from)
            .map_err(|error| error.to_string())?
        };
        if !binding.matches_ancestor(&ancestor.metadata().map_err(|error| error.to_string())?) {
            return Err("Prefab library directory changed; refresh it before continuing".into());
        }
        let file = if binding.suffix()?.as_os_str().is_empty() {
            Some(Arc::new(
                ancestor.try_clone().map_err(|error| error.to_string())?,
            ))
        } else {
            None
        };
        let directory = Self {
            binding,
            requested,
            ancestor,
            file,
        };
        directory.verify()?;
        Ok(directory)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn capture(_requested: &Path) -> Result<Self, String> {
        Err("Bound prefab library I/O requires Linux".into())
    }

    pub fn verify(&self) -> Result<(), String> {
        self.binding.verify()?;
        match (&self.file, fs::symlink_metadata(&self.requested)) {
            (None, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            (Some(file), Ok(current)) if current.is_dir() => {
                let opened = file.metadata().map_err(|error| error.to_string())?;
                #[cfg(unix)]
                if (opened.dev(), opened.ino()) != (current.dev(), current.ino()) {
                    return Err(
                        "Prefab library directory changed; refresh it before continuing".into(),
                    );
                }
                Ok(())
            }
            _ => Err("Prefab library directory changed; refresh it before continuing".into()),
        }
    }

    pub fn is_present(&self) -> bool {
        self.file.is_some()
    }

    pub fn create(&mut self) -> Result<(), String> {
        self.verify()?;
        if self.file.is_some() {
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::{Mode, OFlags};
            let mut parent = self
                .ancestor
                .try_clone()
                .map_err(|error| error.to_string())?;
            for part in self.binding.suffix()?.components() {
                let part = part.as_os_str();
                match rustix::fs::mkdirat(&parent, part, Mode::RWXU) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(error) => return Err(error.to_string()),
                }
                parent = rustix::fs::openat(
                    &parent,
                    part,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                    Mode::empty(),
                )
                .map(File::from)
                .map_err(|error| error.to_string())?;
                self.binding.verify()?;
            }
            self.file = Some(Arc::new(parent));
            self.verify()
        }
        #[cfg(not(target_os = "linux"))]
        Err("Bound prefab directory creation requires Linux".into())
    }

    fn handle(&self) -> Result<&File, String> {
        self.file
            .as_deref()
            .ok_or("Prefab library no longer exists".into())
    }

    pub fn source(&self) -> Result<SourceDirectory, String> {
        Ok(SourceDirectory {
            binding: self.binding.clone(),
            _retained: self.file.clone().ok_or("Prefab library no longer exists")?,
        })
    }

    pub fn names(&self) -> Result<Vec<OsString>, String> {
        if !self.is_present() {
            return Ok(Vec::new());
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::ffi::OsStrExt;
            let mut directory =
                rustix::fs::Dir::read_from(self.handle()?).map_err(|error| error.to_string())?;
            let mut names = Vec::new();
            while let Some(entry) = directory.read() {
                let entry = entry.map_err(|error| error.to_string())?;
                let name = std::ffi::OsStr::from_bytes(entry.file_name().to_bytes());
                if name == "." || name == ".." {
                    continue;
                }
                if names.len() >= MAX_SCAN {
                    return Err("Prefab storage exceeds 512 directory entries".into());
                }
                names.push(name.to_owned());
            }
            Ok(names)
        }
        #[cfg(not(target_os = "linux"))]
        Err("Bound prefab library I/O requires Linux".into())
    }

    pub fn metadata(&self, name: &std::ffi::OsStr) -> Result<fs::Metadata, String> {
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::{Mode, OFlags};
            let file = rustix::fs::openat(
                self.handle()?,
                name,
                OFlags::PATH | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map(File::from)
            .map_err(|error| error.to_string())?;
            file.metadata().map_err(|error| error.to_string())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = name;
            Err("Bound prefab library I/O requires Linux".into())
        }
    }

    pub fn open(&self, name: &std::ffi::OsStr) -> Result<File, String> {
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::{Mode, OFlags};
            rustix::fs::openat(
                self.handle()?,
                name,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map(File::from)
            .map_err(|error| error.to_string())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = name;
            Err("Bound prefab library I/O requires Linux".into())
        }
    }

    pub fn rename(&self, source: &str, destination: &str) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            rustix::fs::renameat_with(
                self.handle()?,
                source,
                self.handle()?,
                destination,
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(|error| {
                format!("Cannot rename prefab without replacing another entry: {error}")
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err("Atomic prefab renaming is supported on Linux only".into())
        }
    }

    pub fn remove(&self, name: &str) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            rustix::fs::unlinkat(self.handle()?, name, rustix::fs::AtFlags::empty())
                .map_err(|error| error.to_string())
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err("Bound prefab deletion requires Linux".into())
        }
    }

    pub fn publication(&self, action: &str) -> Mutation {
        Mutation {
            warning: self.verify().err().map(|error| {
                format!("Prefab was {action}, but the library could not be verified; refresh it before continuing: {error}")
            }),
        }
    }

    pub fn stage(&self) -> Result<Stage<'_>, String> {
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::{Mode, OFlags};
            let directory = self.handle()?;
            let temporary = tempfile::Builder::new()
                .prefix(".hammerr-prefab-")
                .disable_cleanup(true)
                .make_in(&self.requested, |path| {
                    let name = path.file_name().ok_or_else(|| {
                        std::io::Error::other("Prefab staging basename is missing")
                    })?;
                    rustix::fs::openat(
                        directory,
                        name,
                        OFlags::CREATE
                            | OFlags::EXCL
                            | OFlags::RDWR
                            | OFlags::CLOEXEC
                            | OFlags::NOFOLLOW,
                        Mode::RUSR | Mode::WUSR,
                    )
                    .map(File::from)
                    .map_err(std::io::Error::from)
                })
                .map_err(|error| error.to_string())?;
            let basename = temporary
                .path()
                .file_name()
                .ok_or("Prefab staging basename is missing")?
                .to_owned();
            Ok(Stage {
                temporary,
                directory,
                basename,
            })
        }
        #[cfg(not(target_os = "linux"))]
        Err("Bound prefab staging requires Linux".into())
    }
}

pub(super) struct Stage<'a> {
    temporary: tempfile::NamedTempFile,
    directory: &'a File,
    basename: OsString,
}

impl Stage<'_> {
    pub fn write(&mut self, text: &str) -> Result<(), String> {
        self.temporary
            .write_all(text.as_bytes())
            .map_err(|error| error.to_string())?;
        self.temporary
            .as_file()
            .sync_all()
            .map_err(|error| error.to_string())
    }

    #[cfg(target_os = "linux")]
    fn owned(&self) -> Result<bool, String> {
        let opened = self
            .temporary
            .as_file()
            .metadata()
            .map_err(|error| error.to_string())?;
        match rustix::fs::statat(
            self.directory,
            &self.basename,
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(current) => Ok(current.st_dev == opened.dev() && current.st_ino == opened.ino()),
            Err(rustix::io::Errno::NOENT) => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn publish(&self, destination: &str) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            if !self.owned()? {
                return Err("Prefab staging name was replaced; unrelated file preserved".into());
            }
            rustix::fs::linkat(
                self.directory,
                &self.basename,
                self.directory,
                destination,
                rustix::fs::AtFlags::empty(),
            )
            .map_err(|error| error.to_string())
        }
        #[cfg(not(target_os = "linux"))]
        Err("Bound prefab publication requires Linux".into())
    }
}

impl Drop for Stage<'_> {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if matches!(self.owned(), Ok(true)) {
            let _ =
                rustix::fs::unlinkat(self.directory, &self.basename, rustix::fs::AtFlags::empty());
        }
    }
}
