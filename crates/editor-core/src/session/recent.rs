use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use map_core::Document;
use render::Camera;
use serde_json::{Value, json};
use source_formats::file_publication::{BoundDestination, Policy};
use source_formats::vmf::Vmf;

use crate::recent_maps::{self, Authority, BuildSettings, Entry, Snapshot};

use super::content_profiles::Candidate;
use super::worker::{Slot, Ticket};
use super::{Actor, BufferReplacementReview, Session};

#[derive(Default)]
pub(super) struct State {
    worker: Slot,
    pending: Option<Pending>,
    serial: u64,
    error: Option<String>,
}

struct Pending {
    identity: Arc<()>,
    control: compiler::Control,
    name: &'static str,
}

enum Operation {
    Open {
        discard_changes: bool,
        review: BufferReplacementReview,
    },
    Relocate,
}

pub struct Request {
    token: u64,
    identity: Arc<()>,
    entry: Entry,
    serial: u64,
    destination: BoundDestination,
    operation: Operation,
    intent: u64,
    control: compiler::Control,
    started: Instant,
    _worker: Ticket,
}

pub struct Completed {
    request: Request,
    result: Result<Prepared, String>,
}

struct Prepared {
    map: PreparedMap,
    content: Option<Candidate>,
}

pub(super) struct PreparedMap {
    pub(super) document: Document,
    pub(super) camera: Option<Camera>,
    pub(super) recovery: super::recovery::PathMetadata,
    destination: BoundDestination,
}

impl PreparedMap {
    pub(super) fn load(
        destination: BoundDestination,
        control: &compiler::Control,
    ) -> Result<Self, String> {
        control.checkpoint()?;
        let path = destination.authored_path();
        let recovery = super::recovery::PathMetadata::from_destination(&destination)?;
        let bytes = destination
            .read_existing()
            .map_err(|error| format!("{}: {error}", path.display()))?
            .ok_or_else(|| format!("{}: source map does not exist", path.display()))?;
        control.checkpoint()?;
        let text =
            String::from_utf8(bytes).map_err(|error| format!("{}: {error}", path.display()))?;
        let vmf = Vmf::parse(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        let document =
            Document::try_new(vmf).map_err(|error| format!("{}: {error}", path.display()))?;
        document
            .context()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let camera = Camera::from_vmf(document.vmf())?;
        if let Some(camera) = camera {
            camera.validate(1.0)?;
        }
        let prepared = Self {
            document,
            camera,
            recovery,
            destination,
        };
        prepared.verify()?;
        control.checkpoint()?;
        Ok(prepared)
    }

    fn verify(&self) -> Result<(), String> {
        self.destination.verify().map_err(|error| error.to_string())
    }
}

impl Request {
    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn run(self) -> Completed {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.prepare()))
            .unwrap_or_else(|_| Err("Recent-map worker panicked".into()));
        Completed {
            request: self,
            result,
        }
    }

    fn prepare(&self) -> Result<Prepared, String> {
        let map = PreparedMap::load(self.destination.clone(), &self.control)?;
        let content = match &self.operation {
            Operation::Open { .. } => self
                .entry
                .content
                .as_ref()
                .map(|config| Candidate::load(config, &self.control))
                .transpose()?,
            Operation::Relocate => None,
        };
        map.verify()?;
        self.control.checkpoint()?;
        Ok(Prepared { map, content })
    }
}

pub(super) enum Binding {
    Independent(Authority),
    Application(Authority),
}

impl Default for Binding {
    fn default() -> Self {
        Self::Independent(Authority::default())
    }
}

impl Binding {
    fn authority(&self) -> &Authority {
        match self {
            Self::Independent(authority) | Self::Application(authority) => authority,
        }
    }
}

