use std::fs;
use std::io::Write;
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

pub(crate) fn admit_directory(directory: PathBuf) -> Result<PathBuf, String> {
    if !directory.is_absolute() {
        return Err("Recovery directory must be an absolute path".into());
    }
    if directory.to_str().is_none() {
        return Err("Recovery directory must be valid UTF-8".into());
    }
    Ok(directory)
}

fn select_directory(
    override_directory: Option<std::ffi::OsString>,
    state_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Result<PathBuf, String> {
    let requested = if let Some(directory) = override_directory {
        PathBuf::from(directory)
    } else if let Some(directory) =
        state_home.filter(|directory| Path::new(directory).is_absolute())
    {
        PathBuf::from(directory).join("hammerr/recovery")
    } else if let Some(home) = home {
        PathBuf::from(home).join(".local/state/hammerr/recovery")
    } else {
        return Err("Set XDG_STATE_HOME or HAMMERR_RECOVERY_DIR to enable recovery storage".into());
    };
    admit_directory(requested)
}

pub fn default_directory() -> Result<PathBuf, String> {
    let state = std::env::var_os("XDG_STATE_HOME");
    #[cfg(windows)]
    let state = state.or_else(|| std::env::var_os("LOCALAPPDATA"));
    select_directory(
        std::env::var_os("HAMMERR_RECOVERY_DIR"),
        state,
        std::env::var_os("HOME"),
    )
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
