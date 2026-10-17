use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

mod settings;
pub(crate) use settings::ReloadError;
pub use settings::{Authority, SettingsSnapshot, SettingsUpdate};

const FORMAT_VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: u64 = 32 * 1024;
pub const MAX_SETTINGS_BYTES: u64 = 32 * 1024;
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub keep_per_map: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: 120,
            keep_per_map: 10,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if !(30..=3600).contains(&self.interval_seconds) {
            return Err("Autosave interval must be between 30 and 3600 seconds".into());
        }
        if !(1..=50).contains(&self.keep_per_map) {
            return Err("Keep between 1 and 50 recovery snapshots per map".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Autosave,
    Manual,
    BeforeSave,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub format: u32,
    pub id: String,
    pub document_key: String,
    pub source_path: Option<PathBuf>,
    pub title: String,
    pub created_ms: u64,
    pub bytes: u64,
    pub kind: Kind,
}

#[derive(Debug, Default, Serialize)]
pub struct Listing {
    pub entries: Vec<Entry>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Store {
    pub directory: PathBuf,
}

pub fn unique_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{nanos:032x}-{:08x}-{:016x}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    )
}

fn valid_id(id: &str) -> bool {
    id.len() == 58
        && id.bytes().enumerate().all(|(i, byte)| {
            if i == 32 || i == 41 {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), String> {
    let (destination, metadata) = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_symlink() => {
            let destination = path
                .canonicalize()
                .map_err(|error| format!("{}: {error}", path.display()))?;
            let metadata = fs::metadata(&destination)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            (destination, Some(metadata))
        }
        Ok(metadata) => (path.to_path_buf(), Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (path.to_path_buf(), None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if let Some(metadata) = &metadata {
        if !metadata.is_file() {
            return Err(format!("{} must be a regular file", path.display()));
        }
        if metadata.permissions().readonly() {
            return Err(format!("{} is read-only", path.display()));
        }
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| format!("{}: {e}", parent.display()))?;
    if let Some(metadata) = metadata {
        temporary
            .as_file()
            .set_permissions(metadata.permissions())
            .map_err(|e| e.to_string())?;
    }
    temporary
        .write_all(data)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    temporary
        .persist(&destination)
        .map_err(|e| format!("{}: {}", path.display(), e.error))?;
    Ok(())
}

impl Store {
    pub fn load_settings(&self) -> Result<Settings, String> {
        let path = self.directory.join("settings.json");
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
        }
        let file = match options.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Settings::default());
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let metadata = file
            .metadata()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if !metadata.is_file() || metadata.len() > MAX_SETTINGS_BYTES {
            return Err(format!(
                "{} must be a regular file of at most {MAX_SETTINGS_BYTES} bytes",
                path.display()
            ));
        }
        let mut data = Vec::new();
        file.take(MAX_SETTINGS_BYTES + 1)
            .read_to_end(&mut data)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if data.len() as u64 > MAX_SETTINGS_BYTES {
            return Err(format!(
                "{} exceeds {MAX_SETTINGS_BYTES} bytes",
                path.display()
            ));
        }
        let settings: Settings =
            serde_json::from_slice(&data).map_err(|e| format!("{}: {e}", path.display()))?;
        settings.validate()?;
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<(), String> {
        settings.validate()?;
        fs::create_dir_all(&self.directory)
            .map_err(|e| format!("{}: {e}", self.directory.display()))?;
        atomic_write(
            &self.directory.join("settings.json"),
            &serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?,
        )
    }

    fn entry(&self, id: &str) -> Result<Entry, String> {
        if !valid_id(id) {
            return Err("Invalid recovery snapshot ID".into());
        }
        let path = self.directory.join(format!("{id}.json"));
        let metadata =
            fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES {
            return Err("Invalid recovery manifest".into());
        }
        let entry: Entry = serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if entry.format != FORMAT_VERSION || entry.id != id {
            return Err("Unsupported or mismatched recovery manifest".into());
        }
        let map_path = self.directory.join(format!("{id}.vmf"));
        let map_metadata =
            fs::symlink_metadata(&map_path).map_err(|e| format!("{}: {e}", map_path.display()))?;
        if !map_metadata.is_file() || map_metadata.len() != entry.bytes {
            return Err("Recovery snapshot is incomplete or changed on disk".into());
        }
        Ok(entry)
    }

    pub fn list(&self) -> Result<Listing, String> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Listing::default());
            }
            Err(error) => return Err(format!("{}: {error}", self.directory.display())),
        };
        let mut listing = Listing::default();
        for entry in entries {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let Some(id) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .filter(|id| valid_id(id))
            else {
                continue;
            };
            match self.entry(id) {
                Ok(entry) => listing.entries.push(entry),
                Err(error) => listing.warnings.push(error),
            }
        }
        listing.entries.sort_by(|a, b| {
            b.created_ms
                .cmp(&a.created_ms)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(listing)
    }

    pub fn write(
        &self,
        document_key: &str,
        source_path: Option<&Path>,
        title: &str,
        kind: Kind,
        data: &[u8],
    ) -> Result<Entry, String> {
        fs::create_dir_all(&self.directory)
            .map_err(|e| format!("{}: {e}", self.directory.display()))?;
        let id = unique_id();
        let entry = Entry {
            format: FORMAT_VERSION,
            id: id.clone(),
            document_key: document_key.into(),
            source_path: source_path.map(Path::to_owned),
            title: title.into(),
            created_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            bytes: data.len() as u64,
            kind,
        };
        let manifest = serde_json::to_vec_pretty(&entry).map_err(|e| e.to_string())?;
        if manifest.len() as u64 > MAX_MANIFEST_BYTES {
            return Err("Recovery metadata is too large".into());
        }
        let map_path = self.directory.join(format!("{id}.vmf"));
        atomic_write(&map_path, data)?;
        if let Err(error) = atomic_write(&self.directory.join(format!("{id}.json")), &manifest) {
            let _ = fs::remove_file(&map_path);
            return Err(error);
        }
        Ok(entry)
    }

    pub fn read(&self, id: &str) -> Result<(Entry, String), String> {
        let entry = self.entry(id)?;
        let data = fs::read_to_string(self.directory.join(format!("{id}.vmf")))
            .map_err(|e| e.to_string())?;
        Ok((entry, data))
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        self.entry(id)?;
        fs::remove_file(self.directory.join(format!("{id}.json"))).map_err(|e| e.to_string())?;
        fs::remove_file(self.directory.join(format!("{id}.vmf"))).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn prune(&self, document_key: &str, keep: usize) -> Result<(), String> {
        if keep == 0 {
            return Err("Keep at least one recovery snapshot".into());
        }
        for entry in self
            .list()?
            .entries
            .iter()
            .filter(|entry| entry.document_key == document_key)
            .skip(keep)
        {
            self.delete(&entry.id)?;
        }
        Ok(())
    }
}

pub fn default_directory() -> Result<PathBuf, String> {
    if let Some(directory) = std::env::var_os("HAMMERR_RECOVERY_DIR") {
        let path = PathBuf::from(directory);
        return if path.is_absolute() {
            Ok(path)
        } else {
            Err("HAMMERR_RECOVERY_DIR must be an absolute path".into())
        };
    }
    if let Some(directory) = std::env::var_os("XDG_STATE_HOME") {
        let path = PathBuf::from(directory);
        if path.is_absolute() {
            return Ok(path.join("hammerr/recovery"));
        }
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".local/state/hammerr/recovery"))
        .ok_or_else(|| {
            "Set XDG_STATE_HOME or HAMMERR_RECOVERY_DIR to enable recovery storage".into()
        })
}

pub fn capture(
    session: &crate::runtime::SharedSession,
    actor: crate::session::Actor,
    kind: Kind,
) -> Result<Entry, String> {
    let request = crate::runtime::lock(session).begin_recovery_snapshot(kind)?;
    complete_capture(session, actor, &request)
}

pub fn complete_capture(
    session: &crate::runtime::SharedSession,
    actor: crate::session::Actor,
    request: &crate::session::RecoveryRequest,
) -> Result<Entry, String> {
    complete_capture_with(session, actor, request, || {})
}

pub(crate) fn complete_capture_with(
    session: &crate::runtime::SharedSession,
    actor: crate::session::Actor,
    request: &crate::session::RecoveryRequest,
    before_write: impl FnOnce(),
) -> Result<Entry, String> {
    complete_capture_completed_with(session, actor, request, before_write)
        .map(|completion| completion.entry)
}

fn complete_capture_completed_with(
    session: &crate::runtime::SharedSession,
    actor: crate::session::Actor,
    request: &crate::session::RecoveryRequest,
    before_write: impl FnOnce(),
) -> Result<crate::session::RecoveryCompletion, String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        request.run_with(before_write)
    }))
    .unwrap_or_else(|_| Err("Recovery snapshot worker panicked".into()));
    crate::runtime::lock(session).finish_recovery_snapshot_completed(actor, request, result)
}

