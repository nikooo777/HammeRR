//! Valve Pak (VPK) archives, versions 1 and 2.
//!
//! A pak is a `<name>_dir.vpk` index plus numbered `<name>_NNN.vpk` data
//! archives. Paths inside are lowercase with forward slashes.

use std::collections::HashMap;
use std::ffi::CStr;
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

const SIGNATURE: u32 = 0x55aa_1234;
/// `archive_index` value meaning the data follows the index in the dir file.
const DIR_ARCHIVE: u16 = 0x7fff;
const ENTRY_TERMINATOR: u16 = 0xffff;
pub const MAX_DIRECTORY_TREE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_DIRECTORY_PATH_BYTES: usize = 1024;
pub const MAX_DIRECTORY_ENTRIES: usize = 1_000_000;
pub const MAX_DIRECTORY_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone)]
struct Entry {
    crc: u32,
    preload: Box<[u8]>,
    archive: u16,
    offset: u32,
    length: u32,
}

#[derive(Debug)]
pub struct Vpk {
    dir_path: PathBuf,
    /// `<dir>/<name>`, to which `_NNN.vpk` is appended for data archives.
    archive_stem: PathBuf,
    /// Where `DIR_ARCHIVE` entry offsets start in the dir file.
    dir_data_start: u64,
    entries: HashMap<String, Entry>,
    directory_bytes: usize,
    index_identity: IndexIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IndexIdentity {
    length: u64,
    modified: Option<SystemTime>,
    #[cfg(not(unix))]
    created: Option<SystemTime>,
    #[cfg(unix)]
    unix: (u64, u64, i64, i64),
}

fn index_identity(metadata: &Metadata) -> IndexIdentity {
    IndexIdentity {
        length: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(not(unix))]
        created: metadata.created().ok(),
        #[cfg(unix)]
        unix: (
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        ),
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

struct Cursor<'a> {
    remaining: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let (bytes, remaining) = self
            .remaining
            .split_at_checked(n)
            .ok_or_else(|| invalid("VPK directory tree is truncated"))?;
        self.remaining = remaining;
        Ok(bytes)
    }

    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn cstr(&mut self) -> io::Result<&'a str> {
        let text = CStr::from_bytes_until_nul(self.remaining)
            .map_err(|_| invalid("unterminated string in VPK directory tree"))?;
        self.take(text.to_bytes_with_nul().len())?;
        text.to_str().map_err(|_| invalid("non-UTF-8 path in VPK"))
    }
}

