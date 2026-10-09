//! Valve Pak (VPK) archives, versions 1 and 2.
//!
//! A pak is a `<name>_dir.vpk` index plus numbered `<name>_NNN.vpk` data
//! archives. Paths inside are lowercase with forward slashes.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

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
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let bytes = self
            .data
            .get(self.pos..self.pos + n)
            .ok_or_else(|| invalid("VPK directory tree is truncated"))?;
        self.pos += n;
        Ok(bytes)
    }

    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn cstr(&mut self) -> io::Result<&'a str> {
        let rest = &self.data[self.pos..];
        let len = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| invalid("unterminated string in VPK directory tree"))?;
        self.pos += len + 1;
        std::str::from_utf8(&rest[..len]).map_err(|_| invalid("non-UTF-8 path in VPK"))
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

        file.seek(SeekFrom::Start(header_len))?;
        let mut tree = vec![0u8; tree_len];
        file.read_exact(&mut tree)?;
        let entries = Self::parse_tree(&tree)?;

        Ok(Self {
            dir_path,
            archive_stem,
            dir_data_start: header_len + tree_len as u64,
            entries,
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
        let mut cur = Cursor { data: tree, pos: 0 };
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
                        path.to_ascii_lowercase(),
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
        let Some(entry) = self.entries.get(path) else {
            return Ok(None);
        };
        let mut data = Vec::with_capacity(entry.preload.len() + entry.length as usize);
        data.extend_from_slice(&entry.preload);
        if entry.length > 0 {
            let (archive_path, start) = if entry.archive == DIR_ARCHIVE {
                (
                    self.dir_path.clone(),
                    self.dir_data_start + u64::from(entry.offset),
                )
            } else {
                let mut name = self.archive_stem.as_os_str().to_owned();
                name.push(format!("_{:03}.vpk", entry.archive));
                (PathBuf::from(name), u64::from(entry.offset))
            };
            let mut file = File::open(&archive_path)?;
            file.seek(SeekFrom::Start(start))?;
            let mut chunk = file.take(u64::from(entry.length));
            chunk.read_to_end(&mut data)?;
            if data.len() != entry.preload.len() + entry.length as usize {
                return Err(invalid(format!(
                    "{} is truncated reading {path}",
                    archive_path.display()
                )));
            }
        }
        Ok(Some(data))
    }

    /// The CRC32 recorded for a file, for integrity checks.
    pub fn crc(&self, path: &str) -> Option<u32> {
        self.entries.get(path).map(|e| e.crc)
    }
}
