use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use source_formats::keyvalues::WriteOptions;
use source_formats::vmf::Vmf;

use super::{Actor, Document, Session, Snapshot};
use crate::recovery::{
    Authority, Entry, Kind, Listing, Settings, SettingsSnapshot, SettingsUpdate, Store,
};

pub(super) struct State {
    store: Option<Store>,
    settings: Binding,
    observer: RefCell<Observer>,
    document_key: String,
    source_path: Option<PathBuf>,
    title: String,
    last_attempt: Instant,
    captured: Option<Snapshot>,
    last_entry: Option<Entry>,
    storage_error: Option<String>,
    snapshot_error: Option<String>,
    retention_error: Option<String>,
    drain_error: Option<String>,
    pending: Option<Arc<Origin>>,
    serial: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            store: None,
            settings: Binding::Independent(Authority::default()),
            observer: RefCell::new(Observer::default()),
            document_key: format!("untitled:{}", crate::recovery::unique_id()),
            source_path: None,
            title: "Untitled map".into(),
            last_attempt: Instant::now(),
            captured: None,
            last_entry: None,
            storage_error: None,
            snapshot_error: None,
            retention_error: None,
            drain_error: None,
            pending: None,
            serial: 0,
        }
    }
}

impl State {
    fn changed(&mut self) {
        self.serial = self
            .serial
            .checked_add(1)
            .expect("Recovery serial exhausted");
    }

    fn error(&self, settings_error: Option<&str>) -> Option<String> {
        let mut errors: Vec<&str> = Vec::new();
        for (error, prefix) in [
            (self.drain_error.as_deref(), None),
            (
                self.snapshot_error.as_deref(),
                Some("Exit recovery snapshot failed: "),
            ),
            (
                self.retention_error.as_deref(),
                Some("Exit recovery snapshot saved, but recovery cleanup failed: "),
            ),
            (self.storage_error.as_deref(), None),
            (settings_error, None),
        ] {
            let Some(error) = error else { continue };
            let wrapped = prefix.is_some_and(|prefix| {
                let wrapped = format!("{prefix}{error}");
                self.drain_error.as_ref().is_some_and(|drain| {
                    drain == &wrapped || drain.ends_with(&format!("; {wrapped}"))
                })
            });
            if !wrapped && !errors.contains(&error) {
                errors.push(error);
            }
        }
        (!errors.is_empty()).then(|| errors.join("; "))
    }
}

enum Binding {
    Independent(Authority),
    Application(Authority),
}

impl Binding {
    fn authority(&self) -> &Authority {
        match self {
            Self::Independent(authority) | Self::Application(authority) => authority,
        }
    }
}

#[derive(Default)]
struct Observer {
    observed: Option<(u64, bool, SettingsStamp)>,
    serial: u64,
}

#[derive(PartialEq, Eq)]
struct SettingsStamp {
    revision: u64,
    error: Option<String>,
}

impl Observer {
    fn observe(&mut self, local: u64, busy: bool, settings: SettingsSnapshot) -> u64 {
        let observed = (
            local,
            busy,
            SettingsStamp {
                revision: settings.revision,
                error: settings.error,
            },
        );
        if self.observed.as_ref() != Some(&observed) {
            self.serial = self
                .serial
                .checked_add(1)
                .expect("Recovery observer serial exhausted");
            self.observed = Some(observed);
        }
        self.serial
    }
}

pub(super) struct RecoveryBackup {
    entry: Entry,
    store: Store,
    keep: usize,
}

pub(crate) struct RecoveryCompletion {
    pub entry: Entry,
    pub retention_error: Option<String>,
}

pub struct RecoveryRequest {
    store: Store,
    snapshot: Snapshot,
    document_key: String,
    source_path: Option<PathBuf>,
    title: String,
    kind: Kind,
    vmf: Vmf,
    keep: usize,
    origin: Arc<Origin>,
}

#[derive(Clone, Copy)]
#[repr(u8)]
enum Phase {
    Admitted,
    Writing,
    Ready,
    Finishing,
    Terminal,
}