pub(super) fn entry_destination(entry: &Entry) -> Result<BoundDestination, String> {
    let destination = BoundDestination::capture(entry.reopen_path(), Policy::FollowSource)
        .map_err(|error| format!("{}: {error}", entry.reopen_path().display()))?;
    if destination.physical_path() != entry.path {
        return Err("Recent map alias changed its destination; choose Locate map to repair this record explicitly".into());
    }
    let metadata = std::fs::metadata(destination.physical_path())
        .map_err(|error| format!("{}: {error}", entry.reopen_path().display()))?;
    if !metadata.is_file() {
        return Err("Recent map must resolve to an existing regular file".into());
    }
    Ok(destination)
}

impl Session {
    pub fn with_recent_maps(
        game: Result<source_formats::gamefs::GameFs, String>,
        authority: Authority,
    ) -> Self {
        let mut session = Self::new(game);
        session.use_application_recent_maps(authority);
        session
    }

    pub(crate) fn use_application_recent_maps(&mut self, authority: Authority) {
        self.recent_maps = Binding::Application(authority);
        if let Some(error) = self.recent_maps_snapshot().error {
            self.log(Actor::System, error);
        }
    }

    pub fn configure_recent_maps(&mut self, directory: Result<PathBuf, String>) {
        if let Binding::Independent(authority) = &self.recent_maps
            && let Err(error) = authority.configure(directory)
        {
            self.log(Actor::System, error);
        }
    }

    pub fn recent_maps_snapshot(&self) -> Snapshot {
        self.recent_maps.authority().snapshot()
    }

    pub fn recent_maps_serial(&self) -> u64 {
        self.recent_maps.authority().serial()
    }

    pub fn reload_recent_maps(&mut self) -> Result<Value, String> {
        let result = self.recent_maps.authority().reload();
        self.log_recent_map_result(result)
            .map(|snapshot| self.recent_maps_with_snapshot(&snapshot))
    }

    fn recent_game_directory(&self) -> Option<PathBuf> {
        if self.game_error.is_some() {
            return None;
        }
        let path = if self.game.game_dir.is_absolute() {
            self.game.game_dir.clone()
        } else {
            std::env::current_dir().ok()?.join(&self.game.game_dir)
        };
        Some(std::fs::canonicalize(&path).unwrap_or(path))
    }

    pub fn recent_maps(&self) -> Value {
        self.recent_maps_with_snapshot(&self.recent_maps_snapshot())
    }

    pub fn recent_maps_with_snapshot(&self, snapshot: &Snapshot) -> Value {
        let game_directory = self.recent_game_directory();
        let current = self.current_content_profile_config();
        let pending = self.recent_operation.pending.as_ref();
        json!({
            "serial": snapshot.serial,
            "persistence": snapshot.persistence(),
            "directory": snapshot.directory,
            "error": snapshot.error,
            "reload_available": snapshot.directory.is_some(),
            "max_maps": recent_maps::MAX_RECENT_MAPS,
            "max_path_bytes": recent_maps::MAX_PATH_BYTES,
            "max_file_bytes": recent_maps::MAX_FILE_BYTES,
            "worker_active": self.recent_map_worker_busy(),
            "worker_token": pending.map(|_|self.recent_operation.serial),
            "worker_status": pending.map_or("idle", |pending|if pending.control.cancelled(){"cancelling"}else{"validating"}),
            "worker_name": pending.map(|pending|pending.name),
            "validation_pending": pending.is_some(),
            "validation_error": self.recent_operation.error,
            "entries": snapshot.entries.iter().map(|entry| {
                let availability = entry_destination(entry);
                let compatible = entry.content.as_ref().map_or(entry.game_directory == game_directory, |content|Some(content)==current.as_ref());
                json!({
                "path": entry.path,
                "title": entry.path.file_name().map(|name| name.to_string_lossy()),
                "reopen_path": entry.reopen_path(),
                "available": availability.is_ok(),
                "availability_error": availability.err(),
                "game_directory": entry.game_directory,
                "context_compatible": compatible,
                "content_matches_current": entry.content.as_ref().is_some_and(|content|Some(content)==current.as_ref()),
                "context_mode": if entry.content.is_some(){"full_content"}else{"legacy_build"},
                "content": entry.content,
                "relocation_available": !snapshot.blocked && !self.closing && !self.recent_map_worker_busy(),
                "build_settings": entry.build_settings.report(),
            })}).collect::<Vec<_>>(),
        })
    }

