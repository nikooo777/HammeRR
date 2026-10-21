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

    pub(crate) fn verify_index(&self) -> io::Result<()> {
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

    #[test]
    fn changed_index_rejects_plain_reads_for_inline_numbered_and_preload_entries() {
        for archive in [DIR_ARCHIVE, 42, 9] {
            let root = tempfile::tempdir().unwrap();
            let replacement_root = tempfile::tempdir().unwrap();
            let old_body = if archive == 9 {
                b"".as_slice()
            } else {
                b"old0"
            };
            let new_body = if archive == 9 {
                b"".as_slice()
            } else {
                b"new0"
            };
            let index = fixture(root.path(), archive, old_body);
            let pak = Vpk::open(&index).unwrap();
            assert_eq!(
                pak.read_limited("materials/asset.txt", 7).unwrap().unwrap(),
                [b"pre".as_slice(), old_body].concat()
            );
            assert!(
                pak.read_limited("materials/absent.txt", 7)
                    .unwrap()
                    .is_none()
            );
            let replacement = fixture(replacement_root.path(), archive, new_body);
            let mut bytes = fs::read(&replacement).unwrap();
            let preload = bytes.windows(3).position(|part| part == b"pre").unwrap();
            bytes[preload..preload + 3].copy_from_slice(b"new");
            fs::write(&replacement, bytes).unwrap();
            if archive == 42 {
                fs::copy(
                    replacement_root.path().join("test_042.vpk"),
                    root.path().join("test_042.vpk"),
                )
                .unwrap();
            }
            fs::rename(&replacement, &index).unwrap();
            let reloaded = Vpk::open(&index).unwrap();
            assert_eq!(
                reloaded
                    .read_limited("materials/asset.txt", 7)
                    .unwrap()
                    .unwrap(),
                [b"new".as_slice(), new_body].concat()
            );
            for asset in ["materials/asset.txt", "materials/absent.txt"] {
                let error = pak.read_limited(asset, 7).unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::InvalidData);
                assert!(error.to_string().contains("reload mounted content"));
            }
        }
    }

    #[test]
    fn changed_index_rejects_a_new_entry_missing_from_the_cached_tree() {
        let root = tempfile::tempdir().unwrap();
        let replacement_root = tempfile::tempdir().unwrap();
        let index = fixture(root.path(), DIR_ARCHIVE, b"old0");
        let pak = Vpk::open(&index).unwrap();
        assert!(pak.read("materials/fresh.txt").unwrap().is_none());
        let replacement = fixture(replacement_root.path(), DIR_ARCHIVE, b"new0");
        let mut bytes = fs::read(&replacement).unwrap();
        let name = bytes.windows(5).position(|part| part == b"asset").unwrap();
        bytes[name..name + 5].copy_from_slice(b"fresh");
        fs::write(&replacement, bytes).unwrap();
        fs::rename(&replacement, &index).unwrap();
        assert_eq!(
            Vpk::open(&index)
                .unwrap()
                .read("materials/fresh.txt")
                .unwrap()
                .unwrap(),
            b"prenew0"
        );
        let error = pak.read("materials/fresh.txt").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("reload mounted content"));
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
        let file = tempfile::NamedTempFile::new().unwrap();
        for (preload, length) in [(b"".as_slice(), u32::MAX), (b"pre", 64)] {
            let pak = Vpk {
                dir_path: file.path().to_owned(),
                archive_stem: file.path().with_extension("absent-archive"),
                dir_data_start: 0,
                index_identity: index_identity(&file.as_file().metadata().unwrap()),
                entries: HashMap::from([(
                    "models/large.mdl".into(),
                    Entry {
                        crc: 0,
                        preload: preload.into(),
                        archive: 0,
                        offset: 0,
                        length,
                    },
                )]),
            };
            for error in [
                pak.read_limited("models/large.mdl", 64).unwrap_err(),
                pak.asset_source_paths_limited("models/large.mdl", 64)
                    .unwrap_err(),
            ] {
                assert_eq!(error.kind(), io::ErrorKind::InvalidData);
                assert_eq!(
                    error.to_string(),
                    "VPK entry exceeds the caller's byte read limit"
                );
            }
        }
    }
}