struct Origin {
    phase: AtomicU8,
    outcome: Mutex<Option<Result<Entry, String>>>,
}

impl Origin {
    fn new() -> Self {
        Self {
            phase: AtomicU8::new(Phase::Admitted as u8),
            outcome: Mutex::new(None),
        }
    }

    fn live(&self) -> bool {
        self.phase.load(Ordering::Acquire) != Phase::Terminal as u8
    }

    fn transition(&self, from: Phase, to: Phase) -> bool {
        self.phase
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn claim_finish(
        &self,
        supplied: &Result<Entry, String>,
    ) -> Result<Result<Entry, String>, String> {
        let phase = self.phase.load(Ordering::Acquire);
        let (from, outcome) = if phase == Phase::Admitted as u8 && supplied.is_err() {
            (Phase::Admitted, supplied.clone())
        } else if phase == Phase::Ready as u8 {
            let outcome = self.outcome.lock().expect("Recovery outcome lock poisoned");
            let outcome = outcome.as_ref().expect("Ready recovery outcome missing");
            if outcome != supplied {
                return Err("Recovery snapshot result does not belong to this request".into());
            }
            (Phase::Ready, outcome.clone())
        } else {
            return Err("Recovery snapshot request is not ready to finish".into());
        };
        if !self.transition(from, Phase::Finishing) {
            return Err("Recovery snapshot request is no longer ready to finish".into());
        }
        Ok(outcome)
    }
}

struct WriteScope<'a> {
    origin: &'a Origin,
    outcome: Option<Result<Entry, String>>,
}

impl Drop for WriteScope<'_> {
    fn drop(&mut self) {
        let outcome = self
            .outcome
            .take()
            .unwrap_or_else(|| Err("Recovery snapshot worker panicked".into()));
        *self
            .origin
            .outcome
            .lock()
            .expect("Recovery outcome lock poisoned") = Some(outcome);
        self.origin
            .phase
            .store(Phase::Ready as u8, Ordering::Release);
    }
}

struct FinishScope<'a>(&'a Origin);

impl Drop for FinishScope<'_> {
    fn drop(&mut self) {
        self.0.phase.store(Phase::Terminal as u8, Ordering::Release);
    }
}

impl RecoveryRequest {
    pub fn run(&self) -> Result<Entry, String> {
        self.run_with(|| {})
    }

    pub(crate) fn run_with(&self, before_write: impl FnOnce()) -> Result<Entry, String> {
        if !self.origin.transition(Phase::Admitted, Phase::Writing) {
            return Err("Recovery snapshot request has already started or retired".into());
        }
        let mut writing = WriteScope {
            origin: &self.origin,
            outcome: None,
        };
        before_write();
        let outcome = self
            .vmf
            .try_write(&WriteOptions::default())
            .map_err(|error| format!("Cannot serialize recovery snapshot: {error}"))
            .and_then(|text| {
                self.store.write(
                    &self.document_key,
                    self.source_path.as_deref(),
                    &self.title,
                    self.kind.clone(),
                    text.as_bytes(),
                )
            });
        writing.outcome = Some(outcome.clone());
        outcome
    }
}

impl Drop for RecoveryRequest {
    fn drop(&mut self) {
        if !self.origin.transition(Phase::Admitted, Phase::Terminal) {
            self.origin.transition(Phase::Ready, Phase::Terminal);
        }
    }
}

fn path_key(path: &Path) -> String {
    if let Some(path) = path.to_str() {
        return format!("map:{path}");
    }
    use std::fmt::Write;
    let mut key = String::from("map-bytes:");
    for byte in path.as_os_str().as_encoded_bytes() {
        write!(key, "{byte:02x}").expect("Writing to a String cannot fail");
    }
    key
}

pub(super) struct PathMetadata {
    document_key: String,
    source_path: PathBuf,
    title: String,
}