impl Vpk {
    /// Opens a pak by its dir file (`x_dir.vpk`) or by its logical name
    /// (`x.vpk`), as written in `gameinfo.txt`.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let dir_path = Self::dir_file_for(path);
        let file_name = dir_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| invalid("VPK path has no file name"))?;
        let stem = file_name.strip_suffix("_dir.vpk").unwrap_or(file_name);
        let archive_stem = dir_path.with_file_name(stem);

        let mut file = File::open(&dir_path)?;
        let identity = index_identity(&file.metadata()?);
        let mut header = [0u8; 12];
        file.read_exact(&mut header)?;
        let word = |i: usize| u32::from_le_bytes(header[i * 4..i * 4 + 4].try_into().unwrap());
        if word(0) != SIGNATURE {
            return Err(invalid(format!("{} is not a VPK file", dir_path.display())));
        }
        let header_len = match word(1) {
            1 => 12,
            2 => 28,
            v => return Err(invalid(format!("unsupported VPK version {v}"))),
        };
        let tree_len = word(2) as usize;

        if tree_len > MAX_DIRECTORY_TREE_BYTES {
            return Err(invalid("VPK directory tree exceeds 64 MiB"));
        }

        if header_len + tree_len as u64 > file.metadata()?.len() {
            return Err(invalid("VPK directory tree lies outside the file"));
        }
        file.seek(SeekFrom::Start(header_len))?;
        let mut tree = Vec::new();
        tree.try_reserve_exact(tree_len)
            .map_err(|_| invalid("VPK directory tree allocation failed"))?;
        tree.resize(tree_len, 0);
        file.read_exact(&mut tree)?;
        let (entries, directory_bytes) = Self::parse_tree(&tree)?;
        if index_identity(&file.metadata()?) != identity
            || index_identity(&fs::metadata(&dir_path)?) != identity
        {
            return Err(invalid(
                "VPK index changed while opening it; reload mounted content",
            ));
        }

        Ok(Self {
            dir_path,
            archive_stem,
            dir_data_start: header_len + tree_len as u64,
            entries,
            directory_bytes,
            index_identity: identity,
        })
    }

    /// `x.vpk` -> `x_dir.vpk`, unless the path already names a dir file.
    pub fn dir_file_for(path: &Path) -> PathBuf {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        match name.strip_suffix(".vpk") {
            Some(stem) if !stem.ends_with("_dir") => path.with_file_name(format!("{stem}_dir.vpk")),
            _ => path.to_path_buf(),
        }
    }

    fn parse_tree(tree: &[u8]) -> io::Result<(HashMap<String, Entry>, usize)> {
        let mut cur = Cursor { remaining: tree };
        let mut entries = HashMap::new();
        let mut directory_bytes = 0usize;
        let mut entry_count = 0usize;
        loop {
            let ext = cur.cstr()?;
            if ext.is_empty() {
                break;
            }
            if ext.len() > MAX_DIRECTORY_PATH_BYTES {
                return Err(invalid("VPK directory path exceeds 1024 bytes"));
            }
            loop {
                let dir = cur.cstr()?;
                if dir.is_empty() {
                    break;
                }
                if dir.len() > MAX_DIRECTORY_PATH_BYTES {
                    return Err(invalid("VPK directory path exceeds 1024 bytes"));
                }
                loop {
                    let name = cur.cstr()?;
                    if name.is_empty() {
                        break;
                    }
                    let path_len = name
                        .len()
                        .checked_add(if dir == " " { 0 } else { dir.len() + 1 })
                        .and_then(|bytes| {
                            bytes.checked_add(if ext == " " { 0 } else { ext.len() + 1 })
                        })
                        .filter(|&bytes| bytes <= MAX_DIRECTORY_PATH_BYTES)
                        .ok_or_else(|| invalid("VPK directory path exceeds 1024 bytes"))?;
                    entry_count += 1;
                    if entry_count > MAX_DIRECTORY_ENTRIES {
                        return Err(invalid("VPK directory exceeds 1000000 entries"));
                    }
                    let crc = cur.u32()?;
                    let preload_len = cur.u16()? as usize;
                    let archive = cur.u16()?;
                    let offset = cur.u32()?;
                    let length = cur.u32()?;
                    if cur.u16()? != ENTRY_TERMINATOR {
                        return Err(invalid("corrupt VPK directory entry"));
                    }
                    directory_bytes = directory_bytes
                        .checked_add(path_len)
                        .and_then(|bytes| bytes.checked_add(preload_len))
                        .and_then(|bytes| bytes.checked_add(size_of::<(String, Entry)>()))
                        .filter(|&bytes| bytes <= MAX_DIRECTORY_BYTES)
                        .ok_or_else(|| invalid("VPK directory metadata exceeds 128 MiB"))?;
                    let mut preload = Vec::new();
                    preload
                        .try_reserve_exact(preload_len)
                        .map_err(|_| invalid("VPK preload allocation failed"))?;
                    preload.extend_from_slice(cur.take(preload_len)?);

                    let mut path = String::new();
                    path.try_reserve_exact(path_len)
                        .map_err(|_| invalid("VPK path allocation failed"))?;
                    if dir != " " {
                        path.push_str(dir);
                        path.push('/');
                    }
                    path.push_str(name);
                    if ext != " " {
                        path.push('.');
                        path.push_str(ext);
                    }
                    let path = crate::gamefs::asset_relative(&path)
                        .map_err(|error| invalid(error.to_string()))?;
                    if !entries.contains_key(&path) {
                        entries
                            .try_reserve(1)
                            .map_err(|_| invalid("VPK directory entry allocation failed"))?;
                    }
                    entries.insert(
                        path,
                        Entry {
                            crc,
                            preload: preload.into_boxed_slice(),
                            archive,
                            offset,
                            length,
                        },
                    );
                }
            }
        }
        Ok((entries, directory_bytes))
    }

    /// Directory metadata charge, including repeated entries before replacement.
    pub fn directory_bytes(&self) -> usize {
        self.directory_bytes
    }

    pub fn path(&self) -> &Path {
        &self.dir_path
    }

    pub(crate) fn archive_path(&self, archive: u16) -> PathBuf {
        let mut name = self.archive_stem.as_os_str().to_owned();
        name.push(format!("_{archive:03}.vpk"));
        PathBuf::from(name)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(path)
    }

    /// Paths of all files, in no particular order.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Reads a file by its normalized path (lowercase, `/` separators).
    pub fn read(&self, path: &str) -> io::Result<Option<Vec<u8>>> {
        self.read_limited(path, usize::MAX)
    }

    fn bounded_entry(&self, path: &str, maximum: usize) -> io::Result<Option<(&Entry, usize)>> {
        self.verify_index()?;
        let Some(entry) = self.entries.get(path) else {
            return Ok(None);
        };
        let total = entry
            .preload
            .len()
            .checked_add(entry.length as usize)
            .ok_or_else(|| invalid("VPK entry length overflow"))?;
        if total > maximum {
            return Err(invalid("VPK entry exceeds the caller's byte read limit"));
        }
        Ok(Some((entry, total)))
    }

    pub fn read_limited(&self, path: &str, maximum: usize) -> io::Result<Option<Vec<u8>>> {
        let Some((entry, total)) = self.bounded_entry(path, maximum)? else {
            return Ok(None);
        };
        let mut data = entry.preload.to_vec();
        if entry.length > 0 {
            let (archive_path, start) = self.data_location(entry);
            let mut file = File::open(&archive_path)?;
            let metadata = file.metadata()?;
            if entry.archive == DIR_ARCHIVE {
                self.verify_index_metadata(&metadata)?;
            }
            let file_length = metadata.len();
            if start
                .checked_add(u64::from(entry.length))
                .is_none_or(|end| end > file_length)
            {
                return Err(invalid(format!(
                    "VPK entry {path} lies outside {}",
                    archive_path.display()
                )));
            }
            data.try_reserve_exact(entry.length as usize)
                .map_err(|error| invalid(format!("cannot allocate VPK entry {path}: {error}")))?;
            file.seek(SeekFrom::Start(start))?;
            (&mut file)
                .take(u64::from(entry.length))
                .read_to_end(&mut data)?;
            if entry.archive == DIR_ARCHIVE {
                self.verify_index_metadata(&file.metadata()?)?;
            }
            if data.len() != total {
                return Err(invalid(format!(
                    "{} is truncated reading {path}",
                    archive_path.display()
                )));
            }
        }
        self.verify_index()?;
        Ok(Some(data))
    }

    pub fn verify_index(&self) -> io::Result<()> {
        self.verify_index_metadata(&fs::metadata(&self.dir_path)?)
    }

    fn verify_index_metadata(&self, metadata: &Metadata) -> io::Result<()> {
        if index_identity(metadata) != self.index_identity {
            return Err(invalid(
                "VPK index changed since mounting; reload mounted content",
            ));
        }
        Ok(())
    }

    /// Physical files used by the same cached entry mapping as [`Self::read_limited`].
    /// A changed index requires reloading the mount before provenance can be trusted.
    pub fn asset_source_paths(&self, path: &str) -> io::Result<Option<Vec<PathBuf>>> {
        self.asset_source_paths_limited(path, usize::MAX)
    }

    pub fn asset_source_paths_limited(
        &self,
        path: &str,
        maximum: usize,
    ) -> io::Result<Option<Vec<PathBuf>>> {
        let Some((entry, _)) = self.bounded_entry(path, maximum)? else {
            return Ok(None);
        };
        let mut paths = vec![std::path::absolute(&self.dir_path)?];
        if entry.length > 0 && entry.archive != DIR_ARCHIVE {
            paths.push(std::path::absolute(self.data_location(entry).0)?);
        }
        let physical = paths
            .iter()
            .map(fs::canonicalize)
            .collect::<io::Result<Vec<_>>>()?;
        for path in physical {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        Ok(Some(paths))
    }

    fn data_location(&self, entry: &Entry) -> (PathBuf, u64) {
        if entry.archive == DIR_ARCHIVE {
            (
                self.dir_path.clone(),
                self.dir_data_start + u64::from(entry.offset),
            )
        } else {
            (self.archive_path(entry.archive), u64::from(entry.offset))
        }
    }

    /// The CRC32 recorded for a file, for integrity checks.
    pub fn crc(&self, path: &str) -> Option<u32> {
        self.entries.get(path).map(|e| e.crc)
    }
}
