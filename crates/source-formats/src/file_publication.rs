use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DirectoryBinding {
    requested: PathBuf,
    ancestor: PathBuf,
    physical: PathBuf,
    label: &'static str,
    reload: &'static str,
    #[cfg(unix)]
    identity: (u64, u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
}

impl FileIdentity {
    #[cfg(unix)]
    fn capture(metadata: &fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    #[cfg(not(unix))]
    fn capture(_metadata: &fs::Metadata) -> Self {
        Self {
            device: 0,
            inode: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileSlot {
    pub parent: FileIdentity,
    pub leaf: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    FollowSource,
    ReplaceEntry,
}

#[derive(Clone, Debug)]
struct Directory {
    binding: DirectoryBinding,
    ancestor: Arc<File>,
    opened: Option<Arc<File>>,
    requested: PathBuf,
    physical: PathBuf,
}

impl Directory {
    #[cfg(target_os = "linux")]
    fn capture(path: &Path) -> io::Result<Self> {
        use rustix::fs::{Mode, OFlags};
        let binding = DirectoryBinding::capture(path, "File destination", "choose the path again")
            .map_err(io::Error::other)?;
        let ancestor = Arc::new(File::from(rustix::fs::open(
            binding.physical_ancestor(),
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?));
        if !binding.matches_ancestor(&ancestor.metadata()?) {
            return Err(io::Error::other(
                "File destination directory changed identity",
            ));
        }
        let opened = binding
            .suffix()
            .map_err(io::Error::other)?
            .as_os_str()
            .is_empty()
            .then(|| ancestor.clone());
        let directory = Self {
            requested: std::path::absolute(path)?,
            physical: binding
                .physical_ancestor()
                .join(binding.suffix().map_err(io::Error::other)?),
            binding,
            ancestor,
            opened,
        };
        directory.verify()?;
        Ok(directory)
    }

    #[cfg(not(target_os = "linux"))]
    fn capture(_path: &Path) -> io::Result<Self> {
        Err(io::Error::other("Bound file publication requires Linux"))
    }

    fn verify(&self) -> io::Result<()> {
        self.binding.verify().map_err(io::Error::other)?;
        if let Some(opened) = &self.opened {
            let current = fs::metadata(&self.requested)?;
            if !current.is_dir()
                || FileIdentity::capture(&current) != FileIdentity::capture(&opened.metadata()?)
            {
                return Err(io::Error::other(
                    "File destination directory changed identity",
                ));
            }
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn bind(&self) -> io::Result<Self> {
        use rustix::fs::{Mode, OFlags};
        self.verify()?;
        if self.opened.is_some() {
            return Ok(self.clone());
        }
        let mut file = self.ancestor.clone();
        for component in self
            .binding
            .suffix()
            .map_err(io::Error::other)?
            .components()
        {
            if !matches!(component, std::path::Component::Normal(_)) {
                return Err(io::Error::other(
                    "Invalid absent destination directory suffix",
                ));
            }
            file = Arc::new(File::from(rustix::fs::openat(
                &file,
                component.as_os_str(),
                OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )?));
        }
        let bound = Self {
            opened: Some(file),
            ..self.clone()
        };
        bound.verify()?;
        Ok(bound)
    }

    #[cfg(not(target_os = "linux"))]
    fn bind(&self) -> io::Result<Self> {
        Err(io::Error::other("Bound file publication requires Linux"))
    }

    fn handle(&self) -> io::Result<&File> {
        self.opened.as_deref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "File destination parent does not exist",
            )
        })
    }

    fn slot(&self, leaf: &std::ffi::OsStr) -> io::Result<FileSlot> {
        let (parent, suffix) = match &self.opened {
            Some(parent) => (parent, Path::new("")),
            None => (
                &self.ancestor,
                self.binding.suffix().map_err(io::Error::other)?,
            ),
        };
        Ok(FileSlot {
            parent: FileIdentity::capture(&parent.metadata()?),
            leaf: suffix.join(leaf),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EntryStamp {
    identity: FileIdentity,
    length: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    mode: u32,
}

impl EntryStamp {
    fn capture(metadata: &fs::Metadata) -> Self {
        Self {
            identity: FileIdentity::capture(metadata),
            length: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            mode: {
                use std::os::unix::fs::MetadataExt;
                metadata.mode()
            },
        }
    }
}

#[derive(Clone, Debug)]
struct Observation {
    file: Arc<File>,
    stamp: EntryStamp,
}

impl Observation {
    #[cfg(target_os = "linux")]
    fn capture(
        directory: &Directory,
        leaf: &std::ffi::OsStr,
        read: bool,
    ) -> io::Result<Option<Self>> {
        use rustix::fs::{Mode, OFlags};
        let directory = match directory.bind() {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let access = if read {
            OFlags::RDONLY | OFlags::NONBLOCK
        } else {
            OFlags::PATH
        };
        let file = match rustix::fs::openat(
            directory.handle()?,
            leaf,
            access | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        ) {
            Ok(file) => Arc::new(File::from(file)),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let stamp = EntryStamp::capture(&file.metadata()?);
        Ok(Some(Self { file, stamp }))
    }

    #[cfg(not(target_os = "linux"))]
    fn capture(
        _directory: &Directory,
        _leaf: &std::ffi::OsStr,
        _read: bool,
    ) -> io::Result<Option<Self>> {
        Err(io::Error::other("Bound file publication requires Linux"))
    }

    fn verify(
        expected: Option<&Self>,
        directory: &Directory,
        leaf: &std::ffi::OsStr,
    ) -> io::Result<()> {
        let current = Self::capture(directory, leaf, false)?;
        if current.as_ref().map(|entry| &entry.stamp) != expected.map(|entry| &entry.stamp) {
            return Err(io::Error::other(
                "File destination entry changed; choose the path again",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct BoundDestination {
    requested: PathBuf,
    physical: PathBuf,
    referring: Directory,
    directory: Directory,
    referring_leaf: OsString,
    leaf: OsString,
    reference: Option<Observation>,
    existing: Option<Observation>,
    policy: Policy,
    followed: bool,
    slots: Vec<FileSlot>,
}

impl BoundDestination {
    pub fn capture(path: &Path, policy: Policy) -> io::Result<Self> {
        let requested = std::path::absolute(path)?;
        let referring_leaf = requested
            .file_name()
            .ok_or_else(|| io::Error::other("File destination has no basename"))?
            .to_owned();
        let referring = Directory::capture(
            requested
                .parent()
                .ok_or_else(|| io::Error::other("File destination has no parent"))?,
        )?;
        let reference = Observation::capture(&referring, &referring_leaf, false)?;
        let reference_metadata = reference
            .as_ref()
            .map(|entry| entry.file.metadata())
            .transpose()?;
        let followed = policy == Policy::FollowSource
            && reference_metadata.is_some_and(|metadata| metadata.file_type().is_symlink());
        let target = if followed {
            requested.canonicalize()?
        } else {
            requested.clone()
        };
        let leaf = target
            .file_name()
            .ok_or_else(|| io::Error::other("File destination target has no basename"))?
            .to_owned();
        let directory = if followed {
            Directory::capture(
                target
                    .parent()
                    .ok_or_else(|| io::Error::other("File destination target has no parent"))?,
            )?
        } else {
            referring.clone()
        };
        let existing = Observation::capture(&directory, &leaf, false)?;
        if let Some(existing) = &existing {
            let metadata = existing.file.metadata()?;
            if !metadata.is_file()
                && !(policy == Policy::ReplaceEntry && metadata.file_type().is_symlink())
            {
                return Err(io::Error::other("File destination must be a regular file"));
            }
        }
        let physical = directory.physical.join(&leaf);
        let slots = vec![referring.slot(&referring_leaf)?, directory.slot(&leaf)?];
        let destination = Self {
            requested,
            physical,
            referring,
            directory,
            referring_leaf,
            leaf,
            reference,
            existing,
            policy,
            followed,
            slots,
        };
        destination.verify()?;
        Ok(destination)
    }

    pub fn authored_path(&self) -> &Path {
        &self.requested
    }

    pub fn physical_path(&self) -> &Path {
        &self.physical
    }

    pub fn claim_slots(&self) -> Vec<FileSlot> {
        self.slots.clone()
    }

    pub fn file_identities(&self) -> Vec<FileIdentity> {
        self.reference
            .iter()
            .chain(&self.existing)
            .map(|entry| entry.stamp.identity)
            .collect()
    }

    pub fn verify(&self) -> io::Result<()> {
        self.referring.verify()?;
        self.directory.verify()?;
        if self.followed && self.requested.canonicalize()? != self.physical {
            return Err(io::Error::other("Source file symlink changed its target"));
        }
        Observation::verify(
            self.reference.as_ref(),
            &self.referring,
            &self.referring_leaf,
        )?;
        Observation::verify(self.existing.as_ref(), &self.directory, &self.leaf)
    }

    pub fn read_existing(&self) -> io::Result<Option<Vec<u8>>> {
        self.read_existing_with(|| {})
    }

    fn read_existing_with(&self, mut before_read: impl FnMut()) -> io::Result<Option<Vec<u8>>> {
        self.verify()?;
        let Some(existing) = &self.existing else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        #[cfg(unix)]
        {
            use std::io::Read;
            let reader = Observation::capture(&self.directory, &self.leaf, true)?
                .ok_or_else(|| io::Error::other("File destination disappeared while reading"))?;
            if reader.stamp != existing.stamp {
                return Err(io::Error::other("File destination changed while opening"));
            }
            let maximum = existing
                .stamp
                .length
                .checked_add(1)
                .ok_or_else(|| io::Error::other("File destination byte limit overflow"))?;
            let size = usize::try_from(existing.stamp.length).map_err(|error| {
                io::Error::other(format!(
                    "File destination size cannot fit in memory: {error}"
                ))
            })?;
            bytes.try_reserve_exact(size).map_err(|error| {
                io::Error::other(format!(
                    "Could not allocate {size} bytes for file destination: {error}"
                ))
            })?;
            let mut bounded = reader.file.as_ref().take(maximum);
            let mut buffer = [0u8; 64 * 1024];
            loop {
                before_read();
                let count = bounded.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                if count > size - bytes.len() {
                    return Err(io::Error::other("File destination grew while reading"));
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            if EntryStamp::capture(&reader.file.metadata()?) != existing.stamp
                || bytes.len() as u64 != existing.stamp.length
            {
                return Err(io::Error::other("File destination changed while reading"));
            }
        }
        #[cfg(not(unix))]
        return Err(io::Error::other("Bound file reads require Linux"));
        self.verify()?;
        Ok(Some(bytes))
    }

    pub fn stage(&self) -> io::Result<StagedFile> {
        self.verify()?;
        let directory = self.directory.bind()?;
        let mut destination = self.clone();
        destination.directory = directory;
        if destination.referring.physical == destination.directory.physical {
            destination.referring.opened = destination.directory.opened.clone();
        }
        destination.slots = vec![
            destination.referring.slot(&destination.referring_leaf)?,
            destination.directory.slot(&destination.leaf)?,
        ];
        destination.verify()?;
        StagedFile::new(destination)
    }

    fn published(&self, observation: Observation) -> Self {
        let mut destination = self.clone();
        if !destination.followed {
            destination.reference = Some(observation.clone());
        }
        destination.existing = Some(observation);
        destination
    }
}

#[derive(Debug)]
pub struct StagedFile {
    temporary: tempfile::NamedTempFile,
    destination: BoundDestination,
    basename: OsString,
    committed: bool,
}

impl StagedFile {
    #[cfg(target_os = "linux")]
    fn new(destination: BoundDestination) -> io::Result<Self> {
        use rustix::fs::{Mode, OFlags};
        let permissions = if destination.policy == Policy::FollowSource {
            destination
                .existing
                .as_ref()
                .map(|entry| entry.file.metadata().map(|metadata| metadata.permissions()))
                .transpose()?
        } else {
            None
        };
        if permissions.as_ref().is_some_and(fs::Permissions::readonly) {
            return Err(io::Error::other("File destination is read-only"));
        }
        let temporary = tempfile::Builder::new()
            .prefix(".hammerr-publish-")
            .disable_cleanup(true)
            .make_in(&destination.directory.requested, |path| {
                let basename = path
                    .file_name()
                    .ok_or_else(|| io::Error::other("File staging basename is missing"))?;
                if basename == destination.leaf {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "Staging name matches the destination basename",
                    ));
                }
                rustix::fs::openat(
                    destination.directory.handle()?,
                    basename,
                    OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::RDWR
                        | OFlags::CLOEXEC
                        | OFlags::NOFOLLOW,
                    Mode::RUSR | Mode::WUSR,
                )
                .map(File::from)
                .map_err(io::Error::from)
            })?;
        let basename = temporary
            .path()
            .file_name()
            .ok_or_else(|| io::Error::other("File staging basename is missing"))?
            .to_owned();
        let stage = Self {
            temporary,
            destination,
            basename,
            committed: false,
        };
        if let Some(permissions) = permissions {
            stage.file().set_permissions(permissions)?;
        }
        Ok(stage)
    }

    #[cfg(not(target_os = "linux"))]
    fn new(_destination: BoundDestination) -> io::Result<Self> {
        Err(io::Error::other("Bound file staging requires Linux"))
    }

    pub fn file(&self) -> &File {
        self.temporary.as_file()
    }

    pub fn file_mut(&mut self) -> &mut File {
        self.temporary.as_file_mut()
    }

    pub fn sync_all(&self) -> io::Result<()> {
        self.file().sync_all()
    }

    #[cfg(target_os = "linux")]
    fn owned(&self) -> io::Result<bool> {
        let opened = FileIdentity::capture(&self.file().metadata()?);
        match rustix::fs::statat(
            self.destination.directory.handle()?,
            &self.basename,
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(current) => Ok((current.st_dev, current.st_ino) == (opened.device, opened.inode)),
            Err(rustix::io::Errno::NOENT) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub fn commit(self) -> io::Result<CommittedReceipt> {
        self.commit_with(|| {}, || {})
    }

    #[cfg(target_os = "linux")]
    fn commit_with(
        mut self,
        after_verify: impl FnOnce(),
        after_rename: impl FnOnce(),
    ) -> io::Result<CommittedReceipt> {
        self.sync_all()?;
        self.destination.verify()?;
        let file = Arc::new(self.file().try_clone()?);
        let stamp = EntryStamp::capture(&file.metadata()?);
        let destination = self.destination.published(Observation { file, stamp });
        after_verify();
        if !self.owned()? {
            return Err(io::Error::other(
                "File staging name was replaced; unrelated entry preserved",
            ));
        }
        rustix::fs::renameat(
            self.destination.directory.handle()?,
            &self.basename,
            self.destination.directory.handle()?,
            &self.destination.leaf,
        )?;
        self.committed = true;
        after_rename();
        let warning = destination.verify().err().map(|error| {
            format!(
                "File was published, but its path binding changed; choose the path again: {error}"
            )
        });
        Ok(CommittedReceipt {
            destination,
            warning,
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn commit_with(
        self,
        _after_verify: impl FnOnce(),
        _after_rename: impl FnOnce(),
    ) -> io::Result<CommittedReceipt> {
        Err(io::Error::other("Bound file publication requires Linux"))
    }
}

impl Write for StagedFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.file_mut().write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file_mut().flush()
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if !self.committed
            && matches!(self.owned(), Ok(true))
            && let Ok(parent) = self.destination.directory.handle()
        {
            let _ = rustix::fs::unlinkat(parent, &self.basename, rustix::fs::AtFlags::empty());
        }
    }
}

#[derive(Debug)]
pub struct CommittedReceipt {
    pub destination: BoundDestination,
    pub warning: Option<String>,
}

impl DirectoryBinding {
    pub fn requested(&self) -> &Path {
        &self.requested
    }

    pub fn capture(
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

    pub fn verify(&self) -> Result<(), String> {
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

    pub fn physical_ancestor(&self) -> &Path {
        &self.physical
    }

    pub fn suffix(&self) -> Result<&Path, String> {
        self.requested
            .strip_prefix(&self.ancestor)
            .map_err(|error| error.to_string())
    }

    pub fn matches_ancestor(&self, metadata: &fs::Metadata) -> bool {
        let same = metadata.is_dir();
        #[cfg(unix)]
        let same = {
            use std::os::unix::fs::MetadataExt;
            same && (metadata.dev(), metadata.ino()) == self.identity
        };
        same
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
