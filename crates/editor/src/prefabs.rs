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
pub const MAX_ENTRIES: usize = 128;
pub const MAX_SCAN: usize = 512;
pub const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_NAME_BYTES: usize = 80;
#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub bytes: u64,
    modified: SystemTime,
    #[cfg(unix)]
    identity: (u64, u64),
}
pub struct Snapshot {
    pub entries: Vec<Entry>,
    pub stamp: String,
    pub bytes: u64,
    scanned: usize,
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
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => Ok(true),
            Ok(_) => Err("Prefab storage must be a regular directory, not a symlink".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    }
    pub fn snapshot(&self) -> Result<Snapshot, String> {
        let mut entries = Vec::new();
        let mut bytes = 0u64;
        let mut scanned = 0;
        if self.root()? {
            for (index, entry) in fs::read_dir(&self.directory)
                .map_err(|error| error.to_string())?
                .enumerate()
            {
                scanned = index + 1;
                if index >= MAX_SCAN {
                    return Err("Prefab storage exceeds 512 directory entries".into());
                }
                let entry = entry.map_err(|error| error.to_string())?;
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("vmf") {
                    continue;
                }
                let stem = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .ok_or("Prefab filenames must be UTF-8")?;
                name(stem)?;
                let meta = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
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
        for entry in &entries {
            entry.name.hash(&mut hash);
            entry.bytes.hash(&mut hash);
            entry.modified.hash(&mut hash);
            #[cfg(unix)]
            entry.identity.hash(&mut hash);
        }
        Ok(Snapshot {
            entries,
            bytes,
            scanned,
            stamp: format!("{:016x}", hash.finish()),
        })
    }
    fn checked(&self, stamp: &str) -> Result<Snapshot, String> {
        let snapshot = self.snapshot()?;
        if snapshot.stamp != stamp {
            return Err("Prefab library changed; refresh it before continuing".into());
        }
        Ok(snapshot)
    }
    pub fn read(&self, value: &str, stamp: &str) -> Result<String, String> {
        name(value)?;
        let snapshot = self.checked(stamp)?;
        let entry = snapshot
            .entries
            .iter()
            .find(|entry| entry.name == value)
            .ok_or("Prefab no longer exists")?;
        let path = self.directory.join(format!("{value}.vmf"));
        let file = File::open(path).map_err(|error| error.to_string())?;
        let meta = file.metadata().map_err(|error| error.to_string())?;
        #[cfg(unix)]
        if (meta.dev(), meta.ino()) != entry.identity {
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
        String::from_utf8(bytes).map_err(|_| "Prefab VMF must contain UTF-8 text".into())
    }
    pub fn save(&self, value: &str, text: &str, stamp: &str) -> Result<(), String> {
        name(value)?;
        let snapshot = self.checked(stamp)?;
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
        fs::create_dir_all(&self.directory).map_err(|error| error.to_string())?;
        self.root()?;
        self.checked(stamp)?;
        let mut file =
            tempfile::NamedTempFile::new_in(&self.directory).map_err(|error| error.to_string())?;
        file.write_all(text.as_bytes())
            .map_err(|error| error.to_string())?;
        file.as_file()
            .sync_all()
            .map_err(|error| error.to_string())?;
        file.persist_noclobber(self.directory.join(format!("{value}.vmf")))
            .map_err(|error| error.error.to_string())?;
        Ok(())
    }
    pub fn remove(&self, value: &str, stamp: &str) -> Result<(), String> {
        name(value)?;
        let snapshot = self.checked(stamp)?;
        if !snapshot.entries.iter().any(|entry| entry.name == value) {
            return Err("Prefab no longer exists".into());
        }
        fs::remove_file(self.directory.join(format!("{value}.vmf")))
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_files_use_atomic_unique_creation_and_stale_guards() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store {
            directory: dir.path().join("library"),
        };
        let empty = store.snapshot().unwrap();
        store
            .save("Light cluster", "world {}", &empty.stamp)
            .unwrap();
        let saved = store.snapshot().unwrap();
        assert_ne!(empty.stamp, saved.stamp);
        assert_eq!(
            store.read("Light cluster", &saved.stamp).unwrap(),
            "world {}"
        );
        assert!(
            store
                .save("light cluster", "replacement", &saved.stamp)
                .is_err()
        );
        assert!(store.remove("Light cluster", &empty.stamp).is_err());
        assert!(store.save("../outside", "x", &saved.stamp).is_err());
        store.remove("Light cluster", &saved.stamp).unwrap();
        assert!(store.snapshot().unwrap().entries.is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn storage_rejects_symlinks_and_retains_unrelated_files() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        fs::write(&outside, "original").unwrap();
        let root = dir.path().join("library");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("readme.txt"), "keep").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("Link.vmf")).unwrap();
        let store = Store {
            directory: root.clone(),
        };
        assert!(store.snapshot().is_err());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "original");
        assert!(root.join("readme.txt").exists());
        fs::remove_file(root.join("Link.vmf")).unwrap();
        assert_eq!(store.snapshot().unwrap().entries.len(), 0);
    }
}
