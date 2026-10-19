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

fn absolute_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    })
}

fn path_key(path: &Path) -> String {
    format!("map:{}", absolute_path(path).display())
}

impl Session {
    pub fn configure_recovery(&mut self, directory: Result<PathBuf, String>) {
        self.configure_prefabs(directory.clone());
        self.configure_build_recipes(directory.clone());
        self.configure_material_favorites(directory.clone());
        self.configure_color_palette(directory.clone());
        self.configure_grid_preferences(directory.clone());
        self.configure_preview_preferences(directory.clone());
        self.configure_navigation_preferences(directory.clone());
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
            Err(error) if matches!(self.recovery.settings, Binding::Application(_)) => {
                self.recovery.storage_error = Some(error);
            }
            Err(_) => {}
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
        self.update_recovery_path();
    }

    pub(super) fn update_recovery_path(&mut self) {
        if let Some(path) = &self.path {
            self.recovery.source_path = Some(absolute_path(path));
            self.recovery.document_key = path_key(path);
            self.recovery.title = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into();
            self.recovery.changed();
        }
    }

    pub(super) fn backup_before_save(
        &mut self,
        path: &Path,
        destination: &source_formats::file_publication::BoundDestination,
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
        let source = absolute_path(path);
        let key = path_key(path);
        let title = path.file_name().unwrap_or_default().to_string_lossy();
        let result = store
            .write(&key, Some(&source), &title, Kind::BeforeSave, &data)
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

#[cfg(test)]
mod error_domain_tests;

#[cfg(test)]
mod authority_tests;

#[cfg(test)]
mod ownership_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use map_core::{ObjectId, surface};

    fn fixture() -> (tempfile::TempDir, Session, ObjectId) {
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        session.configure_recovery(Ok(directory.path().join("recovery")));
        let id = session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        (directory, session, id)
    }

    fn capture(session: &mut Session) -> Entry {
        let request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
        let result = request.run();
        session
            .finish_recovery_snapshot(Actor::User, &request, result)
            .unwrap()
    }

    #[test]
    fn serialization_errors_preserve_existing_files_and_finish_recovery_requests() {
        let (directory, mut session, _) = fixture();
        let destination = directory.path().join("previous.vmf");
        std::fs::write(&destination, b"previous map").unwrap();
        let mut vmf = session.document.vmf().clone();
        vmf.world.properties.set("message", "hello \"world\"");
        session.document = Document::new(vmf);
        assert!(
            session
                .save(Actor::User, Some(&destination))
                .unwrap_err()
                .contains("quotes")
        );
        assert_eq!(std::fs::read(destination).unwrap(), b"previous map");
        let request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
        let result = request.run();
        assert!(
            result
                .as_ref()
                .unwrap_err()
                .contains("serialize recovery snapshot")
        );
        assert!(
            session
                .finish_recovery_snapshot(Actor::User, &request, result)
                .is_err()
        );
        assert!(!session.recovery_busy());
        assert!(session.recovery_entries().unwrap().entries.is_empty());
    }

    #[test]
    fn recovery_request_writes_once_across_concurrent_and_repeated_calls() {
        let (_directory, mut session, _) = fixture();
        let before = session.document().vmf().clone();
        let history = session.history_snapshot();
        let request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                barrier.wait();
                request.run()
            });
            let second = scope.spawn(|| {
                barrier.wait();
                request.run()
            });
            [first.join().unwrap(), second.join().unwrap()]
        });
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let error = results
            .iter()
            .find_map(|result| result.as_ref().err())
            .unwrap();
        assert!(error.contains("already started"));
        let entry = results.into_iter().find_map(Result::ok).unwrap();
        assert_eq!(session.recovery_entries().unwrap().entries.len(), 1);
        session
            .finish_recovery_snapshot(Actor::User, &request, Ok(entry))
            .unwrap();
        assert!(request.run().unwrap_err().contains("already started"));
        assert_eq!(session.recovery_entries().unwrap().entries.len(), 1);
        capture(&mut session);
        assert_eq!(session.recovery_entries().unwrap().entries.len(), 2);
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.history_snapshot(), history);
    }

    #[test]
    fn failed_recovery_request_requires_new_admission_after_storage_repair() {
        let (directory, mut session, _) = fixture();
        let blocked = directory.path().join("blocked");
        std::fs::write(&blocked, b"not a directory").unwrap();
        session.recovery.store = Some(Store {
            directory: blocked.clone(),
        });
        let request = session.begin_recovery_snapshot(Kind::Manual).unwrap();
        let failure = request.run().unwrap_err();
        std::fs::remove_file(&blocked).unwrap();
        assert!(request.run().unwrap_err().contains("already started"));
        assert!(!blocked.exists());
        session
            .finish_recovery_snapshot(Actor::User, &request, Err(failure.clone()))
            .unwrap_err();
        assert_eq!(session.recovery_status()["snapshot_error"], failure);
        capture(&mut session);
        assert_eq!(session.recovery_entries().unwrap().entries.len(), 1);
        assert!(session.recovery_status()["snapshot_error"].is_null());
    }

    #[cfg(unix)]
    #[test]
    fn save_follows_file_links_and_rejects_unresolvable_destinations() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        for actor in [Actor::User, Actor::Agent] {
            let (directory, mut session, _) = fixture();
            let link = directory.path().join("linked.vmf");
            for destination in ["linked.vmf", "absent.vmf"] {
                symlink(destination, &link).unwrap();
                let before = session.document.vmf().clone();
                let revision = session.document.revision();
                let undo = session.document.undo_labels().count();
                let error = session.save(actor, Some(&link)).unwrap_err();
                assert!(error.contains("linked.vmf"), "{error}");
                assert_eq!(std::fs::read_link(&link).unwrap(), Path::new(destination));
                assert_eq!(session.document.vmf(), &before);
                assert_eq!(session.document.revision(), revision);
                assert_eq!(session.document.undo_labels().count(), undo);
                assert!(session.document.is_dirty());
                assert!(session.path().is_none());
                assert!(!directory.path().join("absent.vmf").exists());
                std::fs::remove_file(&link).unwrap();
            }
            let target = directory.path().join("target.vmf");
            let original = Document::empty().vmf().write(&Default::default());
            std::fs::write(&target, original.as_bytes()).unwrap();
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
            symlink("target.vmf", &link).unwrap();
            session.save(actor, Some(&link)).unwrap();
            assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("target.vmf"));
            assert_eq!(
                std::fs::read(&target).unwrap(),
                std::fs::read(&link).unwrap()
            );
            assert_eq!(
                std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o640
            );
            assert!(!session.document.is_dirty());
            assert_eq!(session.path(), Some(link.as_path()));
            assert_eq!(
                Vmf::parse(&std::fs::read_to_string(&target).unwrap())
                    .unwrap()
                    .world
                    .solids
                    .len(),
                1
            );
            let backups = session.recovery_entries().unwrap().entries;
            assert_eq!(backups.len(), 1);
            assert_eq!(
                session
                    .recovery
                    .store
                    .as_ref()
                    .unwrap()
                    .read(&backups[0].id)
                    .unwrap()
                    .1,
                original
            );
        }
    }

    #[test]
    fn duplicate_ids_in_open_and_recovery_preserve_the_active_document() {
        for actor in [Actor::User, Actor::Agent] {
            let (directory, mut session, id) = fixture();
            session
                .select(&[id], super::super::SelectMode::Replace)
                .unwrap();
            session
                .edit(actor, "temporary", |tx| {
                    tx.world_properties_mut().set("skyname", "temporary");
                    Ok(())
                })
                .unwrap();
            session.undo(actor).unwrap();
            session
                .create_primitive(actor, map_core::primitive::Spec::default(), true)
                .unwrap();
            let before = session.document.vmf().clone();
            let preview = session.render_document().vmf().clone();
            let state = (
                session.document_generation,
                session.document.revision(),
                session.selected_ids(),
                session.camera,
            );
            let text = "world { id 1 } entity { id 2 classname light } entity { id 2 classname info_target }";
            let path = directory.path().join("duplicate.vmf");
            std::fs::write(&path, text).unwrap();
            assert!(
                session
                    .open(actor, &path, true)
                    .unwrap_err()
                    .contains("duplicate object ID 2")
            );
            let entry = session
                .recovery
                .store
                .as_ref()
                .unwrap()
                .write(
                    "invalid",
                    None,
                    "Invalid IDs",
                    Kind::Manual,
                    text.as_bytes(),
                )
                .unwrap();
            assert!(
                session
                    .restore_recovery_snapshot(actor, &entry.id, true, None)
                    .unwrap_err()
                    .contains("duplicate object ID 2")
            );
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.render_document().vmf(), &preview);
            assert_eq!(
                (
                    session.document_generation,
                    session.document.revision(),
                    session.selected_ids(),
                    session.camera
                ),
                state
            );
            assert_eq!(session.document.undo_labels().count(), 1);
            assert_eq!(session.document.redo_labels().count(), 1);
            assert!(session.path().is_none());
        }
    }

    #[test]
    fn snapshots_exclude_previews_and_preserve_unsaved_state_history_and_versions() {
        let (_directory, mut session, ObjectId::Solid(id)) = fixture() else {
            panic!()
        };
        let face = surface::FaceId {
            solid: id,
            side: session.document().solid(id).unwrap().sides[0].id,
        };
        let original = session.document().vmf().clone();
        let revision = session.document().revision();
        let undo = session.document().undo_labels().count();
        session
            .edit_faces(
                Actor::Agent,
                &[face],
                surface::SurfaceEdit {
                    material: Some("metal/preview".into()),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
        let entry = capture(&mut session);
        assert_eq!(session.document().vmf(), &original);
        assert_eq!(session.document().revision(), revision);
        assert_eq!(session.document().undo_labels().count(), undo);
        assert!(session.document().is_dirty() && session.has_face_preview());
        let text = session
            .recovery
            .store
            .as_ref()
            .unwrap()
            .read(&entry.id)
            .unwrap()
            .1;
        let recovered = Vmf::parse(&text).unwrap();
        assert_eq!(recovered.world.solids[0].sides[0].material, "BRICK/TEST");
        assert_eq!(recovered.world.properties, original.world.properties);
        assert_eq!(session.recovery_status()["pending_changes"], false);
    }

    #[test]
    fn restore_requires_discard_and_snapshot_guards_and_opens_a_dirty_copy() {
        let (directory, mut session, _) = fixture();
        let path = directory.path().join("original.vmf");
        session.save(Actor::User, Some(&path)).unwrap();
        let saved = std::fs::read(&path).unwrap();
        let entry = capture(&mut session);
        let expected = Some((session.document_generation(), session.document().revision()));
        session
            .create_block(
                Actor::User,
                DVec3::splat(128.0),
                DVec3::splat(192.0),
                "extra",
                false,
            )
            .unwrap();
        let before = session.document().vmf().clone();
        assert!(
            session
                .restore_recovery_snapshot(Actor::Agent, &entry.id, false, None)
                .is_err()
        );
        assert!(
            session
                .restore_recovery_snapshot(Actor::Agent, &entry.id, true, expected)
                .is_err()
        );
        assert!(
            session
                .restore_recovery_snapshot(Actor::Agent, "../outside", true, None)
                .is_err()
        );
        assert_eq!(session.document().vmf(), &before);
        session
            .restore_recovery_snapshot(Actor::Agent, &entry.id, true, None)
            .unwrap();
        assert!(session.path().is_none() && session.document().is_dirty());
        assert_eq!(session.document().objects().count(), 1);
        assert_eq!(session.document().undo_labels().count(), 0);
        assert!(session.save(Actor::Agent, None).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), saved);
    }

    #[test]
    fn older_snapshot_completion_does_not_mark_newer_edits_or_replacement_as_captured() {
        let (_directory, mut session, _) = fixture();
        let request = session.begin_recovery_snapshot(Kind::Autosave).unwrap();
        assert!(session.begin_recovery_snapshot(Kind::Manual).is_err());
        session
            .create_block(
                Actor::User,
                DVec3::splat(128.0),
                DVec3::splat(192.0),
                "extra",
                false,
            )
            .unwrap();
        session
            .finish_recovery_snapshot(Actor::System, &request, request.run())
            .unwrap();
        assert_eq!(session.recovery_status()["pending_changes"], true);
        let request = session.begin_recovery_snapshot(Kind::Autosave).unwrap();
        session.new_map(Actor::User, true).unwrap();
        session
            .finish_recovery_snapshot(Actor::System, &request, request.run())
            .unwrap();
        assert!(session.recovery.last_entry.is_none());
        assert!(!session.recovery_busy());
        assert_eq!(session.recovery_entries().unwrap().entries.len(), 2);
    }

    #[test]
    fn autosave_scheduling_avoids_duplicate_snapshots_and_retries_errors() {
        let (_directory, mut session, _) = fixture();
        assert!(!session.autosave_due());
        session.recovery.last_attempt = Instant::now() - Duration::from_secs(121);
        assert!(session.autosave_due());
        let request = session.begin_recovery_snapshot(Kind::Autosave).unwrap();
        assert!(!session.autosave_due());
        assert!(
            session
                .finish_recovery_snapshot(Actor::System, &request, Err("disk full".into()))
                .is_err()
        );
        assert_eq!(session.recovery_status()["error"], "disk full");
        assert!(!session.autosave_due());
        session.recovery.last_attempt = Instant::now() - Duration::from_secs(121);
        assert!(session.autosave_due());
        let entry = capture(&mut session);
        session.recovery.last_attempt = Instant::now() - Duration::from_secs(121);
        assert!(!session.autosave_due());
        session
            .delete_recovery_snapshot(Actor::Agent, &entry.id)
            .unwrap();
        assert!(session.autosave_due());
        let mut settings = session.recovery_settings().clone();
        settings.enabled = false;
        session
            .set_recovery_settings(Actor::User, settings, None)
            .unwrap();
        assert!(!session.autosave_due());
    }

    #[test]
    fn failed_save_preserves_versions_dirty_state_and_existing_recovery() {
        let (directory, mut session, _) = fixture();
        let path = directory.path().join("map.vmf");
        session.save(Actor::User, Some(&path)).unwrap();
        session
            .create_block(
                Actor::User,
                DVec3::splat(128.0),
                DVec3::splat(192.0),
                "extra",
                false,
            )
            .unwrap();
        let mut settings = session.recovery_settings().clone();
        settings.keep_per_map = 1;
        session
            .set_recovery_settings(Actor::User, settings, None)
            .unwrap();
        let checkpoint = capture(&mut session);
        let before = session.document().vmf().clone();
        let disk = std::fs::read(&path).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        assert!(session.save(Actor::User, None).is_err());
        assert_eq!(session.document().vmf(), &before);
        assert!(session.document().is_dirty());
        assert_eq!(std::fs::read(&path).unwrap(), disk);
        let entries = session.recovery_entries().unwrap().entries;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, checkpoint.id);
        assert!(
            session
                .save(Actor::User, Some(&directory.path().join("missing/map.vmf")))
                .is_err()
        );
        assert_eq!(session.document().vmf(), &before);
    }

    #[test]
    fn successful_save_keeps_previous_bytes_and_backup_failure_does_not_lose_the_save() {
        let (directory, mut session, _) = fixture();
        let path = directory.path().join("map.vmf");
        session.save(Actor::User, Some(&path)).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        session
            .create_block(
                Actor::User,
                DVec3::splat(128.0),
                DVec3::splat(192.0),
                "extra",
                false,
            )
            .unwrap();
        session.save(Actor::Agent, None).unwrap();
        let entries = session.recovery_entries().unwrap().entries;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, Kind::BeforeSave);
        assert_eq!(
            session
                .recovery
                .store
                .as_ref()
                .unwrap()
                .read(&entries[0].id)
                .unwrap()
                .1,
            first
        );
        let blocked = directory.path().join("not-a-directory");
        std::fs::write(&blocked, "blocked").unwrap();
        session.configure_recovery(Ok(blocked));
        session
            .create_block(
                Actor::User,
                DVec3::splat(256.0),
                DVec3::splat(320.0),
                "extra",
                false,
            )
            .unwrap();
        session.save(Actor::User, None).unwrap();
        assert!(!session.document().is_dirty());
        assert_eq!(
            Vmf::parse(&std::fs::read_to_string(path).unwrap())
                .unwrap()
                .world
                .solids
                .len(),
            3
        );
        assert!(
            session
                .recent_log()
                .next()
                .unwrap()
                .text
                .contains("backup failed")
        );
    }

    #[test]
    fn settings_guards_preserve_newer_values_and_persist_across_sessions() {
        let (directory, mut session, _) = fixture();
        let version = session.recovery_settings_revision();
        let settings = Settings {
            enabled: false,
            interval_seconds: 60,
            keep_per_map: 4,
        };
        session
            .set_recovery_settings(Actor::Agent, settings.clone(), Some(version))
            .unwrap();
        assert!(
            session
                .set_recovery_settings(Actor::User, Settings::default(), Some(version))
                .is_err()
        );
        assert_eq!(session.recovery_settings(), settings);
        let mut reopened = Session::new(Err("no assets needed".into()));
        reopened.configure_recovery(Ok(directory.path().join("recovery")));
        assert_eq!(reopened.recovery_settings(), settings);
    }
}
