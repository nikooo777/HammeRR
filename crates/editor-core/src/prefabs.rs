#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File},
    hash::{Hash, Hasher},
    io::{Read, Write},
    path::PathBuf,
    time::SystemTime,
};
#[cfg(not(windows))]
mod directory;
#[cfg(windows)]
#[path = "prefabs/directory_windows.rs"]
mod directory;
use directory::Directory;
pub const MAX_ENTRIES: usize = 128;
pub const MAX_SCAN: usize = 512;
pub const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_NAME_BYTES: usize = 80;
mod authority;
pub(crate) use authority::Edit;
pub use authority::{Authority, Snapshot as RegistrySnapshot};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub bytes: u64,
    modified: SystemTime,
    #[cfg(unix)]
    identity: (u64, u64),
    #[cfg(windows)]
    identity: source_formats::file_publication::FileIdentity,
    directory: directory::SourceDirectory,
}
impl Entry {
    fn same_file(&self, other: &Self) -> bool {
        #[cfg(any(unix, windows))]
        {
            self.identity == other.identity
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = other;
            false
        }
    }
}
pub struct Snapshot {
    pub entries: Vec<Entry>,
    pub stamp: String,
    pub bytes: u64,
    scanned: usize,
    directory: Directory,
}
#[derive(Debug)]
pub struct Mutation {
    pub warning: Option<String>,
}
pub struct Store {
    pub directory: PathBuf,
}
pub fn name(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_NAME_BYTES
        || value.trim() != value
        || !value
            .chars()
            .all(|c| c.is_alphanumeric() || [' ', '_', '-'].contains(&c))
    {
        return Err("Prefab names need 1–80 UTF-8 bytes of letters, numbers, spaces, dashes or underscores, without surrounding whitespace".into());
    }
    Ok(())
}
impl Store {
    fn root(&self) -> Result<bool, String> {
        let path = &self.directory;
        if path.to_str().is_none() {
            return Err("Prefab directory must be UTF-8".into());
        }
        if !path.is_absolute()
            || path.as_os_str().len() > 4096
            || path.to_string_lossy().chars().any(char::is_control)
        {
            return Err(
                "Prefab directory must be an absolute path of at most 4096 bytes without controls"
                    .into(),
            );
        }
        let leaf: PathBuf = path.components().collect();
        match fs::symlink_metadata(leaf) {
            Ok(meta) if meta.is_dir() => Ok(true),
            Ok(_) => Err("Prefab storage must be a regular directory, not a symlink".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    }
    pub fn snapshot(&self) -> Result<Snapshot, String> {
        self.root()?;
        let directory = Directory::capture(&self.directory)?;
        let mut entries = Vec::new();
        let mut bytes = 0u64;
        let mut scanned = 0;
        if directory.is_present() {
            for (index, filename) in directory.names()?.into_iter().enumerate() {
                scanned = index + 1;
                if index >= MAX_SCAN {
                    return Err("Prefab storage exceeds 512 directory entries".into());
                }
                let path = PathBuf::from(&filename);
                if path.extension().and_then(|ext| ext.to_str()) != Some("vmf") {
                    continue;
                }
                let stem = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .ok_or("Prefab filenames must be UTF-8")?;
                name(stem)?;
                let meta = directory.metadata(&filename)?;
                if !meta.is_file() || meta.len() > map_core::transfer::MAX_BYTES as u64 {
                    return Err(format!(
                        "Prefab {stem} must be a regular VMF file of at most 8 MiB"
                    ));
                }
                bytes = bytes.saturating_add(meta.len());
                if entries.len() >= MAX_ENTRIES || bytes > MAX_TOTAL_BYTES {
                    return Err("Prefab storage exceeds 128 VMFs or 64 MiB".into());
                }
                entries.push(Entry {
                    name: stem.into(),
                    bytes: meta.len(),
                    modified: meta.modified().map_err(|error| error.to_string())?,
                    #[cfg(unix)]
                    identity: (meta.dev(), meta.ino()),
                    #[cfg(windows)]
                    identity: source_formats::file_publication::FileIdentity::from_file(
                        &directory.open(&filename)?,
                    )
                    .map_err(|error| error.to_string())?,
                    directory: directory.source()?,
                });
            }
        }
        entries.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.name.cmp(&b.name))
        });
        let mut hash = DefaultHasher::new();
        self.directory.hash(&mut hash);
        directory.binding.hash(&mut hash);
        directory.is_present().hash(&mut hash);
        for entry in &entries {
            entry.name.hash(&mut hash);
            entry.bytes.hash(&mut hash);
            entry.modified.hash(&mut hash);
            #[cfg(any(unix, windows))]
            entry.identity.hash(&mut hash);
        }
        directory.verify()?;
        Ok(Snapshot {
            entries,
            bytes,
            scanned,
            stamp: format!("{:016x}", hash.finish()),
            directory,
        })
    }
    fn checked(&self, stamp: &str) -> Result<Snapshot, String> {
        let snapshot = self.snapshot()?;
        if snapshot.stamp != stamp {
            return Err("Prefab library changed; refresh it before continuing".into());
        }
        Ok(snapshot)
    }
    pub(crate) fn check_preview(&self, source: &Entry) -> Result<(), String> {
        source.directory.verify()?;
        let snapshot = self.snapshot()?;
        if source.directory.binding != snapshot.directory.binding {
            return Err("Prefab library directory changed; preview it again".into());
        }
        if let Some(current) = snapshot
            .entries
            .iter()
            .find(|entry| entry.name == source.name)
        {
            if current != source {
                return Err("Prefab source changed; choose it in Prefabs and preview again".into());
            }
        } else if let Some(renamed) = snapshot
            .entries
            .iter()
            .find(|entry| entry.same_file(source))
        {
            return Err(format!(
                "Prefab was renamed to {}; choose it in Prefabs and preview again",
                renamed.name
            ));
        }
        Ok(())
    }
    pub fn rename(&self, value: &str, new_name: &str, stamp: &str) -> Result<Mutation, String> {
        name(value)?;
        name(new_name)?;
        let snapshot = self.checked(stamp)?;
        if !snapshot.entries.iter().any(|entry| entry.name == value) {
            return Err("Prefab no longer exists".into());
        }
        if value == new_name {
            return Err("Choose a different prefab name".into());
        }
        if snapshot.entries.iter().any(|entry| {
            entry.name != value && entry.name.to_lowercase() == new_name.to_lowercase()
        }) {
            return Err("A prefab with that name already exists; choose another name".into());
        }
        self.checked(stamp)?;
        snapshot.directory.verify()?;
        snapshot
            .directory
            .rename(&format!("{value}.vmf"), &format!("{new_name}.vmf"))?;
        Ok(snapshot.directory.publication("renamed"))
    }
    pub fn read(&self, value: &str, stamp: &str) -> Result<String, String> {
        name(value)?;
        let snapshot = self.checked(stamp)?;
        let entry = snapshot
            .entries
            .iter()
            .find(|entry| entry.name == value)
            .ok_or("Prefab no longer exists")?;
        let filename = format!("{value}.vmf");
        let file = snapshot.directory.open(filename.as_ref())?;
        let meta = file.metadata().map_err(|error| error.to_string())?;
        #[cfg(unix)]
        if (meta.dev(), meta.ino()) != entry.identity {
            return Err("Prefab was replaced while reading".into());
        }
        #[cfg(windows)]
        if source_formats::file_publication::FileIdentity::from_file(&file)
            .map_err(|error| error.to_string())?
            != entry.identity
        {
            return Err("Prefab was replaced while reading".into());
        }
        if !meta.is_file()
            || meta.len() != entry.bytes
            || meta.modified().map_err(|error| error.to_string())? != entry.modified
        {
            return Err("Prefab changed while reading".into());
        }
        let mut bytes = Vec::new();
        file.take(map_core::transfer::MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 != entry.bytes {
            return Err("Prefab changed size while reading".into());
        }
        self.checked(stamp)?;
        snapshot.directory.verify()?;
        String::from_utf8(bytes).map_err(|_| "Prefab VMF must contain UTF-8 text".into())
    }
    pub fn save(&self, value: &str, text: &str, stamp: &str) -> Result<Mutation, String> {
        name(value)?;
        let mut snapshot = self.checked(stamp)?;
        if snapshot
            .entries
            .iter()
            .any(|entry| entry.name.to_lowercase() == value.to_lowercase())
        {
            return Err("A prefab with that name already exists; choose another name".into());
        }
        if text.len() > map_core::transfer::MAX_BYTES
            || snapshot.scanned >= MAX_SCAN
            || snapshot.entries.len() >= MAX_ENTRIES
            || snapshot.bytes.saturating_add(text.len() as u64) > MAX_TOTAL_BYTES
        {
            return Err("Prefab exceeds the 8 MiB file, 128-entry or 64 MiB library limit".into());
        }
        snapshot.directory.create()?;
        self.check_entries(&snapshot)?;
        let mut stage = snapshot.directory.stage()?;
        stage.write(text)?;
        self.check_entries(&snapshot)?;
        snapshot.directory.verify()?;
        stage.publish(&format!("{value}.vmf"))?;
        drop(stage);
        Ok(snapshot.directory.publication("saved"))
    }
    fn check_entries(&self, expected: &Snapshot) -> Result<(), String> {
        expected.directory.verify()?;
        let current = self.snapshot()?;
        if current.entries != expected.entries {
            return Err("Prefab library changed; refresh it before continuing".into());
        }
        expected.directory.verify()
    }
    pub fn remove(&self, value: &str, stamp: &str) -> Result<Mutation, String> {
        name(value)?;
        let snapshot = self.checked(stamp)?;
        if !snapshot.entries.iter().any(|entry| entry.name == value) {
            return Err("Prefab no longer exists".into());
        }
        self.checked(stamp)?;
        snapshot.directory.verify()?;
        snapshot.directory.remove(&format!("{value}.vmf"))?;
        Ok(snapshot.directory.publication("deleted"))
    }
}