    fn log_recent_map_result<T>(&mut self, result: Result<T, String>) -> Result<T, String> {
        if let Err(error) = &result {
            self.log(Actor::System, error);
        }
        result
    }

    fn recent_map_error(&mut self, error: String) {
        let error = self.recent_maps.authority().report_error(error);
        self.log(Actor::System, error);
    }

    pub(crate) fn remember_recent_map(&mut self, path: &Path) {
        self.remember_map(path, true, true);
    }

    pub(super) fn remember_saved_map(&mut self, path: &Path) {
        self.remember_map(path, true, false);
    }

    fn remember_map(&mut self, path: &Path, reorder: bool, preserve_incompatible: bool) {
        if self.recent_maps.authority().blocked() {
            return;
        }
        let (path, reopen_path) = match recent_maps::source_paths(path) {
            Ok(paths) => paths,
            Err(error) => {
                self.recent_map_error(error);
                return;
            }
        };
        let entry = Entry {
            path,
            reopen_path,
            game_directory: self.recent_game_directory(),
            build_settings: BuildSettings::from_options(self.compile_options()),
            content: self.current_content_profile_config(),
        };
        let result = self.recent_maps.authority().remember(
            entry,
            reorder,
            preserve_incompatible,
            self.compiling(),
        );
        let _ = self.log_recent_map_result(result);
    }

    pub(super) fn remember_current_map_context(&mut self) {
        if let Some(path) = self.path.clone() {
            self.remember_map(&path, false, false);
        }
    }

    pub(super) fn restore_recent_map_context(&mut self, path: &Path, remember: bool) {
        if self.compiling() || self.closing {
            return;
        }
        let Ok(path) = recent_maps::absolute_path(path) else {
            return;
        };
        let Some(entry) = self.recent_maps.authority().entry(&path) else {
            return;
        };
        if entry.content.is_some() {
            if entry.content != self.current_content_profile_config() {
                self.log(Actor::System, "This map has a remembered content configuration; use Recent maps to restore it");
            }
            return;
        }
        if entry.game_directory != self.recent_game_directory() {
            self.log(
                Actor::System,
                "Recent map content differs from the mounted game; kept current build settings",
            );
            return;
        }
        if let Err(error) =
            self.update_build_settings_inner(Actor::System, entry.build_settings.update(), remember)
        {
            let error = format!("Recent map build settings could not restore: {error}");
            if remember {
                self.recent_map_error(error);
            } else {
                self.log(Actor::System, error);
            }
        }
    }

    pub fn open_recent_map(
        &mut self,
        actor: Actor,
        path: &Path,
        discard_changes: bool,
    ) -> Result<String, String> {
        let review = self.buffer_replacement_review();
        self.open_recent_map_reviewed(actor, path, discard_changes, &review)
    }

    pub fn remove_recent_map(&mut self, path: &Path) -> Result<(), String> {
        let result = self.recent_maps.authority().remove(path);
        self.log_recent_map_result(result)
    }

    pub fn clear_recent_maps(&mut self) -> Result<(), String> {
        let result = self.recent_maps.authority().clear();
        self.log_recent_map_result(result)
    }
}

impl Session {
    fn ensure_recent_map_open_available(
        &self,
        discard_changes: bool,
        full_context: bool,
    ) -> Result<(), String> {
        self.ensure_discardable(discard_changes)?;
        if !full_context {
            if !discard_changes && self.document_close_review().requires_discard() {
                return Err("Apply or cancel pending drafts before opening a recent map, or explicitly discard the reviewed changes".into());
            }
            return Ok(());
        }
        if self.content_profile_worker_busy() {
            return Err("Wait for content profile validation before opening a recent map".into());
        }
        self.ensure_content_transition_available(discard_changes)
    }