impl PathMetadata {
    pub(super) fn from_destination(
        destination: &source_formats::file_publication::BoundDestination,
    ) -> Result<Self, String> {
        let source_path = [destination.physical_path(), destination.authored_path()]
            .into_iter()
            .find(|path| path.to_str().is_some())
            .ok_or("Recovery source path must be valid UTF-8; use a UTF-8 alias to open or save this map")?
            .to_owned();
        Ok(Self {
            document_key: path_key(destination.physical_path()),
            source_path,
            title: destination
                .authored_path()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
        })
    }
}

impl Session {
    pub fn configure_recovery(&mut self, directory: Result<PathBuf, String>) {
        let directory = directory.and_then(crate::recovery::admit_directory);
        self.configure_prefabs(directory.clone());
        self.configure_build_recipes(directory.clone());
        self.configure_material_favorites(directory.clone());
        self.configure_color_palette(directory.clone());
        self.configure_grid_preferences(directory.clone());
        self.configure_pivot_preferences(directory.clone());
        self.configure_preview_preferences(directory.clone());
        self.configure_navigation_preferences(directory.clone());
        self.configure_selection_preferences(directory.clone());
        self.configure_shortcuts(directory.clone());
        self.configure_recent_maps(crate::recent_maps::directory(directory.clone()));
        if let Binding::Independent(authority) = &self.recovery.settings
            && let Err(error) = authority.configure(directory.clone())
        {
            self.log(
                Actor::System,
                format!("Recovery settings could not load: {error}"),
            );
        }
        match directory {
            Ok(directory) => {
                self.recovery.store = Some(Store { directory });
                self.recovery.storage_error = None;
            }
            Err(error) => {
                self.recovery.store = None;
                self.recovery.storage_error = Some(error);
            }
        }
        self.recovery.changed();
    }

    pub fn with_recovery_settings(
        game: Result<source_formats::gamefs::GameFs, String>,
        authority: Authority,
    ) -> Self {
        let mut session = Self::new(game);
        session.use_application_recovery_settings(authority);
        session
    }

    pub(crate) fn use_application_recovery_settings(&mut self, authority: Authority) {
        self.recovery.settings = Binding::Application(authority);
        self.recovery.changed();
        if let Some(error) = self.recovery_settings_snapshot().error {
            self.log(
                Actor::System,
                format!("Recovery settings could not load: {error}"),
            );
        }
    }

    pub(crate) fn report_recovery_worker_error(&mut self, error: String) {
        self.recovery.drain_error = Some(error.clone());
        self.recovery.changed();
        self.log(Actor::System, format!("Recovery shutdown failed: {error}"));
    }

    pub fn recovery_serial(&self) -> u64 {
        self.recovery.observer.borrow_mut().observe(
            self.recovery.serial,
            self.recovery_busy(),
            self.recovery_settings_snapshot(),
        )
    }

    pub fn recovery_settings_snapshot(&self) -> SettingsSnapshot {
        self.recovery.settings.authority().snapshot()
    }

    pub fn recovery_settings(&self) -> Settings {
        self.recovery_settings_snapshot().settings
    }

    pub fn recovery_busy(&self) -> bool {
        self.recovery
            .pending
            .as_ref()
            .is_some_and(|origin| origin.live())
    }

    pub fn recovery_settings_revision(&self) -> u64 {
        self.recovery_settings_snapshot().revision
    }

    pub fn recovery_status(&self) -> Value {
        let settings = self.recovery_settings_snapshot();
        self.recovery_status_with_settings(&settings)
    }

    pub fn recovery_status_snapshot(&self) -> (SettingsSnapshot, Value) {
        let settings = self.recovery_settings_snapshot();
        let status = self.recovery_status_with_settings(&settings);
        (settings, status)
    }

