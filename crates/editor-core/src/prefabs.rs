use sha2::{Digest, Sha256};
use source_formats::file_publication::{BoundDestination, Policy};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File},
    hash::{Hash, Hasher},
    io::{Read, Write},
    path::PathBuf,
    sync::Mutex,
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
mod organization;
pub(crate) use authority::Edit;
pub use authority::{Authority, Snapshot as RegistrySnapshot};
pub use organization::{MAX_CATEGORY_BYTES, MAX_TAG_BYTES, MAX_TAGS, Organization};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub bytes: u64,
    pub organization: Result<Organization, String>,
    digest: [u8; 32],
    modified: SystemTime,
    #[cfg(unix)]
    identity: (u64, u64),
    #[cfg(windows)]
    identity: source_formats::file_publication::FileIdentity,
    directory: directory::SourceDirectory,
}
impl Entry {
    fn same_source(&self, other: &Self) -> bool {
        self.name == other.name
            && self.bytes == other.bytes
            && self.modified == other.modified
            && self.directory == other.directory
            && self.same_file(other)
    }

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
    organizations: Mutex<Vec<Entry>>,
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
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            organizations: Mutex::new(Vec::new()),
        }
    }

    fn observe_entry(&self, directory: &Directory, mut entry: Entry) -> Result<Entry, String> {
        {
            let cache = self.organizations.lock().map_err(
                |_| "Prefab organization cache lock is poisoned; restart the application",
            )?;
            if let Some(cached) = cache.iter().find(|cached| cached.same_source(&entry)) {
                return Ok(cached.clone());
            }
        }
        let loaded = read_entry_bytes(directory, &entry);
        let mut cache = self
            .organizations
            .lock()
            .map_err(|_| "Prefab organization cache lock is poisoned; restart the application")?;
        cache.retain(|cached| cached.name != entry.name);
        let bytes = loaded?;
        entry.digest = Sha256::digest(&bytes).into();
        entry.organization = String::from_utf8(bytes)
            .map_err(|_| "Prefab VMF must contain UTF-8 text".to_owned())
            .and_then(|text| organization::parse(&text));
        if cache.len() == MAX_ENTRIES {
            cache.remove(0);
        }
        cache.push(entry.clone());
        Ok(entry)
    }

    fn checked_entry_bytes(&self, directory: &Directory, entry: &Entry) -> Result<Vec<u8>, String> {
        let result = read_entry_bytes(directory, entry).and_then(|bytes| {
            let digest: [u8; 32] = Sha256::digest(&bytes).into();
            if digest != entry.digest {
                return Err(
                    "Prefab content changed; refresh the library and review it again".into(),
                );
            }
            Ok(bytes)
        });
        if result.is_err() {
            self.organizations
                .lock()
                .map_err(|_| "Prefab organization cache lock is poisoned; restart the application")?
                .retain(|cached| !cached.same_source(entry));
        }
        result
    }

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
                let entry = Entry {
                    name: stem.into(),
                    bytes: meta.len(),
                    organization: Err("Prefab organization has not been read".into()),
                    digest: [0; 32],
                    modified: meta.modified().map_err(|error| error.to_string())?,
                    #[cfg(unix)]
                    identity: (meta.dev(), meta.ino()),
                    #[cfg(windows)]
                    identity: source_formats::file_publication::FileIdentity::from_file(
                        &directory.open(&filename)?,
                    )
                    .map_err(|error| error.to_string())?,
                    directory: directory.source()?,
                };
                let entry = self
                    .observe_entry(&directory, entry)
                    .map_err(|error| format!("Prefab {stem}: {error}"))?;
                entries.push(entry);
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
            entry.organization.hash(&mut hash);
            entry.digest.hash(&mut hash);
            #[cfg(any(unix, windows))]
            entry.identity.hash(&mut hash);
        }
        directory.verify()?;
        self.organizations
            .lock()
            .map_err(|_| "Prefab organization cache lock is poisoned; restart the application")?
            .retain(|cached| entries.iter().any(|entry| cached.same_source(entry)));
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
            self.checked_entry_bytes(&snapshot.directory, source)?;
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
        let entry = snapshot
            .entries
            .iter()
            .find(|entry| entry.name == value)
            .ok_or("Prefab no longer exists")?;
        if value == new_name {
            return Err("Choose a different prefab name".into());
        }
        if snapshot.entries.iter().any(|entry| {
            entry.name != value && entry.name.to_lowercase() == new_name.to_lowercase()
        }) {
            return Err("A prefab with that name already exists; choose another name".into());
        }
        self.checked(stamp)?;
        self.checked_entry_bytes(&snapshot.directory, entry)?;
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
        let bytes = self.checked_entry_bytes(&snapshot.directory, entry)?;
        entry.organization.as_ref().map_err(Clone::clone)?;
        let text = String::from_utf8(bytes).map_err(|_| "Prefab VMF must contain UTF-8 text")?;
        self.checked(stamp)?;
        snapshot.directory.verify()?;
        Ok(text)
    }
    pub fn save(&self, value: &str, text: &str, stamp: &str) -> Result<Mutation, String> {
        name(value)?;
        let text = organization::organize(text, &Organization::default())?;
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
        stage.write(&text)?;
        self.check_entries(&snapshot)?;
        snapshot.directory.verify()?;
        stage.publish(&format!("{value}.vmf"))?;
        drop(stage);
        Ok(snapshot.directory.publication("saved"))
    }

    pub fn replace(&self, value: &str, text: &str, stamp: &str) -> Result<Mutation, String> {
        name(value)?;
        let previous = self.read(value, stamp)?;
        let replacement = organization::replace(&previous, text)?;
        self.rewrite(value, &previous, &replacement, stamp, "replaced")
    }

    pub fn organize(
        &self,
        value: &str,
        organization: &Organization,
        stamp: &str,
    ) -> Result<Mutation, String> {
        name(value)?;
        let previous = self.read(value, stamp)?;
        let replacement = organization::organize(&previous, organization)?;
        self.rewrite(value, &previous, &replacement, stamp, "organized")
    }

    fn rewrite(
        &self,
        value: &str,
        previous: &str,
        text: &str,
        stamp: &str,
        action: &str,
    ) -> Result<Mutation, String> {
        let snapshot = self.checked(stamp)?;
        let entry = snapshot
            .entries
            .iter()
            .find(|entry| entry.name == value)
            .ok_or("Prefab no longer exists")?;
        if text == previous {
            if self.read(value, stamp)? != previous {
                return Err(
                    "Prefab content changed; review it again before changing the library".into(),
                );
            }
            return Ok(Mutation { warning: None });
        }
        if text.len() > map_core::transfer::MAX_BYTES
            || snapshot
                .bytes
                .saturating_sub(entry.bytes)
                .saturating_add(text.len() as u64)
                > MAX_TOTAL_BYTES
            || snapshot.scanned >= MAX_SCAN
        {
            return Err(
                "Prefab exceeds the 8 MiB file, 512 directory entries or 64 MiB library limit"
                    .into(),
            );
        }
        let destination = BoundDestination::capture(
            &self.directory.join(format!("{value}.vmf")),
            Policy::FollowSource,
        )
        .map_err(|error| error.to_string())?;
        snapshot.directory.verify()?;
        if self.read(value, stamp)? != previous {
            return Err(
                "Prefab content changed; review it again before changing the library".into(),
            );
        }
        let mut stage = destination.stage().map_err(|error| error.to_string())?;
        stage
            .write_all(text.as_bytes())
            .map_err(|error| error.to_string())?;
        self.check_entries(&snapshot)?;
        if self.checked_entry_bytes(&snapshot.directory, entry)? != previous.as_bytes() {
            return Err(
                "Prefab content changed; review it again before changing the library".into(),
            );
        }
        destination.verify().map_err(|error| error.to_string())?;
        snapshot.directory.verify()?;
        let receipt = stage.commit().map_err(|error| error.to_string())?;
        let verified = snapshot.directory.verify().and_then(|()| {
            let actual = receipt
                .destination
                .read_existing()
                .map_err(|error| error.to_string())?;
            if actual.as_deref() != Some(text.as_bytes()) {
                return Err("Published prefab content changed".into());
            }
            snapshot.directory.verify()
        });
        Ok(Mutation {warning:receipt.warning.or_else(||verified.err()).map(|error|format!("Prefab was {action}, but publication could not be verified; refresh the library: {error}"))})
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
        let entry = snapshot
            .entries
            .iter()
            .find(|entry| entry.name == value)
            .ok_or("Prefab no longer exists")?;
        self.checked(stamp)?;
        self.checked_entry_bytes(&snapshot.directory, entry)?;
        snapshot.directory.verify()?;
        snapshot.directory.remove(&format!("{value}.vmf"))?;
        Ok(snapshot.directory.publication("deleted"))
    }
}

fn read_entry_bytes(directory: &Directory, entry: &Entry) -> Result<Vec<u8>, String> {
    entry.directory.verify()?;
    if entry.directory.binding != directory.binding {
        return Err("Prefab source belongs to a different library directory".into());
    }
    let filename = format!("{}.vmf", entry.name);
    let mut file = directory.open(filename.as_ref())?;
    verify_entry_file(&file, entry)?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(map_core::transfer::MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 != entry.bytes {
        return Err("Prefab changed size while reading".into());
    }
    verify_entry_file(&file, entry)?;
    let current = directory.open(filename.as_ref())?;
    verify_entry_file(&current, entry)?;
    directory.verify()?;
    entry.directory.verify()?;
    Ok(bytes)
}

fn verify_entry_file(file: &File, entry: &Entry) -> Result<(), String> {
    let meta = file.metadata().map_err(|error| error.to_string())?;
    #[cfg(unix)]
    if (meta.dev(), meta.ino()) != entry.identity {
        return Err("Prefab was replaced while reading".into());
    }
    #[cfg(windows)]
    if source_formats::file_publication::FileIdentity::from_file(file)
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
    Ok(())
}
