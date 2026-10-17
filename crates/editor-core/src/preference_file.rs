use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

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

pub(crate) fn read_source(path: &Path, max_file_bytes: u64, label: &str) -> Result<Source, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Source::Missing);
        }
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if !metadata.is_file() {
        return Err(format!("{} must be a regular file", path.display()));
    }
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let opened = file.metadata().map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (opened.dev(), opened.ino()) != (metadata.dev(), metadata.ino()) {
            return Err(format!("{label} were replaced while loading"));
        }
    }
    let modified = metadata
        .modified()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if !opened.is_file()
        || opened.len() != metadata.len()
        || opened.modified().map_err(|error| error.to_string())? != modified
    {
        return Err(format!("{label} changed while loading"));
    }
    let mut bytes = Vec::new();
    (&file)
        .take(max_file_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let after = file.metadata().map_err(|error| error.to_string())?;
    if bytes.len() as u64 != metadata.len().min(max_file_bytes + 1)
        || after.len() != metadata.len()
        || after.modified().map_err(|error| error.to_string())? != modified
    {
        return Err(format!("{label} changed size while loading"));
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

#[derive(Clone, Debug)]
pub(crate) struct ObservedSource {
    pub source: Source,
    file_name: &'static str,
    directory: DirectoryBinding,
}

#[derive(Clone, Debug)]
struct DirectoryBinding {
    requested: PathBuf,
    ancestor: PathBuf,
    physical: PathBuf,
    label: &'static str,
    reload: &'static str,
    #[cfg(unix)]
    identity: (u64, u64),
}

impl DirectoryBinding {
    fn capture(
        directory: &Path,
        label: &'static str,
        reload: &'static str,
    ) -> Result<Self, String> {
        let requested = std::path::absolute(directory).map_err(|error| error.to_string())?;
        let mut ancestor = requested.clone();
        let metadata = loop {
            match fs::metadata(&ancestor) {
                Ok(metadata) if metadata.is_dir() => break metadata,
                Ok(_) => return Err(format!("{label} directory must be a directory")),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if !ancestor.pop() {
                        return Err(error.to_string());
                    }
                }
                Err(error) => return Err(error.to_string()),
            }
        };
        let physical = ancestor.canonicalize().map_err(|error| error.to_string())?;
        let binding = Self {
            requested,
            ancestor,
            physical,
            label,
            reload,
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (metadata.dev(), metadata.ino())
            },
        };
        binding.verify()?;
        Ok(binding)
    }

    fn verify(&self) -> Result<(), String> {
        let metadata = fs::metadata(&self.ancestor).map_err(|error| error.to_string())?;
        let same = metadata.is_dir()
            && self
                .ancestor
                .canonicalize()
                .map_err(|error| error.to_string())?
                == self.physical;
        #[cfg(unix)]
        let same = {
            use std::os::unix::fs::MetadataExt;
            same && (metadata.dev(), metadata.ino()) == self.identity
        };
        if !same {
            return Err(format!("{} directory changed; {}", self.label, self.reload));
        }
        let suffix = self
            .requested
            .strip_prefix(&self.ancestor)
            .map_err(|error| error.to_string())?;
        let mut path = self.physical.clone();
        for part in suffix.components() {
            path.push(part);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                _ => {
                    return Err(format!(
                        "{} directory binding changed; {}",
                        self.label, self.reload
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct Publication {
    pub source: Result<ObservedSource, String>,
}

#[derive(Debug)]
pub(crate) struct SaveError {
    pub conflict: bool,
    pub message: String,
}

impl SaveError {
    pub(crate) fn failure(message: impl Into<String>) -> Self {
        Self {
            conflict: false,
            message: map_core::entity_helpers::bounded_preview(&message.into(), 512),
        }
    }
}

pub(crate) struct WriteLock {
    file: File,
    parent: File,
    directory: PathBuf,
    requested_directory: PathBuf,
    path: PathBuf,
    label: &'static str,
    reload: &'static str,
}

impl WriteLock {
    #[cfg(target_os = "linux")]
    pub(crate) fn acquire(
        directory: &Path,
        lock_name: &str,
        label: &'static str,
        reload: &'static str,
    ) -> Result<Self, String> {
        use rustix::fs::{FlockOperation, Mode, OFlags};
        fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        let requested_directory =
            std::path::absolute(directory).map_err(|error| error.to_string())?;
        let directory = directory
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let parent = File::open(&directory).map_err(|error| error.to_string())?;
        let path = directory.join(lock_name);
        let descriptor = rustix::fs::open(
            &path,
            OFlags::CREATE | OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|error| format!("{label} lock unavailable: {error}"))?;
        let file = File::from(descriptor);
        if !file
            .metadata()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            return Err(format!("{label} lock must be a regular file"));
        }
        rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive)
            .map_err(|error| format!("{label} are busy or cannot be locked; retry: {error}"))?;
        let guard = Self {
            file,
            parent,
            directory,
            requested_directory,
            path,
            label,
            reload,
        };
        guard.verify()?;
        Ok(guard)
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn acquire(
        _directory: &Path,
        _lock_name: &str,
        label: &'static str,
        _reload: &'static str,
    ) -> Result<Self, String> {
        Err(format!("Coordinated {label} writes require Linux"))
    }

    pub(crate) fn verify(&self) -> Result<(), String> {
        let opened = self.file.metadata().map_err(|error| error.to_string())?;
        let current = fs::symlink_metadata(&self.path).map_err(|error| error.to_string())?;
        let parent = self.parent.metadata().map_err(|error| error.to_string())?;
        let directory =
            fs::metadata(&self.requested_directory).map_err(|error| error.to_string())?;
        let same = opened.is_file()
            && current.is_file()
            && parent.is_dir()
            && directory.is_dir()
            && self
                .requested_directory
                .canonicalize()
                .map_err(|error| error.to_string())?
                == self.directory;
        #[cfg(unix)]
        let same = {
            use std::os::unix::fs::MetadataExt;
            same && (opened.dev(), opened.ino()) == (current.dev(), current.ino())
                && (parent.dev(), parent.ino()) == (directory.dev(), directory.ino())
        };
        if !same {
            return Err(format!(
                "{} lock or directory changed identity; retry {}",
                self.label, self.reload
            ));
        }
        Ok(())
    }
}

pub(crate) struct CoordinatedFile<'a> {
    pub directory: &'a Path,
    pub file_name: &'static str,
    pub lock_name: &'static str,
    pub label: &'static str,
    pub reload: &'static str,
    pub saved_description: &'static str,
    pub max_bytes: u64,
}

impl CoordinatedFile<'_> {
    fn path(&self) -> PathBuf {
        self.directory.join(self.file_name)
    }

    pub fn observe(&self) -> Result<ObservedSource, String> {
        let directory = DirectoryBinding::capture(self.directory, self.label, self.reload)?;
        let source = read_source(&self.path(), self.max_bytes, self.label)?;
        directory.verify()?;
        Ok(ObservedSource {
            source,
            directory,
            file_name: self.file_name,
        })
    }

    fn verify_expected(&self, expected: &ObservedSource) -> Result<(), SaveError> {
        let label = self.label;
        let reload = self.reload;
        if expected.file_name != self.file_name {
            return Err(SaveError {
                conflict: true,
                message: format!("{label} source belongs to a different file; {reload}"),
            });
        }
        if std::path::absolute(self.directory)
            .map_err(|error| SaveError::failure(error.to_string()))?
            != expected.directory.requested
        {
            return Err(SaveError {
                conflict: true,
                message: format!("{label} source belongs to a different directory; {reload}"),
            });
        }
        expected.directory.verify().map_err(|message| SaveError {
            conflict: true,
            message,
        })?;
        Ok(())
    }

    pub fn observe_from(&self, expected: &ObservedSource) -> Result<ObservedSource, String> {
        self.verify_expected(expected)
            .map_err(|error| error.message)?;
        let current = self.observe()?;
        self.verify_expected(expected)
            .map_err(|error| error.message)?;
        Ok(current)
    }

    pub fn publish(
        &self,
        data: &[u8],
        expected: &ObservedSource,
    ) -> Result<Publication, SaveError> {
        self.publish_with(data, expected, crate::recovery::atomic_write)
    }

    pub fn publish_with(
        &self,
        data: &[u8],
        expected: &ObservedSource,
        publish: impl FnOnce(&Path, &[u8]) -> Result<(), String>,
    ) -> Result<Publication, SaveError> {
        let label = self.label;
        let reload = self.reload;
        if data.len() as u64 > self.max_bytes {
            return Err(SaveError::failure(format!(
                "{label} must fit in {} bytes",
                self.max_bytes
            )));
        }
        self.verify_expected(expected)?;
        let guard = WriteLock::acquire(self.directory, self.lock_name, label, reload)
            .map_err(SaveError::failure)?;
        expected.directory.verify().map_err(|message| SaveError {
            conflict: true,
            message,
        })?;
        let current = self.observe().map_err(SaveError::failure)?;
        if current.source != expected.source {
            return Err(SaveError {
                conflict: true,
                message: format!(
                    "{label} changed in another editor or outside this session; {reload} before changing it"
                ),
            });
        }
        guard.verify().map_err(SaveError::failure)?;
        publish(&self.path(), data).map_err(SaveError::failure)?;
        let source = guard
            .verify()
            .and_then(|()| self.observe())
            .and_then(|source| {
                guard.verify()?;
                if matches!(&source.source, Source::File {bytes,..} if bytes==data) {
                    Ok(source)
                } else {
                    Err(format!("{label} changed after publication"))
                }
            })
            .map_err(|error| {
                format!(
                    "{label} were written but could not verify the {}; {reload}: {error}",
                    self.saved_description
                )
            });
        Ok(Publication { source })
    }
}

#[cfg(test)]
mod tests;
