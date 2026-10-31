use std::{
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use source_formats::file_publication::{BoundDestination, CommittedReceipt};

pub(crate) fn named_preference(
    name: &str,
    max_bytes: usize,
    label: &str,
) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > max_bytes || name.chars().any(char::is_control) {
        return Err(format!(
            "{label} name must contain 1–{max_bytes} bytes without control characters"
        ));
    }
    Ok(name.into())
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
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOFOLLOW).bits() as i32,
        );
    }
    let file = options
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
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
    file_name: OsString,
    directory: DirectoryBinding,
    following: Option<BoundDestination>,
}

pub(crate) use source_formats::file_publication::DirectoryBinding;

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

    #[cfg(windows)]
    pub(crate) fn acquire(
        directory: &Path,
        lock_name: &str,
        label: &'static str,
        reload: &'static str,
    ) -> Result<Self, String> {
        use source_formats::file_publication::windows::{lock_file, open_directory};

        fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        let requested_directory =
            std::path::absolute(directory).map_err(|error| error.to_string())?;
        let directory = directory
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let parent = open_directory(&directory).map_err(|error| error.to_string())?;
        let path = directory.join(lock_name);
        let file =
            lock_file(&path).map_err(|error| format!("{label} lock unavailable: {error}"))?;
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

    #[cfg(not(any(target_os = "linux", windows)))]
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

#[cfg(target_os = "linux")]
impl Drop for WriteLock {
    fn drop(&mut self) {
        use std::io::Write;

        // Forked children can retain this open file description until exec.
        if let Err(error) = rustix::fs::flock(&self.file, rustix::fs::FlockOperation::Unlock) {
            let _ = writeln!(
                std::io::stderr(),
                "{} lock could not unlock: {error}",
                self.label
            );
        }
    }
}

pub(crate) struct CoordinatedFile<'a, N: AsRef<OsStr> = &'static str> {
    pub directory: &'a Path,
    pub file_name: N,
    pub lock_name: &'static str,
    pub label: &'static str,
    pub reload: &'static str,
    pub saved_description: &'static str,
    pub max_bytes: u64,
}

impl<N: AsRef<OsStr>> CoordinatedFile<'_, N> {
    fn path(&self) -> PathBuf {
        self.directory.join(self.file_name.as_ref())
    }

    pub fn observe(&self) -> Result<ObservedSource, String> {
        let directory = DirectoryBinding::capture(self.directory, self.label, self.reload)?;
        let source = read_source(&self.path(), self.max_bytes, self.label)?;
        directory.verify()?;
        Ok(ObservedSource {
            source,
            directory,
            file_name: self.file_name.as_ref().to_owned(),
            following: None,
        })
    }

    #[cfg(any(target_os = "linux", windows))]
    pub fn observe_following(&self) -> Result<ObservedSource, String> {
        use source_formats::file_publication::Policy;

        let directory = DirectoryBinding::capture(self.directory, self.label, self.reload)?;
        let destination = BoundDestination::capture(&self.path(), Policy::FollowSource)
            .map_err(|error| format!("{}: {error}", self.path().display()))?;
        let source = read_source(destination.physical_path(), self.max_bytes, self.label)?;
        destination.verify().map_err(|error| error.to_string())?;
        directory.verify()?;
        Ok(ObservedSource {
            source,
            directory,
            file_name: self.file_name.as_ref().to_owned(),
            following: Some(destination),
        })
    }

    #[cfg(not(any(target_os = "linux", windows)))]
    pub fn observe_following(&self) -> Result<ObservedSource, String> {
        let directory = DirectoryBinding::capture(self.directory, self.label, self.reload)?;
        let path = self.path();
        let path = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_symlink() => {
                path.canonicalize().map_err(|error| error.to_string())?
            }
            _ => path,
        };
        let source = read_source(&path, self.max_bytes, self.label)?;
        directory.verify()?;
        Ok(ObservedSource {
            source,
            directory,
            file_name: self.file_name.as_ref().to_owned(),
            following: None,
        })
    }

    fn verify_expected(&self, expected: &ObservedSource) -> Result<(), SaveError> {
        let label = self.label;
        let reload = self.reload;
        if expected.file_name.as_os_str() != self.file_name.as_ref() {
            return Err(SaveError {
                conflict: true,
                message: format!("{label} source belongs to a different file; {reload}"),
            });
        }
        if std::path::absolute(self.directory)
            .map_err(|error| SaveError::failure(error.to_string()))?
            != expected.directory.requested()
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

    pub fn publish_following(
        &self,
        data: &[u8],
        expected: &ObservedSource,
    ) -> Result<Publication, SaveError> {
        self.publish_following_with(data, expected, publish_bound)
    }

    pub fn publish_following_with(
        &self,
        data: &[u8],
        expected: &ObservedSource,
        publish: impl FnOnce(&BoundDestination, &[u8]) -> Result<CommittedReceipt, String>,
    ) -> Result<Publication, SaveError> {
        let label = self.label;
        let reload = self.reload;
        if !cfg!(any(target_os = "linux", windows)) {
            return Err(SaveError::failure(format!(
                "Coordinated {label} writes require Linux"
            )));
        }
        if data.len() as u64 > self.max_bytes {
            return Err(SaveError::failure(format!(
                "{label} must fit in {} bytes",
                self.max_bytes
            )));
        }
        self.verify_expected(expected)?;
        let destination = expected
            .following
            .as_ref()
            .ok_or_else(|| SaveError::failure(format!("Inspect {label} before writing")))?;
        if destination.physical_path().file_name() == Some(std::ffi::OsStr::new(self.lock_name)) {
            return Err(SaveError::failure(format!(
                "{label} destination cannot be its coordination lock"
            )));
        }
        let verify_destination = || {
            destination.verify().map_err(|error| SaveError {
                conflict: true,
                message: format!("{label} changed outside this session; {reload}: {error}"),
            })
        };
        verify_destination()?;
        let directory = destination
            .physical_path()
            .parent()
            .ok_or_else(|| SaveError::failure("Settings destination has no parent"))?;
        let guard = WriteLock::acquire(directory, self.lock_name, label, reload)
            .map_err(SaveError::failure)?;
        self.verify_expected(expected)?;
        verify_destination()?;
        let current = self.observe_following().map_err(SaveError::failure)?;
        if current.source != expected.source {
            return Err(SaveError {
                conflict: true,
                message: format!(
                    "{label} changed in another editor or outside this session; {reload} before changing it"
                ),
            });
        }
        guard.verify().map_err(SaveError::failure)?;
        let receipt = publish(destination, data).map_err(SaveError::failure)?;
        let source = (|| {
            if let Some(warning) = receipt.warning {
                return Err(warning);
            }
            guard.verify()?;
            expected.directory.verify()?;
            receipt
                .destination
                .verify()
                .map_err(|error| error.to_string())?;
            let source = self.observe_following()?;
            guard.verify()?;
            receipt
                .destination
                .verify()
                .map_err(|error| error.to_string())?;
            if matches!(&source.source, Source::File { bytes, .. } if bytes == data) {
                Ok(source)
            } else {
                Err(format!("{label} changed after publication"))
            }
        })()
        .map_err(|error| {
            format!(
                "{label} were written but could not verify the {}; {reload}: {error}",
                self.saved_description
            )
        });
        Ok(Publication { source })
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

fn publish_bound(destination: &BoundDestination, data: &[u8]) -> Result<CommittedReceipt, String> {
    use std::io::Write;

    let mut stage = destination.stage().map_err(|error| error.to_string())?;
    stage.write_all(data).map_err(|error| error.to_string())?;
    stage.commit().map_err(|error| error.to_string())
}