    fn begin_recent_map_request(
        &mut self,
        entry: Entry,
        serial: u64,
        destination: BoundDestination,
        operation: Operation,
    ) -> Result<Request, String> {
        self.ensure_not_closing()?;
        let worker = self
            .recent_operation
            .worker
            .begin()
            .ok_or("Wait for the current recent-map worker")?;
        self.recent_operation.serial += 1;
        let token = self.recent_operation.serial;
        let identity = Arc::new(());
        let control = compiler::Control::default();
        let name = match &operation {
            Operation::Open { .. } => "Open recent map",
            Operation::Relocate => "Locate recent map",
        };
        self.recent_operation.pending = Some(Pending {
            identity: identity.clone(),
            control: control.clone(),
            name,
        });
        self.recent_operation.error = None;
        self.view_serial += 1;
        Ok(Request {
            token,
            identity,
            entry,
            serial,
            destination,
            operation,
            intent: self.content_profile_intent(),
            control,
            started: Instant::now(),
            _worker: worker,
        })
    }

    pub fn begin_open_recent_map(
        &mut self,
        path: &Path,
        discard_changes: bool,
        review: &BufferReplacementReview,
        expected_serial: Option<u64>,
    ) -> Result<Request, String> {
        self.ensure_buffer_replacement_review(review)?;
        let (entry, snapshot) = self.recent_maps.authority().find(path, expected_serial)?;
        self.ensure_recent_map_open_available(discard_changes, entry.content.is_some())?;
        let destination = entry_destination(&entry)?;
        self.begin_recent_map_request(
            entry,
            snapshot.serial,
            destination,
            Operation::Open {
                discard_changes,
                review: review.clone(),
            },
        )
    }

    pub fn begin_relocate_recent_map(
        &mut self,
        old_path: &Path,
        new_path: &Path,
        expected_serial: Option<u64>,
    ) -> Result<Request, String> {
        self.ensure_not_closing()?;
        let (entry, snapshot) = self
            .recent_maps
            .authority()
            .find(old_path, expected_serial)?;
        let destination = BoundDestination::capture(new_path, Policy::FollowSource)
            .map_err(|error| format!("{}: {error}", new_path.display()))?;
        self.begin_recent_map_request(entry, snapshot.serial, destination, Operation::Relocate)
    }

    pub fn finish_recent_map(
        &mut self,
        actor: Actor,
        completed: Completed,
    ) -> Result<Value, String> {
        let Completed { request, result } = completed;
        let pending = self
            .recent_operation
            .pending
            .as_ref()
            .ok_or("Recent-map validation was cancelled or replaced")?;
        if request.token != self.recent_operation.serial
            || !Arc::ptr_eq(&request.identity, &pending.identity)
        {
            return Err("Recent-map completion belongs to a different or replaced request".into());
        }
        let result = self.finish_recent_map_inner(actor, &request, result);
        self.recent_operation.pending = None;
        self.recent_operation.error = result.as_ref().err().cloned();
        self.view_serial += 1;
        drop(request);
        result.map(|mut report| {
            report["recent_maps"] = self.recent_maps();
            report
        })
    }

