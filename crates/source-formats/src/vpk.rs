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

        if header_len + tree_len as u64 > file.metadata()?.len() {
            return Err(invalid("VPK directory tree lies outside the file"));
        }
        file.seek(SeekFrom::Start(header_len))?;
        let mut tree = vec![0u8; tree_len];
        file.read_exact(&mut tree)?;
        let entries = Self::parse_tree(&tree)?;
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

    fn parse_tree(tree: &[u8]) -> io::Result<HashMap<String, Entry>> {
        let mut cur = Cursor { remaining: tree };
        let mut entries = HashMap::new();
        loop {
            let ext = cur.cstr()?;
            if ext.is_empty() {
                break;
            }
            loop {
                let dir = cur.cstr()?;
                if dir.is_empty() {
                    break;
                }
                loop {
                    let name = cur.cstr()?;
                    if name.is_empty() {
                        break;
                    }
                    let crc = cur.u32()?;
                    let preload_len = cur.u16()? as usize;
                    let archive = cur.u16()?;
                    let offset = cur.u32()?;
                    let length = cur.u32()?;
                    if cur.u16()? != ENTRY_TERMINATOR {
                        return Err(invalid("corrupt VPK directory entry"));
                    }
                    let preload = cur.take(preload_len)?.into();

                    let mut path = String::with_capacity(dir.len() + name.len() + ext.len() + 2);
                    if dir != " " {
                        path.push_str(dir);
                        path.push('/');
                    }
                    path.push_str(name);
                    if ext != " " {
                        path.push('.');
                        path.push_str(ext);
                    }
                    entries.insert(
                        crate::gamefs::asset_relative(&path)
                            .map_err(|error| invalid(error.to_string()))?,
                        Entry {
                            crc,
                            preload,
                            archive,
                            offset,
                            length,
                        },
                    );
                }
            }
        }
        Ok(entries)
    }

    pub fn path(&self) -> &Path {
        &self.dir_path
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

    pub fn read_limited(&self, path: &str, maximum: usize) -> io::Result<Option<Vec<u8>>> {
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
        let mut data = entry.preload.to_vec();
        if entry.length > 0 {
            let (archive_path, start) = self.data_location(entry);
            let mut file = File::open(&archive_path)?;
            let file_length = file.metadata()?.len();
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
            let mut chunk = file.take(u64::from(entry.length));
            chunk.read_to_end(&mut data)?;
            if data.len() != total {
                return Err(invalid(format!(
                    "{} is truncated reading {path}",
                    archive_path.display()
                )));
            }
        }
        Ok(Some(data))
    }

    /// Physical files used by the same cached entry mapping as [`Self::read_limited`].
    /// A changed index requires reloading the mount before provenance can be trusted.
    pub fn asset_source_paths(&self, path: &str) -> io::Result<Option<Vec<PathBuf>>> {
        if index_identity(&fs::metadata(&self.dir_path)?) != self.index_identity {
            return Err(invalid(
                "VPK index changed since mounting; reload mounted content",
            ));
        }
        let Some(entry) = self.entries.get(path) else {
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
            let mut name = self.archive_stem.as_os_str().to_owned();
            name.push(format!("_{:03}.vpk", entry.archive));
            (PathBuf::from(name), u64::from(entry.offset))
        }
    }

    /// The CRC32 recorded for a file, for integrity checks.
    pub fn crc(&self, path: &str) -> Option<u32> {
        self.entries.get(path).map(|e| e.crc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(root: &Path, archive: u16, body: &[u8]) -> PathBuf {
        let mut tree = b"txt\0materials\0asset\0".to_vec();
        tree.extend(0u32.to_le_bytes());
        tree.extend(3u16.to_le_bytes());
        tree.extend(archive.to_le_bytes());
        tree.extend(2u32.to_le_bytes());
        tree.extend((body.len() as u32).to_le_bytes());
        tree.extend(ENTRY_TERMINATOR.to_le_bytes());
        tree.extend(b"pre\0\0\0");
        let mut bytes = SIGNATURE.to_le_bytes().to_vec();
        bytes.extend(1u32.to_le_bytes());
        bytes.extend((tree.len() as u32).to_le_bytes());
        bytes.extend(tree);
        if archive == DIR_ARCHIVE {
            bytes.extend(b"xx");
            bytes.extend(body);
        } else if !body.is_empty() {
            let mut data = b"xx".to_vec();
            data.extend(body);
            fs::write(root.join(format!("test_{archive:03}.vpk")), data).unwrap();
        }
        let path = root.join("test_dir.vpk");
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn asset_sources_match_inline_numbered_and_preload_only_reads() {
        for (archive, body) in [(DIR_ARCHIVE, b"body".as_slice()), (42, b"body"), (9, b"")] {
            let root = tempfile::tempdir().unwrap();
            let index = fixture(root.path(), archive, body);
            let pak = Vpk::open(&index).unwrap();
            let paths = pak
                .asset_source_paths("materials/asset.txt")
                .unwrap()
                .unwrap();
            let expected = if archive == 42 {
                vec![index, root.path().join("test_042.vpk")]
            } else {
                vec![index]
            };
            for path in expected {
                assert!(paths.contains(&path));
                assert!(paths.contains(&path.canonicalize().unwrap()));
            }
            assert_eq!(
                pak.read("materials/asset.txt").unwrap().unwrap(),
                [b"pre".as_slice(), body].concat()
            );
            assert!(pak.asset_source_paths("absent").unwrap().is_none());
        }
    }

    #[test]
    fn changed_index_rejects_cached_present_and_absent_entry_provenance() {
        let root = tempfile::tempdir().unwrap();
        let index = fixture(root.path(), DIR_ARCHIVE, b"body");
        let pak = Vpk::open(&index).unwrap();
        let replacement = root.path().join("replacement.vpk");
        fs::write(&replacement, fs::read(&index).unwrap()).unwrap();
        fs::rename(replacement, index).unwrap();
        for asset in ["materials/asset.txt", "absent"] {
            assert!(
                pak.asset_source_paths(asset)
                    .unwrap_err()
                    .to_string()
                    .contains("reload mounted content")
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn asset_sources_keep_vpk_aliases_and_their_physical_targets() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let index = fixture(root.path(), 42, b"body");
        let alias = root.path().join("alias_dir.vpk");
        let archive = root.path().join("alias_042.vpk");
        symlink("test_dir.vpk", &alias).unwrap();
        symlink("test_042.vpk", &archive).unwrap();
        let pak = Vpk::open(&alias).unwrap();
        let paths = pak
            .asset_source_paths("materials/asset.txt")
            .unwrap()
            .unwrap();
        for path in [alias, archive, index, root.path().join("test_042.vpk")] {
            assert!(paths.contains(&path), "{}", path.display());
        }
        assert_eq!(
            pak.read("materials/asset.txt").unwrap().unwrap(),
            b"prebody"
        );
    }

    #[test]
    fn bounded_reads_reject_declared_sizes_before_opening_an_archive() {
        let file = tempfile::tempfile().unwrap();
        let pak = Vpk {
            dir_path: "missing_dir.vpk".into(),
            archive_stem: "missing".into(),
            dir_data_start: 0,
            index_identity: index_identity(&file.metadata().unwrap()),
            entries: HashMap::from([(
                "models/large.mdl".into(),
                Entry {
                    crc: 0,
                    preload: Box::new([]),
                    archive: 0,
                    offset: 0,
                    length: u32::MAX,
                },
            )]),
        };
        let error = pak.read_limited("models/large.mdl", 64).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("limit"));
    }
}