type StopSignal = std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>;

pub struct Worker {
    stop: StopSignal,
    thread: Option<std::thread::JoinHandle<()>>,
    session: std::sync::Weak<std::sync::Mutex<crate::session::Session>>,
    drained: Option<Result<Option<Entry>, String>>,
}

impl Worker {
    pub fn start(session: &crate::runtime::SharedSession) -> Result<Self, String> {
        let stop = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let signal = stop.clone();
        let weak = std::sync::Arc::downgrade(session);
        let worker_session = weak.clone();
        let thread = std::thread::Builder::new()
            .name("map-autosave".into())
            .spawn(move || autosave_loop(weak, signal))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            stop,
            thread: Some(thread),
            session: worker_session,
            drained: None,
        })
    }

    pub fn drain(&mut self) -> Result<Option<Entry>, String> {
        if let Some(result) = &self.drained {
            return result.clone();
        }
        let session = self.session.upgrade();
        *self
            .stop
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.stop.1.notify_all();
        let joined = self.thread.take().map_or(Ok(()), |thread| {
            thread
                .join()
                .map_err(|_| "Recovery autosave thread panicked".to_owned())
        });
        let captured = match &session {
            None => Err("Recovery session was released before worker drain".into()),
            Some(session) => {
                let request = crate::runtime::lock(session).begin_exit_recovery_snapshot();
                request.and_then(|request| match request {
                    None => Ok(None),
                    Some(request) => complete_capture_completed_with(
                        session,
                        crate::session::Actor::System,
                        &request,
                        || {},
                    )
                    .map_err(|error| format!("Exit recovery snapshot failed: {error}"))
                    .and_then(|completion| match completion.retention_error {
                        Some(error) => Err(format!(
                            "Exit recovery snapshot saved, but recovery cleanup failed: {error}"
                        )),
                        None => Ok(Some(completion.entry)),
                    }),
                })
            }
        };
        let result = match (joined, captured) {
            (Ok(()), result) => result,
            (Err(join), Ok(_)) => Err(join),
            (Err(join), Err(capture)) => Err(format!("{join}; {capture}")),
        };
        if let (Some(session), Err(error)) = (&session, &result) {
            crate::runtime::lock(session).report_recovery_worker_error(error.clone());
        }
        self.drained = Some(result.clone());
        result
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if self.drained.is_none()
            && let Err(error) = self.drain()
        {
            eprintln!("Could not drain recovery worker: {error}");
        }
    }
}