    fn finish_recent_map_inner(
        &mut self,
        actor: Actor,
        request: &Request,
        result: Result<Prepared, String>,
    ) -> Result<Value, String> {
        request.control.checkpoint()?;
        self.ensure_not_closing()?;
        let prepared = result?;
        prepared.map.verify()?;
        let authority = self.recent_maps.authority().clone();
        match &request.operation {
            Operation::Relocate => {
                let (entry, _) = authority.relocate(
                    &request.entry.path,
                    &request.destination,
                    Some(request.serial),
                )?;
                self.log(
                    actor,
                    format!(
                        "Located recent map {} at {}",
                        request.entry.path.display(),
                        entry.reopen_path().display()
                    ),
                );
                Ok(json!({"relocated":{
                    "old_path":request.entry.path,"path":entry.path,"reopen_path":entry.reopen_path(),
                }}))
            }
            Operation::Open {
                discard_changes,
                review,
            } => {
                self.ensure_buffer_replacement_review(review)?;
                if request.intent != self.content_profile_intent() {
                    return Err("Content configuration changed while preparing the recent map; review Open again".into());
                }
                self.ensure_recent_map_open_available(
                    *discard_changes,
                    request.entry.content.is_some(),
                )?;
                let camera =
                    self.replacement_camera(&prepared.map.document, prepared.map.camera)?;
                let claim = self.file_claims.source(request.destination.clone())?;
                let full_context = request.entry.content.is_some();
                let legacy_compatible =
                    !full_context && request.entry.game_directory == self.recent_game_directory();
                let legacy_restore = legacy_compatible && !self.compiling();
                let legacy_options = legacy_restore
                    .then(|| request.entry.build_settings.options())
                    .transpose()?;
                let path = request.destination.authored_path().to_owned();
                let message = format!(
                    "opened {} ({} brushes, {} entities) in {:.0?} · {}",
                    path.display(),
                    prepared.map.document.vmf().world.solids.len(),
                    prepared.map.document.vmf().entities.len(),
                    request.started.elapsed(),
                    if full_context {
                        "restored remembered content configuration"
                    } else if self.compiling() {
                        "active build; kept current build settings"
                    } else if legacy_compatible {
                        "restored compatible legacy build settings"
                    } else {
                        "legacy content differs; kept current build settings"
                    },
                );
                authority.use_entry(&request.entry.path, Some(request.serial), |entry, _| {
                    if entry != request.entry {
                        return Err(
                            "Recent map context changed while preparing Open; refresh Recent maps"
                                .into(),
                        );
                    }
                    prepared.map.verify()?;
                    if let Some(content) = &prepared.content {
                        content.verify()?;
                    }
                    self.adopt_document(prepared.map.document, Some(path.clone()), camera);
                    if let (Some(content), Some(config)) = (prepared.content, entry.content) {
                        self.adopt_prepared_content(content, config, None);
                    } else if let Some(options) = legacy_options
                        && options != self.compile_options()
                    {
                        self.adopt_profile_build_settings(options);
                    }
                    self.update_recovery_path(prepared.map.recovery);
                    self.file_claims.source = Some(claim);
                    Ok(())
                })?;
                self.remember_recent_map(&path);
                self.log(actor, &message);
                Ok(json!({
                    "message":message,"path":path,"context_restored":full_context,
                    "legacy_context":!full_context,"legacy_build_restored":legacy_restore,
                }))
            }
        }
    }

    pub fn cancel_recent_map(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let pending = self
            .recent_operation
            .pending
            .as_ref()
            .filter(|_| token == self.recent_operation.serial)
            .ok_or("Recent-map worker was finished or replaced")?;
        let _ = pending.control.cancel();
        self.view_serial += 1;
        self.log(actor, "Cancelling recent-map validation");
        Ok(self.recent_maps())
    }

    pub fn fail_recent_map(&mut self, token: u64, error: String) {
        if token == self.recent_operation.serial {
            self.shutdown_recent_map();
            self.recent_operation.pending = None;
            self.recent_operation.error = Some(error);
            self.view_serial += 1;
        }
    }

    pub(super) fn shutdown_recent_map(&mut self) {
        if let Some(pending) = &self.recent_operation.pending {
            let _ = pending.control.cancel();
        }
    }

    pub(crate) fn recent_map_worker_busy(&self) -> bool {
        self.recent_operation.worker.busy()
    }

    pub(super) fn recent_map_close_work(&self) -> Value {
        json!({
            "sequence":self.recent_operation.serial,"worker_active":self.recent_map_worker_busy(),
            "name":self.recent_operation.pending.as_ref().map(|pending|pending.name),
            "cancelled":self.recent_operation.pending.as_ref().map(|pending|pending.control.cancelled()),
        })
    }
}
