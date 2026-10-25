use std::fs::{File, Metadata};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::time::SystemTime;

use serde_json::{Value, json};

use super::DocumentId;

const MAX_DIAGNOSTIC_BYTES: usize = 2048;

fn bounded(mut message: String) -> String {
    if message.len() > MAX_DIAGNOSTIC_BYTES {
        let end = message
            .char_indices()
            .map(|(index, _)| index)
            .take_while(|index| *index <= MAX_DIAGNOSTIC_BYTES - "…".len())
            .last()
            .unwrap_or(0);
        message.truncate(end);
        message.push('…');
    }
    message
}

#[derive(Clone)]
pub(crate) struct Binding {
    requested: PathBuf,
    canonical: PathBuf,
    file: Arc<File>,
    executable: bool,
    identity: (u64, u64),
    length: u64,
    modified: SystemTime,
}

#[cfg(target_os = "linux")]
fn open_binding(path: &Path) -> std::io::Result<File> {
    use rustix::fs::{Mode, OFlags};
    rustix::fs::open(
        path,
        OFlags::PATH | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(Into::into)
}

#[cfg(not(target_os = "linux"))]
fn open_binding(_path: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Opened native installation bindings require Linux",
    ))
}

#[cfg(unix)]
fn physical_identity(metadata: &Metadata) -> Result<(u64, u64), String> {
    use std::os::unix::fs::MetadataExt;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn physical_identity(_metadata: &Metadata) -> Result<(u64, u64), String> {
    Err("Opened native installation identity requires Unix".into())
}

impl Binding {
    fn capture(path: &Path, executable: bool) -> Result<Self, String> {
        Self::capture_with(path, executable, open_binding)
    }

    fn capture_with(
        path: &Path,
        executable: bool,
        open: impl FnOnce(&Path) -> std::io::Result<File>,
    ) -> Result<Self, String> {
        let requested = std::path::absolute(path).map_err(|error| error.to_string())?;
        let canonical = requested
            .canonicalize()
            .map_err(|error| format!("{}: {error}", requested.display()))?;
        crate::report_path::checked(&canonical)?;
        let file = open(&canonical).map_err(|error| format!("{}: {error}", canonical.display()))?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if (executable && !metadata.is_file()) || (!executable && !metadata.is_dir()) {
            return Err(format!(
                "Invalid native installation binding: {}",
                canonical.display()
            ));
        }
        let binding = Self {
            requested,
            canonical,
            file: Arc::new(file),
            executable,
            identity: physical_identity(&metadata)?,
            length: metadata.len(),
            modified: metadata.modified().map_err(|error| error.to_string())?,
        };
        binding.verify()?;
        Ok(binding)
    }

    fn verify(&self) -> Result<(), String> {
        let current = self.requested.canonicalize().map_err(|error| {
            format!(
                "Native installation binding changed at {}: {error}",
                self.requested.display()
            )
        })?;
        let expected = self.file.metadata().map_err(|error| error.to_string())?;
        let actual = std::fs::metadata(&current).map_err(|error| error.to_string())?;
        if current != self.canonical
            || physical_identity(&expected)? != physical_identity(&actual)?
            || (self.executable
                && (actual.len() != self.length
                    || actual.modified().map_err(|error| error.to_string())? != self.modified))
        {
            return Err(format!(
                "Native installation binding changed at {}",
                self.requested.display()
            ));
        }
        Ok(())
    }

    fn describe(&self) -> Value {
        json!({"path":self.canonical,"device":self.identity.0,"inode":self.identity.1})
    }
}

#[derive(Clone)]
pub(crate) struct Target {
    engine: Binding,
    game: Binding,
    executable: Binding,
    steam: Option<Binding>,
    mounts: Vec<Binding>,
}

impl Target {
    pub(crate) fn play(engine: &Path, game: &Path, executable: &Path) -> Result<Self, String> {
        Ok(Self {
            engine: Binding::capture(engine, false)?,
            game: Binding::capture(game, false)?,
            executable: Binding::capture(executable, true)?,
            steam: None,
            mounts: Vec::new(),
        })
    }

    pub(crate) fn capture(
        environment: &native_runtime::CaptureEnvironment,
    ) -> Result<Self, String> {
        let mut target = Self::play(
            &environment.engine_root,
            &environment.engine_root.join("cstrike"),
            &environment.engine_root.join("cstrike_linux64"),
        )?;
        target.steam = Some(Binding::capture(&environment.steam_root, false)?);
        target.mounts = environment
            .mounts
            .iter()
            .map(|mount| Binding::capture(&mount.path, mount.path.is_file()))
            .collect::<Result<_, _>>()?;
        Ok(target)
    }

    pub(crate) fn verify(&self) -> Result<(), String> {
        self.engine.verify()?;
        self.game.verify()?;
        self.executable.verify()?;
        if let Some(steam) = &self.steam {
            steam.verify()?;
        }
        for mount in &self.mounts {
            mount.verify()?;
        }
        Ok(())
    }

    fn describe(&self) -> Value {
        json!({"engine":self.engine.describe(),"game":self.game.describe(),"executable":self.executable.describe(),"steam":self.steam.as_ref().map(Binding::describe),"mounts":self.mounts.iter().map(Binding::describe).collect::<Vec<_>>()})
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Phase {
    Capture,
    PlayStartup,
    Play,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::PlayStartup => "play_startup",
            Self::Play => "play",
        }
    }
}

struct Record {
    id: u64,
    document: DocumentId,
    phase: &'static str,
    targets: Value,
    failure: Option<String>,
}

impl Record {
    fn message(&self) -> String {
        let state = match self.phase {
            "capture" => "Native activity busy: reflection capture",
            "play_startup" => "Native activity busy: game startup",
            "play" => "Native activity busy: managed game running",
            _ => "Native activity unavailable: cleanup failed",
        };
        let mut message = format!(
            "{state} (activity {}, document {}).",
            self.id,
            self.document.value()
        );
        if let Some(failure) = &self.failure {
            message.push_str(&format!(" {failure}"));
        }
        for (key, label) in [("capture", "Capture"), ("play", "Play")] {
            if let Some(path) = self.targets[key]["engine"]["path"].as_str() {
                message.push_str(&format!(" {label}: {path}."));
            }
        }
        bounded(message)
    }
}

#[derive(Default)]
struct State {
    serial: u64,
    record: Option<Record>,
}

#[derive(Default)]
struct Core {
    claimed: AtomicBool,
    owner_alive: AtomicBool,
    state: Mutex<State>,
}

struct Owner {
    core: Arc<Core>,
    process: bool,
}

impl Drop for Owner {
    fn drop(&mut self) {
        let state = self
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.core.owner_alive.store(false, Ordering::Release);
        if self.process && state.record.is_none() {
            self.core.claimed.store(false, Ordering::Release);
        }
    }
}

#[derive(Clone)]
pub(crate) struct Authority(Arc<Owner>);

impl Authority {
    pub(crate) fn process() -> Result<Self, String> {
        static PROCESS: OnceLock<Arc<Core>> = OnceLock::new();
        let core = PROCESS.get_or_init(|| Arc::new(Core::default())).clone();
        if core
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("The process native activity already has an application owner or unconfirmed cleanup".into());
        }
        core.owner_alive.store(true, Ordering::Release);
        Ok(Self(Arc::new(Owner {
            core,
            process: true,
        })))
    }

    pub(crate) fn isolated() -> Self {
        let core = Arc::new(Core::default());
        core.owner_alive.store(true, Ordering::Release);
        Self(Arc::new(Owner {
            core,
            process: false,
        }))
    }

    pub(crate) fn client(&self, document: DocumentId) -> Client {
        Client {
            core: self.0.core.clone(),
            owner: Arc::downgrade(&self.0),
            document,
        }
    }

    pub(crate) fn cleanup_result(&self) -> Result<(), String> {
        let state = self
            .0
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.record.as_ref().map_or(Ok(()), |record| {
            Err(record.failure.clone().unwrap_or_else(|| {
                format!(
                    "Native activity {} remains occupied during {}",
                    record.id, record.phase
                )
            }))
        })
    }
}

