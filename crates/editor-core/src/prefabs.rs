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
mod directory;
use directory::Directory;
pub const MAX_ENTRIES: usize = 128;
pub const MAX_SCAN: usize = 512;
pub const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_NAME_BYTES: usize = 80;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub bytes: u64,
    modified: SystemTime,
    #[cfg(unix)]
    identity: (u64, u64),
    directory: directory::SourceDirectory,
}
impl Entry {
    fn same_file(&self, other: &Self) -> bool {
        #[cfg(unix)]
        {
            self.identity == other.identity
        }
        #[cfg(not(unix))]
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
            #[cfg(unix)]
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn retarget(alias: &std::path::Path, target: &std::path::Path) {
        if fs::symlink_metadata(alias).is_ok() {
            fs::remove_file(alias).unwrap();
        }
        std::os::unix::fs::symlink(target, alias).unwrap();
    }

    #[test]
    fn empty_catalog_stamps_bind_parent_alias_and_physical_directory() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original/library");
        let replacement = directory.path().join("replacement/library");
        fs::create_dir_all(&original).unwrap();
        fs::create_dir_all(&replacement).unwrap();
        let alias = directory.path().join("bound");
        retarget(&alias, original.parent().unwrap());
        let store = Store {
            directory: alias.join("library"),
        };
        let reviewed = store.snapshot().unwrap();
        retarget(&alias, replacement.parent().unwrap());
        assert_ne!(reviewed.stamp, store.snapshot().unwrap().stamp);
        assert!(store.save("Probe", "world {}", &reviewed.stamp).is_err());
        assert_eq!(fs::read_dir(&original).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&replacement).unwrap().count(), 0);