fn autosave_loop(
    session: std::sync::Weak<std::sync::Mutex<crate::session::Session>>,
    stop: StopSignal,
) {
    loop {
        let signal = stop
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (signal, _) = stop
            .1
            .wait_timeout_while(signal, std::time::Duration::from_secs(5), |stop| !*stop)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *signal {
            return;
        }
        drop(signal);
        let Some(session) = session.upgrade() else {
            return;
        };
        let request = crate::runtime::lock(&session).begin_due_recovery_snapshot();
        if let Some(request) = request {
            let _ = complete_capture(&session, crate::session::Actor::System, &request);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().join("recovery"),
        };
        (directory, store)
    }

    #[test]
    fn snapshots_roundtrip_and_retention_keeps_other_maps() {
        let (_directory, store) = store();
        let map = map_core::Document::empty().vmf().write(&Default::default());
        let first = store
            .write(
                "a",
                Some(Path::new("a.vmf")),
                "a.vmf",
                Kind::Autosave,
                map.as_bytes(),
            )
            .unwrap();
        let second = store
            .write(
                "a",
                Some(Path::new("a.vmf")),
                "a.vmf",
                Kind::BeforeSave,
                map.as_bytes(),
            )
            .unwrap();
        let other = store
            .write("b", None, "Untitled", Kind::Manual, map.as_bytes())
            .unwrap();
        assert_eq!(store.read(&first.id).unwrap().1, map);
        store.prune("a", 1).unwrap();
        assert!(store.read(&first.id).is_err());
        assert!(store.read(&second.id).is_ok());
        assert!(store.read(&other.id).is_ok());
        assert_eq!(store.list().unwrap().entries.len(), 2);
        store.delete(&second.id).unwrap();
        assert_eq!(store.list().unwrap().entries.len(), 1);
    }

    #[test]
    fn settings_validate_before_replacing_persistent_values() {
        let (_directory, store) = store();
        assert_eq!(store.load_settings().unwrap(), Settings::default());
        let good = Settings {
            interval_seconds: 60,
            keep_per_map: 3,
            enabled: false,
        };
        store.save_settings(&good).unwrap();
        let mut bad = good.clone();
        bad.keep_per_map = 0;
        assert!(store.save_settings(&bad).is_err());
        assert_eq!(store.load_settings().unwrap(), good);
        bad.keep_per_map = 2;
        bad.interval_seconds = 0;
        assert!(store.save_settings(&bad).is_err());
    }

    #[test]
    fn invalid_incomplete_and_external_snapshot_paths_are_rejected() {
        let (directory, store) = store();
        assert!(store.read("../outside").is_err());
        assert!(store.delete("../outside").is_err());
        let entry = store
            .write("a", None, "Untitled", Kind::Manual, b"map")
            .unwrap();
        fs::write(
            store.directory.join(format!("{}.vmf", entry.id)),
            b"changed",
        )
        .unwrap();
        assert!(store.read(&entry.id).is_err());
        let listing = store.list().unwrap();
        assert_eq!(listing.entries.len(), 0);
        assert_eq!(listing.warnings.len(), 1);
        let external = directory.path().join("outside");
        fs::write(&external, b"map").unwrap();
        fs::remove_file(store.directory.join(format!("{}.vmf", entry.id))).unwrap();
        std::os::unix::fs::symlink(&external, store.directory.join(format!("{}.vmf", entry.id)))
            .unwrap();
        assert!(store.read(&entry.id).is_err());
        assert!(store.delete(&entry.id).is_err());
        assert_eq!(fs::read(&external).unwrap(), b"map");
    }

    #[test]
    fn failed_atomic_save_preserves_original_and_unrelated_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let map = directory.path().join("map.vmf");
        let unrelated = directory.path().join("map.vmf.tmp");
        fs::write(&unrelated, b"unrelated").unwrap();
        atomic_write(&map, b"original").unwrap();
        atomic_write(&map, b"updated").unwrap();
        assert_eq!(fs::read(&map).unwrap(), b"updated");
        assert_eq!(fs::read(&unrelated).unwrap(), b"unrelated");
        let mut permissions = fs::metadata(&map).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&map, permissions).unwrap();
        assert!(atomic_write(&map, b"lost").is_err());
        assert_eq!(fs::read(&map).unwrap(), b"updated");
    }
}

