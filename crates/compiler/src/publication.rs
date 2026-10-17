use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::Control;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    length: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    identity: [u64; 6],
}

fn stamp(metadata: &fs::Metadata) -> Stamp {
    Stamp {
        length: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        identity: {
            use std::os::unix::fs::MetadataExt;
            [
                metadata.dev(),
                metadata.ino(),
                metadata.mtime() as u64,
                metadata.mtime_nsec() as u64,
                metadata.ctime() as u64,
                metadata.ctime_nsec() as u64,
            ]
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIdentity {
    requested: PathBuf,
    path: PathBuf,
    stamp: Option<Stamp>,
    sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    requested: PathBuf,
    path: PathBuf,
    stamp: Option<Stamp>,
    container: bool,
    kind: u8,
}

impl FileStamp {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn same_file(&self, other: &Self) -> bool {
        #[cfg(unix)]
        if let (Some(a), Some(b)) = (&self.stamp, &other.stamp) {
            return a.identity[..2] == b.identity[..2];
        }
        self.path == other.path
    }

    pub fn verify_path(&self, path: &Path) -> io::Result<()> {
        if canonical_destination(path)? != self.path {
            return Err(io::Error::other(
                "Build source or destination alias changed its target",
            ));
        }
        self.verify_metadata()
    }

    pub(crate) fn verify_container_identity(&self) -> io::Result<()> {
        let current = Self::capture_kind(&self.requested, true)?;
        let same = self.path == current.path && self.kind == current.kind;
        #[cfg(unix)]
        let same = same
            && self.stamp.as_ref().map(|stamp| &stamp.identity[..2])
                == current.stamp.as_ref().map(|stamp| &stamp.identity[..2]);
        if !same {
            return Err(io::Error::other(
                "Instance referring directory changed identity",
            ));
        }
        Ok(())
    }

    pub fn capture(path: &Path) -> io::Result<Self> {
        Self::capture_kind(path, false)
    }

    pub(crate) fn capture_container(path: &Path) -> io::Result<Self> {
        Self::capture_kind(path, true)
    }

    fn capture_kind(path: &Path, container: bool) -> io::Result<Self> {
        let requested = std::path::absolute(path)?;
        let path = canonical_destination(path)?;
        let mut kind = 0;
        let current = match fs::symlink_metadata(&path) {
            Ok(metadata)
                if (metadata.is_file() && (container || metadata.len() <= 512 * 1024 * 1024))
                    || (container && (metadata.is_dir() || metadata.file_type().is_symlink())) =>
            {
                kind = if metadata.is_file() {
                    1
                } else if metadata.is_dir() {
                    2
                } else {
                    3
                };
                Some(stamp(&metadata))
            }
            Ok(_) => {
                return Err(io::Error::other(
                    "Build source or destination must be a bounded regular file",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        Ok(Self {
            requested,
            path,
            stamp: current,
            container,
            kind,
        })
    }

    pub fn verify_metadata(&self) -> io::Result<()> {
        if Self::capture_kind(&self.requested, self.container)? != *self {
            return Err(io::Error::other(format!(
                "Build source or destination changed: {}",
                self.requested.display()
            )));
        }
        Ok(())
    }

    pub fn verify_identity(&self, identity: &FileIdentity) -> io::Result<()> {
        self.verify_metadata()?;
        if self.path != identity.path || self.stamp != identity.stamp {
            return Err(io::Error::other(
                "Hashed build identity differs from the initial source/destination snapshot",
            ));
        }
        identity.verify_metadata()
    }

    pub fn verify_lease(&self, lease: &OutputLease) -> io::Result<()> {
        self.verify_identity(&lease.identity)
    }
}

impl FileIdentity {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn capture(path: &Path, control: &Control) -> io::Result<Self> {
        Self::capture_bounded(path, 512 * 1024 * 1024, control)
    }

    pub fn capture_bounded(path: &Path, limit: u64, control: &Control) -> io::Result<Self> {
        Self::read_identity(path, limit, false, control).map(|(identity, _)| identity)
    }

    pub fn read_bounded(path: &Path, limit: u64, control: &Control) -> io::Result<(Self, Vec<u8>)> {
        let result = Self::read_identity(path, limit, true, control)?;
        if result.0.stamp.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Build source is missing",
            ));
        }
        Ok(result)
    }

    fn read_identity(
        path: &Path,
        limit: u64,
        retain: bool,
        control: &Control,
    ) -> io::Result<(Self, Vec<u8>)> {
        let requested = std::path::absolute(path)?;
        let path = canonical_destination(path)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok((
                    Self {
                        requested,
                        path,
                        stamp: None,
                        sha256: None,
                    },
                    Vec::new(),
                ));
            }
            Err(error) => return Err(error),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > limit {
            return Err(io::Error::other(format!(
                "Build source must be a regular file within the {limit}-byte limit"
            )));
        }
        let before = stamp(&metadata);
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(all(target_os = "linux", target_pointer_width = "64"))]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(&path)?;
        if stamp(&file.metadata()?) != before {
            return Err(io::Error::other(
                "Build file changed while opening its identity",
            ));
        }
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut total = 0u64;
        let mut bytes = Vec::new();
        loop {
            control.checkpoint().map_err(io::Error::other)?;
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or_else(|| io::Error::other("Build file size overflow"))?;
            if total > before.length {
                return Err(io::Error::other(
                    "Build file grew while reading its identity",
                ));
            }
            hash.update(&buffer[..count]);
            if retain {
                bytes.extend_from_slice(&buffer[..count]);
            }
        }
        if total != before.length || stamp(&file.metadata()?) != before {
            return Err(io::Error::other(
                "Build file changed while reading its identity",
            ));
        }
        let result = Self {
            requested,
            path,
            stamp: Some(before),
            sha256: Some(format!("{:x}", hash.finalize())),
        };
        result.verify_metadata()?;
        Ok((result, bytes))
    }

    pub fn verify(&self, control: &Control) -> io::Result<()> {
        let limit = self.stamp.as_ref().map_or(0, |stamp| stamp.length);
        if Self::capture_bounded(&self.requested, limit, control)? != *self {
            return Err(io::Error::other(format!(
                "Build source or destination changed: {}",
                self.requested.display()
            )));
        }
        Ok(())
    }

    pub fn sha256(&self) -> Option<&str> {
        self.sha256.as_deref()
    }

    pub fn same_file(&self, other: &Self) -> bool {
        #[cfg(unix)]
        if let (Some(a), Some(b)) = (&self.stamp, &other.stamp) {
            return a.identity[..2] == b.identity[..2];
        }
        self.path == other.path
    }

    pub fn verify_bytes(&self, bytes: &[u8]) -> io::Result<()> {
        let digest = format!("{:x}", Sha256::digest(bytes));
        if self.sha256.as_deref() != Some(digest.as_str())
            || self.stamp.as_ref().map(|stamp| stamp.length) != Some(bytes.len() as u64)
        {
            return Err(io::Error::other(
                "Build file bytes differ from the expected content",
            ));
        }
        Ok(())
    }

    pub fn verify_metadata(&self) -> io::Result<()> {
        if canonical_destination(&self.requested)? != self.path {
            return Err(io::Error::other(
                "Build source or destination parent path changed",
            ));
        }
        let current = match fs::symlink_metadata(&self.path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                Some(stamp(&metadata))
            }
            Ok(_) => {
                return Err(io::Error::other(
                    "Build source or destination became a non-regular file",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if current != self.stamp {
            return Err(io::Error::other(format!(
                "Build source or destination changed: {}",
                self.path.display()
            )));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct OutputLease {
    identity: FileIdentity,
    _lock: File,
}

impl OutputLease {
    pub fn path(&self) -> &Path {
        &self.identity.path
    }

    pub fn acquire(path: &Path, control: &Control) -> io::Result<Self> {
        let destination = canonical_destination(path)?;
        let name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| io::Error::other("Reflection output requires a UTF-8 file name"))?;
        let lock_path = destination.with_file_name(format!(".hammerr-reflection-{name}.lock"));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(all(target_os = "linux", target_pointer_width = "64"))]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let lock = options.open(lock_path)?;
        if !lock.metadata()?.is_file() {
            return Err(io::Error::other(
                "Reflection output lease is not a regular file",
            ));
        }
        #[cfg(all(target_os = "linux", target_pointer_width = "64"))]
        {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err(io::Error::other(
                    "Another reflection build owns this output destination",
                ));
            }
        }
        #[cfg(not(all(target_os = "linux", target_pointer_width = "64")))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Reflection output leases require Linux64",
            ));
        }
        #[allow(unreachable_code)]
        {
            let identity = FileIdentity::capture(path, control)?;
            if identity.path != destination {
                return Err(io::Error::other(
                    "Reflection output parent changed while acquiring its lease",
                ));
            }
            Ok(Self {
                identity,
                _lock: lock,
            })
        }
    }

    pub fn verify(&self, control: &Control) -> io::Result<()> {
        self.identity.verify(control)
    }

    pub(crate) fn verify_commit(&self, destination: &Path) -> io::Result<()> {
        if canonical_destination(destination)? != self.identity.path {
            return Err(io::Error::other(
                "Prepared build output differs from its leased destination",
            ));
        }
        self.identity.verify_metadata()
    }
}

fn canonical_destination(path: &Path) -> io::Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("Build destination has no file name"))?;
    Ok(parent.canonicalize()?.join(name))
}

