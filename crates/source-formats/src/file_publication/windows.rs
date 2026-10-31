use std::fs::OpenOptions;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetFileInformationByHandle, MoveFileW,
};

use super::*;

pub fn rename_noreplace(source: &Path, destination: &Path) -> io::Result<()> {
    let mut source: Vec<u16> = source.as_os_str().encode_wide().collect();
    let mut destination: Vec<u16> = destination.as_os_str().encode_wide().collect();
    if source.contains(&0) || destination.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "File paths cannot contain NUL",
        ));
    }
    source.push(0);
    destination.push(0);
    if unsafe { MoveFileW(source.as_ptr(), destination.as_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn file_identity(file: &File) -> io::Result<FileIdentity> {
    let mut information = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    let result =
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    let information = unsafe { information.assume_init() };
    Ok(FileIdentity {
        device: u64::from(information.dwVolumeSerialNumber),
        inode: (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    })
}

pub fn open_directory(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if !file.metadata()?.is_dir() {
        return Err(io::Error::other("Expected a regular directory"));
    }
    Ok(file)
}

pub fn open_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

pub fn lock_file(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Coordination lock must be a regular file"));
    }
    file.try_lock().map_err(io::Error::other)?;
    Ok(file)
}

impl Directory {
    pub(super) fn capture(path: &Path) -> io::Result<Self> {
        let binding = DirectoryBinding::capture(path, "File destination", "choose the path again")
            .map_err(io::Error::other)?;
        let ancestor = Arc::new(open_directory(binding.physical_ancestor())?);
        if file_identity(&ancestor)? != binding.identity {
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

    pub(super) fn bind(&self) -> io::Result<Self> {
        self.verify()?;
        let directory = Self {
            opened: Some(Arc::new(open_directory(&self.physical)?)),
            ..self.clone()
        };
        directory.verify()?;
        Ok(directory)
    }
}

impl Observation {
    pub(super) fn capture(
        directory: &Directory,
        leaf: &std::ffi::OsStr,
        read: bool,
    ) -> io::Result<Option<Self>> {
        let directory = match directory.bind() {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let path = directory.physical.join(leaf);
        let mut options = OpenOptions::new();
        if read {
            options.read(true);
        } else {
            options.access_mode(FILE_READ_ATTRIBUTES);
        }
        let file = match options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
        {
            Ok(file) => Arc::new(file),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let stamp = EntryStamp::from_file(&file)?;
        Ok(Some(Self { file, stamp }))
    }
}

impl StagedFile {
    pub(super) fn new(destination: BoundDestination) -> io::Result<Self> {
        destination.verify()?;
        if destination
            .existing
            .as_ref()
            .map(|entry| entry.file.metadata())
            .transpose()?
            .is_some_and(|metadata| metadata.permissions().readonly())
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "File destination is read-only",
            ));
        }
        let temporary = tempfile::Builder::new()
            .prefix(".hammerr-publish-")
            .disable_cleanup(true)
            .tempfile_in(&destination.directory.physical)?;
        let basename = temporary
            .path()
            .file_name()
            .ok_or_else(|| io::Error::other("File staging basename is missing"))?
            .to_owned();
        Ok(Self {
            temporary,
            destination,
            basename,
            committed: false,
        })
    }

    pub(super) fn owned(&self) -> io::Result<bool> {
        match open_file(self.temporary.path()) {
            Ok(file) => Ok(file_identity(&file)? == file_identity(self.file())?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
}