#[cfg(test)]
mod worker_tests;

#[cfg(test)]
mod settings_reload_tests {
    use super::*;

    #[test]
    fn settings_loader_is_bounded_and_keeps_absent_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().to_owned(),
        };
        assert_eq!(store.load_settings().unwrap(), Settings::default());
        let path = directory.path().join("settings.json");
        fs::write(&path, vec![b' '; MAX_SETTINGS_BYTES as usize + 1]).unwrap();
        assert!(store.load_settings().unwrap_err().contains("32768"));
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(store.load_settings().unwrap_err().contains("regular file"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn settings_loader_rejects_fifo_promptly_and_follows_regular_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().to_owned(),
        };
        let path = directory.path().join("settings.json");
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &path,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let fifo_store = store.clone();
        let worker = std::thread::spawn(move || sender.send(fifo_store.load_settings()).unwrap());
        let result = receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("FIFO settings read must reject promptly");
        worker.join().unwrap();
        assert!(result.unwrap_err().contains("regular file"));
        fs::remove_file(&path).unwrap();
        let target = directory.path().join("target.json");
        let settings = Settings {
            enabled: false,
            interval_seconds: 90,
            keep_per_map: 3,
        };
        fs::write(&target, serde_json::to_vec(&settings).unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert_eq!(store.load_settings().unwrap(), settings);
        let original = fs::metadata(&target).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&target, readonly).unwrap();
        assert_eq!(store.load_settings().unwrap(), settings);
        assert!(
            store
                .save_settings(&Settings::default())
                .unwrap_err()
                .contains("read-only")
        );
        assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
        fs::set_permissions(&target, original).unwrap();
    }
}