#[cfg(all(test, target_os = "linux", target_pointer_width = "64"))]
mod tests {
    use super::*;

    #[test]
    fn a_changed_output_or_accepted_cancel_preserves_the_previous_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("map.bsp");
        fs::write(&path, b"previous").unwrap();
        let lease = OutputLease::acquire(&path, &Control::default()).unwrap();
        assert!(OutputLease::acquire(&path, &Control::default()).is_err());
        let mut staged = source_formats::file_publication::BoundDestination::capture(
            &path,
            source_formats::file_publication::Policy::ReplaceEntry,
        )
        .unwrap()
        .stage()
        .unwrap();
        use std::io::Write;
        staged.file_mut().write_all(b"new output").unwrap();
        let prepared = crate::PreparedWrite {
            staged,
            destination: path.clone(),
            success: true,
            instance_dependencies: None,
            instance_output_stamp: None,
        };
        fs::write(&path, b"external replacement").unwrap();
        assert!(lease.verify(&Control::default()).is_err());
        assert!(
            prepared
                .commit_checked(&lease, &Control::default())
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"external replacement");
        drop(lease);
        let control = Control::default();
        let lease = OutputLease::acquire(&path, &control).unwrap();
        let prepared = crate::PreparedWrite {
            staged: source_formats::file_publication::BoundDestination::capture(
                &path,
                source_formats::file_publication::Policy::ReplaceEntry,
            )
            .unwrap()
            .stage()
            .unwrap(),
            destination: path.clone(),
            success: true,
            instance_dependencies: None,
            instance_output_stamp: None,
        };
        control.cancel().unwrap();
        assert!(prepared.commit_checked(&lease, &control).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"external replacement");
    }

    #[test]
    fn frozen_parent_targets_reject_alias_retarget_and_aba_acquisition() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let a = directory.path().join("a");
        let b = directory.path().join("b");
        let alias = directory.path().join("alias");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::write(a.join("map.bsp"), b"source A").unwrap();
        fs::write(b.join("map.bsp"), b"source B").unwrap();
        symlink(&a, &alias).unwrap();
        let stamp = FileStamp::capture(&alias.join("map.bsp")).unwrap();
        fs::remove_file(&alias).unwrap();
        symlink(&b, &alias).unwrap();
        assert!(stamp.verify_path(&alias.join("map.bsp")).is_err());
        let identity_b =
            FileIdentity::capture(&alias.join("map.bsp"), &Control::default()).unwrap();
        fs::remove_file(&alias).unwrap();
        symlink(&a, &alias).unwrap();
        stamp.verify_metadata().unwrap();
        assert!(stamp.verify_identity(&identity_b).is_err());
        let lease = OutputLease::acquire(stamp.path(), &Control::default()).unwrap();
        stamp.verify_lease(&lease).unwrap();
        fs::remove_file(&alias).unwrap();
        symlink(&b, &alias).unwrap();
        assert!(stamp.verify_metadata().is_err());
    }
}