#[derive(Clone)]
pub(crate) struct Client {
    core: Arc<Core>,
    owner: Weak<Owner>,
    document: DocumentId,
}

impl Client {
    pub(crate) fn reserve(
        &self,
        phase: Phase,
        capture: Option<&Target>,
        play: Option<&Target>,
    ) -> Result<Permit, String> {
        let targets =
            json!({"capture":capture.map(Target::describe),"play":play.map(Target::describe)});
        let _owner = self
            .owner
            .upgrade()
            .ok_or("The application native activity owner is closed")?;
        let mut state = self
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !self.core.owner_alive.load(Ordering::Acquire) {
            return Err("The application native activity owner is closed".into());
        }
        if let Some(record) = &state.record {
            return Err(record.message());
        }
        state.serial = state
            .serial
            .checked_add(1)
            .ok_or("Native activity identity exhausted")?;
        let id = state.serial;
        state.record = Some(Record {
            id,
            document: self.document,
            phase: phase.name(),
            targets,
            failure: None,
        });
        Ok(Permit {
            client: self.clone(),
            id,
            clean: true,
            failed: false,
        })
    }

    pub(crate) fn status(&self) -> Value {
        let state = self
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        json!({"available":state.record.is_none(),"message":state.record.as_ref().map_or_else(||"Native activity available.".to_owned(), Record::message),"owner":state.record.as_ref().map(|record|json!({"activity_id":record.id,"document_id":record.document.value(),"phase":record.phase,"targets":record.targets,"cleanup_error":record.failure}))})
    }