        let reviewed = store.snapshot().unwrap();
        let displaced = replacement.with_file_name("displaced");
        fs::rename(&replacement, &displaced).unwrap();
        fs::create_dir(&replacement).unwrap();
        assert_ne!(reviewed.stamp, store.snapshot().unwrap().stamp);
        assert!(store.save("Probe", "world {}", &reviewed.stamp).is_err());
        assert_eq!(fs::read_dir(&displaced).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&replacement).unwrap().count(), 0);
    }

    #[test]
    fn stable_parent_alias_supports_first_save_read_rename_delete_and_detachment() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original");
        fs::create_dir(&original).unwrap();
        let alias = directory.path().join("bound");
        retarget(&alias, &original);
        let store = Store {
            directory: alias.join("nested/library"),
        };
        let missing = store.snapshot().unwrap();
        assert!(!original.join("nested").exists());
        assert!(
            store
                .save("Alpha", "world {}", &missing.stamp)
                .unwrap()
                .warning
                .is_none()
        );
        let saved = store.snapshot().unwrap();
        assert_eq!(store.read("Alpha", &saved.stamp).unwrap(), "world {}");
        assert!(
            store
                .rename("Alpha", "Beta", &saved.stamp)
                .unwrap()
                .warning
                .is_none()
        );
        let renamed = store.snapshot().unwrap();
        let source = renamed.entries[0].clone();
        assert!(
            store
                .remove("Beta", &renamed.stamp)
                .unwrap()
                .warning
                .is_none()
        );
        store.check_preview(&source).unwrap();
        let replacement = directory.path().join("replacement");
        fs::create_dir_all(replacement.join("nested/library")).unwrap();
        retarget(&alias, &replacement);
        assert!(store.check_preview(&source).is_err());
    }

    #[test]
    fn independently_created_missing_library_rejects_old_stamp() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().join("library"),
        };
        let missing = store.snapshot().unwrap();
        fs::create_dir(&store.directory).unwrap();
        let present = store.snapshot().unwrap();
        assert_ne!(missing.stamp, present.stamp);
        assert!(store.save("Old", "world {}", &missing.stamp).is_err());
        assert!(
            store
                .save("New", "world {}", &present.stamp)
                .unwrap()
                .warning
                .is_none()
        );
    }

    #[test]
    fn retained_directory_publication_survives_retarget_with_committed_warning() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original/library");
        let replacement = directory.path().join("replacement/library");
        fs::create_dir_all(&original).unwrap();
        fs::create_dir_all(&replacement).unwrap();
        let alias = directory.path().join("bound");
        retarget(&alias, original.parent().unwrap());
        let store = Store {
            directory: alias.join("library"),
        };
        let reviewed = store.snapshot().unwrap();
        let mut stage = reviewed.directory.stage().unwrap();
        stage.write("owned").unwrap();
        reviewed.directory.verify().unwrap();
        retarget(&alias, replacement.parent().unwrap());
        stage.publish("Alpha.vmf").unwrap();
        drop(stage);
        assert!(reviewed.directory.publication("saved").warning.is_some());
        assert_eq!(
            fs::read_to_string(original.join("Alpha.vmf")).unwrap(),
            "owned"
        );
        assert_eq!(fs::read_dir(&original).unwrap().count(), 1);
        assert_eq!(fs::read_dir(&replacement).unwrap().count(), 0);

        fs::write(replacement.join("Alpha.vmf"), "unrelated").unwrap();
        retarget(&alias, original.parent().unwrap());
        let reviewed = store.snapshot().unwrap();
        reviewed.directory.verify().unwrap();
        retarget(&alias, replacement.parent().unwrap());
        reviewed.directory.rename("Alpha.vmf", "Beta.vmf").unwrap();
        assert!(reviewed.directory.publication("renamed").warning.is_some());
        assert_eq!(
            fs::read_to_string(original.join("Beta.vmf")).unwrap(),
            "owned"
        );
        assert_eq!(
            fs::read_to_string(replacement.join("Alpha.vmf")).unwrap(),
            "unrelated"
        );
        assert!(!replacement.join("Beta.vmf").exists());

        retarget(&alias, original.parent().unwrap());
        let reviewed = store.snapshot().unwrap();
        reviewed.directory.verify().unwrap();
        retarget(&alias, replacement.parent().unwrap());
        reviewed.directory.remove("Beta.vmf").unwrap();
        assert!(reviewed.directory.publication("deleted").warning.is_some());
        assert_eq!(fs::read_dir(&original).unwrap().count(), 0);
        assert_eq!(
            fs::read_to_string(replacement.join("Alpha.vmf")).unwrap(),
            "unrelated"
        );
    }

    #[test]
    fn stale_alias_requests_preserve_both_nonempty_libraries() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original/library");
        let replacement = directory.path().join("replacement/library");
        fs::create_dir_all(&original).unwrap();
        fs::create_dir_all(&replacement).unwrap();
        fs::write(original.join("Alpha.vmf"), "original").unwrap();
        fs::write(replacement.join("Alpha.vmf"), "replacement").unwrap();
        let alias = directory.path().join("bound");
        retarget(&alias, original.parent().unwrap());
        let store = Store {
            directory: alias.join("library"),
        };
        let reviewed = store.snapshot().unwrap();
        retarget(&alias, replacement.parent().unwrap());
        assert!(store.read("Alpha", &reviewed.stamp).is_err());
        assert!(store.rename("Alpha", "Beta", &reviewed.stamp).is_err());
        assert!(store.remove("Alpha", &reviewed.stamp).is_err());
        assert_eq!(
            fs::read_to_string(original.join("Alpha.vmf")).unwrap(),
            "original"
        );
        assert_eq!(
            fs::read_to_string(replacement.join("Alpha.vmf")).unwrap(),
            "replacement"
        );
        assert!(!original.join("Beta.vmf").exists());
        assert!(!replacement.join("Beta.vmf").exists());
    }

    #[test]
    fn leaf_symlinks_and_fifos_are_rejected_without_following_or_waiting() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("library");
        fs::create_dir(&root).unwrap();
        let alias = directory.path().join("alias");
        retarget(&alias, &root);
        for path in [alias.clone(), alias.join(""), alias.join(".")] {
            assert!(Store { directory: path }.snapshot().is_err());
        }
        let store = Store {
            directory: root.clone(),
        };
        let reviewed = store.snapshot().unwrap();
        let outside = directory.path().join("outside.vmf");
        fs::write(&outside, "outside").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("Link.vmf")).unwrap();
        assert!(reviewed.directory.open("Link.vmf".as_ref()).is_err());
        assert!(store.snapshot().is_err());
        fs::remove_file(root.join("Link.vmf")).unwrap();
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            root.join("Pipe.vmf"),
            rustix::fs::Mode::RUSR,
        )
        .unwrap();
        assert!(
            !reviewed
                .directory
                .open("Pipe.vmf".as_ref())
                .unwrap()
                .metadata()
                .unwrap()
                .is_file()
        );
        assert!(store.snapshot().is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "outside");
    }

    #[test]
    fn metadata_catalog_retains_unreadable_entries_for_rename_and_delete() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("Alpha.vmf");
        fs::write(&file, "retained").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0)).unwrap();
        let store = Store {
            directory: directory.path().into(),
        };
        let snapshot = store.snapshot().unwrap();
        assert_eq!(snapshot.entries.len(), 1);
        store.rename("Alpha", "Beta", &snapshot.stamp).unwrap();
        let snapshot = store.snapshot().unwrap();
        store.remove("Beta", &snapshot.stamp).unwrap();
        assert!(store.snapshot().unwrap().entries.is_empty());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn atomic_rename_preserves_bytes_and_both_entries_on_invalid_or_stale_requests() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().into(),
        };
        store
            .save(
                "Alpha",
                "world { original }",
                &store.snapshot().unwrap().stamp,
            )
            .unwrap();
        store
            .save("Beta", "world { other }", &store.snapshot().unwrap().stamp)
            .unwrap();
        let snapshot = store.snapshot().unwrap();
        let alpha = snapshot
            .entries
            .iter()
            .find(|entry| entry.name == "Alpha")
            .unwrap()
            .clone();
        for new_name in ["beta", "../outside", " Alpha", "Alpha"] {
            assert!(store.rename("Alpha", new_name, &snapshot.stamp).is_err());
            assert_eq!(
                fs::read_to_string(directory.path().join("Alpha.vmf")).unwrap(),
                "world { original }"
            );
            assert_eq!(
                fs::read_to_string(directory.path().join("Beta.vmf")).unwrap(),
                "world { other }"
            );
        }
        store.rename("Alpha", "ALPHA", &snapshot.stamp).unwrap();
        assert!(!directory.path().join("Alpha.vmf").exists());
        assert_eq!(
            fs::read_to_string(directory.path().join("ALPHA.vmf")).unwrap(),
            "world { original }"
        );
        assert!(store.check_preview(&alpha).unwrap_err().contains("renamed"));
        assert!(store.rename("Beta", "Gamma", &snapshot.stamp).is_err());
        assert!(directory.path().join("Beta.vmf").exists());
        assert!(!directory.path().join("Gamma.vmf").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn competing_renames_never_replace_or_lose_either_payload() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().into(),
        };
        store
            .save("Alpha", "alpha", &store.snapshot().unwrap().stamp)
            .unwrap();
        store
            .save("Beta", "beta", &store.snapshot().unwrap().stamp)
            .unwrap();
        let snapshot = store.snapshot().unwrap().stamp;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut workers = Vec::new();
        for source in ["Alpha", "Beta"] {
            let (directory, snapshot, barrier) = (
                directory.path().to_path_buf(),
                snapshot.clone(),
                barrier.clone(),
            );
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                Store { directory }.rename(source, "Result", &snapshot)
            }));
        }
        barrier.wait();
        let succeeded = workers
            .into_iter()
            .filter_map(|worker| worker.join().unwrap().ok())
            .count();
        assert_eq!(succeeded, 1);
        let snapshot = store.snapshot().unwrap();
        assert_eq!(snapshot.entries.len(), 2);
        let mut texts = snapshot
            .entries
            .iter()
            .map(|entry| store.read(&entry.name, &snapshot.stamp).unwrap())
            .collect::<Vec<_>>();
        texts.sort();
        assert_eq!(texts, ["alpha", "beta"]);
    }
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