    fn recovery_status_with_settings(&self, settings: &SettingsSnapshot) -> Value {
        let busy = self.recovery_busy();
        let pending = self.document.is_dirty()
            && !self
                .recovery
                .captured
                .as_ref()
                .is_some_and(|s| self.current_snapshot(s));
        json!({
            "generation": self.document_generation, "revision": self.document.revision(),
            "available": self.recovery.store.is_some(), "settings": settings.settings,
            "settings_revision": settings.revision,
            "directory": self.recovery.store.as_ref().map(|store| &store.directory),
            "busy": busy, "pending_changes": pending,
            "next_autosave_seconds": (pending && self.recovery.store.is_some() && settings.settings.enabled && !busy && !self.closing).then(|| Duration::from_secs(settings.settings.interval_seconds).saturating_sub(self.recovery.last_attempt.elapsed()).as_secs()),
            "last_snapshot": self.recovery.last_entry, "error": self.recovery.error(settings.error.as_deref()),
            "settings_error": settings.error, "storage_error": self.recovery.storage_error,
            "snapshot_error": self.recovery.snapshot_error,
            "retention_error": self.recovery.retention_error, "drain_error": self.recovery.drain_error,
            "document_key": self.recovery.document_key,
        })
    }

    pub fn set_recovery_settings(
        &mut self,
        actor: Actor,
        settings: Settings,
        expected_revision: Option<u64>,
    ) -> Result<(), String> {
        self.update_recovery_settings(actor, settings.into(), expected_revision)
            .map(|_| ())
    }

    pub fn update_recovery_settings(
        &mut self,
        actor: Actor,
        update: SettingsUpdate,
        expected_revision: Option<u64>,
    ) -> Result<Value, String> {
        let settings = self
            .recovery
            .settings
            .authority()
            .update(update, expected_revision)?;
        self.log(actor, "updated autosave and recovery settings");
        Ok(self.recovery_status_with_settings(&settings))
    }

    pub fn reload_recovery_settings(
        &mut self,
        actor: Actor,
        expected_revision: Option<u64>,
    ) -> Result<Value, String> {
        let loaded = self.recovery.settings.authority().reload(expected_revision);
        match loaded {
            Ok(settings) => {
                self.log(actor, "Reloaded saved recovery settings");
                Ok(self.recovery_status_with_settings(&settings))
            }
            Err(crate::recovery::ReloadError::Storage(error)) => {
                self.log(actor, &error);
                Err(error)
            }
            Err(crate::recovery::ReloadError::Stale(error)) => Err(error),
        }
    }

    pub fn recovery_entries(&self) -> Result<Listing, String> {
        self.recovery
            .store
            .as_ref()
            .ok_or("Recovery storage is unavailable")?
            .list()
    }

    pub fn autosave_due(&self) -> bool {
        self.autosave_due_with_settings(&self.recovery_settings())
    }

    fn autosave_due_with_settings(&self, settings: &Settings) -> bool {
        self.recovery.store.is_some()
            && settings.enabled
            && !self.recovery_busy()
            && !self.closing
            && self.document.is_dirty()
            && self.recovery.last_attempt.elapsed()
                >= Duration::from_secs(settings.interval_seconds)
            && !self
                .recovery
                .captured
                .as_ref()
                .is_some_and(|s| self.current_snapshot(s))
    }

    pub(crate) fn begin_due_recovery_snapshot(&mut self) -> Option<RecoveryRequest> {
        let settings = self.recovery_settings();
        self.autosave_due_with_settings(&settings)
            .then(|| {
                self.begin_recovery_snapshot_with_settings(Kind::Autosave, settings)
                    .ok()
            })
            .flatten()
    }

    pub(crate) fn begin_exit_recovery_snapshot(
        &mut self,
    ) -> Result<Option<RecoveryRequest>, String> {
        let settings = self.recovery_settings();
        let pending = self.document.is_dirty()
            && !self
                .recovery
                .captured
                .as_ref()
                .is_some_and(|snapshot| self.current_snapshot(snapshot));
        if !settings.enabled || !pending {
            return Ok(None);
        }
        if self.recovery.store.is_none() {
            return Err("Exit recovery storage is unavailable".into());
        }
        self.begin_recovery_snapshot_with_settings(Kind::Autosave, settings)
            .map(Some)
            .map_err(|error| format!("Exit recovery snapshot failed: {error}"))
    }