    pub(crate) fn close_work(&self) -> Value {
        let state = self
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state
            .record
            .as_ref()
            .filter(|record| record.document == self.document)
            .map_or(Value::Null, |record| {
                json!({
                    "activity_id": record.id, "phase": record.phase,
                    "cleanup_error": record.failure,
                })
            })
    }

    pub(crate) fn cleanup_result(&self) -> Result<(), String> {
        let state = self
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state
            .record
            .as_ref()
            .filter(|record| record.document == self.document)
            .and_then(|record| record.failure.clone())
            .map_or(Ok(()), Err)
    }
}

pub(crate) struct Permit {
    client: Client,
    id: u64,
    clean: bool,
    failed: bool,
}

impl Permit {
    pub(crate) fn phase(&mut self, phase: Phase) {
        let mut state = self
            .client
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(record) = state
            .record
            .as_mut()
            .filter(|record| record.id == self.id && record.document == self.client.document)
            && record.failure.is_none()
        {
            record.phase = phase.name();
        }
    }

    pub(crate) fn started(&mut self) {
        self.clean = false;
    }

    pub(crate) fn confirmed_clean(&mut self) {
        if !self.failed {
            self.clean = true;
        }
    }

    pub(crate) fn failed_cleanup(&mut self, error: &str) {
        self.clean = false;
        self.failed = true;
        let mut state = self
            .client
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(record) = state
            .record
            .as_mut()
            .filter(|record| record.id == self.id && record.document == self.client.document)
        {
            record.phase = "failed_cleanup";
            if record.failure.is_none() {
                record.failure = Some(bounded(error.to_owned()));
            }
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if !self.clean {
            self.failed_cleanup("Native activity cleanup was not confirmed");
            return;
        }
        let mut state = self
            .client
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state
            .record
            .as_ref()
            .is_some_and(|record| record.id == self.id && record.document == self.client.document)
        {
            state.record = None;
            if !self.client.core.owner_alive.load(Ordering::Acquire) {
                self.client.core.claimed.store(false, Ordering::Release);
            }
        }
    }
}

#[cfg(test)]
mod tests;