    pub fn begin_recovery_snapshot(&mut self, kind: Kind) -> Result<RecoveryRequest, String> {
        let settings = self.recovery_settings();
        self.begin_recovery_snapshot_with_settings(kind, settings)
    }

    fn begin_recovery_snapshot_with_settings(
        &mut self,
        kind: Kind,
        settings: Settings,
    ) -> Result<RecoveryRequest, String> {
        if self.closing && kind != Kind::Autosave {
            return Err("The editor is closing".into());
        }
        if self.recovery_busy() {
            return Err("A recovery snapshot is already being written".into());
        }
        let store = self
            .recovery
            .store
            .clone()
            .ok_or("Recovery storage is unavailable")?;
        let origin = Arc::new(Origin::new());
        self.recovery.pending = Some(origin.clone());
        self.recovery.last_attempt = Instant::now();
        self.recovery.changed();
        Ok(RecoveryRequest {
            store,
            snapshot: Snapshot {
                generation: self.document_generation,
                revision: self.document.revision(),
            },
            document_key: self.recovery.document_key.clone(),
            source_path: self.recovery.source_path.clone(),
            title: self.recovery.title.clone(),
            kind,
            vmf: self.document.vmf().clone(),
            keep: settings.keep_per_map,
            origin,
        })
    }

    pub fn finish_recovery_snapshot(
        &mut self,
        actor: Actor,
        request: &RecoveryRequest,
        result: Result<Entry, String>,
    ) -> Result<Entry, String> {
        self.finish_recovery_snapshot_completed(actor, request, result)
            .map(|completion| completion.entry)
    }

    pub(crate) fn finish_recovery_snapshot_completed(
        &mut self,
        actor: Actor,
        request: &RecoveryRequest,
        result: Result<Entry, String>,
    ) -> Result<RecoveryCompletion, String> {
        if !self
            .recovery
            .pending
            .as_ref()
            .is_some_and(|origin| Arc::ptr_eq(origin, &request.origin))
        {
            return Err("Recovery snapshot request does not belong to this pending capture".into());
        }
        let result = request.origin.claim_finish(&result)?;
        let _finishing = FinishScope(&request.origin);
        self.recovery.last_attempt = Instant::now();
        self.recovery.changed();
        match result {
            Ok(entry) => {
                self.recovery.snapshot_error = None;
                self.recovery.retention_error = None;
                if request.snapshot.generation == self.document_generation
                    && request.document_key == self.recovery.document_key
                {
                    self.recovery.captured = Some(request.snapshot.clone());
                    self.recovery.last_entry = Some(entry.clone());
                }
                let retention_error = request
                    .store
                    .prune(&request.document_key, request.keep)
                    .err();
                if let Some(error) = &retention_error {
                    self.recovery.retention_error = Some(error.clone());
                    self.log(
                        Actor::System,
                        format!("Recovery snapshot saved, but retention failed: {error}"),
                    );
                } else {
                    self.log(
                        actor,
                        format!("recovery snapshot saved for {}", entry.title),
                    );
                }
                Ok(RecoveryCompletion {
                    entry,
                    retention_error,
                })
            }
            Err(error) => {
                self.recovery.snapshot_error = Some(error.clone());
                self.log(actor, format!("Recovery snapshot failed: {error}"));
                Err(error)
            }
        }
    }

    pub fn delete_recovery_snapshot(&mut self, actor: Actor, id: &str) -> Result<(), String> {
        if self.recovery_busy() {
            return Err("Wait for the current recovery snapshot to finish".into());
        }
        self.recovery
            .store
            .as_ref()
            .ok_or("Recovery storage is unavailable")?
            .delete(id)?;
        if self
            .recovery
            .last_entry
            .as_ref()
            .is_some_and(|entry| entry.id == id)
        {
            self.recovery.last_entry = None;
            self.recovery.captured = None;
        }
        self.recovery.changed();
        self.log(actor, "deleted recovery snapshot");
        Ok(())
    }

    pub fn restore_recovery_snapshot(
        &mut self,
        actor: Actor,
        id: &str,
        discard_changes: bool,
        expected: Option<(u64, u64)>,
    ) -> Result<(), String> {
        self.ensure_not_closing()?;
        if expected.is_some_and(|(generation, revision)| {
            generation != self.document_generation || revision != self.document.revision()
        }) {
            return Err(
                "Map changed since recovery was inspected; refresh before replacing it".into(),
            );
        }
        self.ensure_discardable(discard_changes)?;
        let (entry, text) = self
            .recovery
            .store
            .as_ref()
            .ok_or("Recovery storage is unavailable")?
            .read(id)?;
        let vmf = Vmf::parse(&text).map_err(|e| format!("Recovery map could not be read: {e}"))?;
        let mut document = Document::try_new(vmf)
            .map_err(|error| format!("Recovery map has invalid IDs: {error}"))?;
        document.context().map_err(|error| error.to_string())?;
        document.mark_unsaved();
        self.replace_document(document, None);
        self.recovery.title = format!("Recovered {}", entry.title);
        self.recovery.source_path = entry.source_path;
        self.log(
            actor,
            format!(
                "recovered {} as an unsaved copy; choose Save As to keep it",
                entry.title
            ),
        );
        Ok(())
    }

    pub(super) fn reset_recovery_document(&mut self) {
        self.recovery.document_key = format!("untitled:{}", crate::recovery::unique_id());
        self.recovery.source_path = None;
        self.recovery.title = "Untitled map".into();
        self.recovery.last_attempt = Instant::now();
        self.recovery.captured = None;
        self.recovery.last_entry = None;
        self.recovery.changed();
    }

    pub(super) fn update_recovery_path(&mut self, metadata: PathMetadata) {
        self.recovery.source_path = Some(metadata.source_path);
        self.recovery.document_key = metadata.document_key;
        self.recovery.title = metadata.title;
        self.recovery.changed();
    }

    pub(super) fn backup_before_save(
        &mut self,
        destination: &source_formats::file_publication::BoundDestination,
        metadata: &PathMetadata,
    ) -> Result<Option<RecoveryBackup>, String> {
        let Some(store) = self.recovery.store.clone() else {
            return Ok(None);
        };
        let keep = self.recovery_settings().keep_per_map;
        let data = match destination.read_existing() {
            Ok(Some(data)) => data,
            Ok(None) => return Ok(None),
            Err(error) => return Err(format!("Could not read previous save: {error}")),
        };
        let result = store
            .write(
                &metadata.document_key,
                Some(&metadata.source_path),
                &metadata.title,
                Kind::BeforeSave,
                &data,
            )
            .map(|entry| Some(RecoveryBackup { entry, store, keep }));
        self.recovery.changed();
        if let Err(error) = &result {
            self.recovery.snapshot_error = Some(error.clone());
        }
        result
    }

    pub(super) fn finish_save_backup(
        &mut self,
        backup: Option<&RecoveryBackup>,
        saved: bool,
    ) -> Result<(), String> {
        let Some(backup) = backup else {
            return Ok(());
        };
        let result = if saved {
            backup.store.prune(&backup.entry.document_key, backup.keep)
        } else {
            backup.store.delete(&backup.entry.id)
        };
        self.recovery.changed();
        if let Err(error) = &result {
            self.recovery.retention_error = Some(error.clone());
        }
        result
    }
}

impl State {
    pub(super) fn close_work(&self) -> serde_json::Value {
        serde_json::json!({
            "sequence": self.serial,
            "document_key": self.document_key,
            "pending": self.pending.as_ref().map(|pending| pending.live()),
            "shutdown_error": self.drain_error,
            "settings_revision": self.settings.authority().snapshot().revision,
        })
    }
}
