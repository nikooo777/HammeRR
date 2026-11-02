use std::cell::{Cell, RefCell};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use native_runtime::{CaptureEnvironment, ContentMount};
use serde_json::{Value, json};
use source_formats::fgd::Catalog;
use source_formats::gamefs::{GameFs, Mount};

use crate::bounded_file::Source;
use crate::build_settings::BuildSettings;
use crate::content_profiles::{Authority, Config, Edit, Profile, Snapshot};
use crate::game;

use super::worker::{Slot, Ticket};
use super::{Actor, Session};

const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_NATIVE_LIBRARY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_MOUNTS: usize = 256;

#[derive(Default)]
pub(super) struct State {
    authority: Authority,
    pub(super) current: Option<Config>,
    active: Option<Profile>,
    observed: RefCell<Option<Intent>>,
    intent: Cell<u64>,
    worker: Slot,
    pending: Option<Pending>,
    serial: u64,
    error: Option<String>,
}

struct Pending {
    identity: Arc<()>,
    control: compiler::Control,
}

enum Operation {
    Save {
        name: String,
        replace: Option<String>,
    },
    Apply {
        id: String,
        name: String,
    },
}

pub struct Request {
    token: u64,
    identity: Arc<()>,
    config: Config,
    operation: Operation,
    revision: u64,
    intent: Option<u64>,
    control: compiler::Control,
    _worker: Ticket,
}

pub struct Completed {
    request: Request,
    result: Result<Candidate, String>,
}

impl Request {
    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn run(self) -> Completed {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Candidate::load(&self.config, &self.control)
        }))
        .unwrap_or_else(|_| Err("Content profile validation worker panicked".into()));
        Completed {
            request: self,
            result,
        }
    }
}

#[derive(PartialEq, Eq)]
struct Intent {
    config: Option<Config>,
    definitions: u64,
    definitions_status: u64,
    content: u64,
    document: u64,
}

struct Directory {
    requested: PathBuf,
    canonical: PathBuf,
    identity: source_formats::file_publication::FileIdentity,
    modified: Option<SystemTime>,
}

#[cfg(not(windows))]
fn open_directory(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

#[cfg(windows)]
fn open_directory(path: &Path) -> std::io::Result<File> {
    source_formats::file_publication::windows::open_directory(path)
}

impl Directory {
    fn capture(path: &Path) -> Result<Self, String> {
        let canonical = path.canonicalize().map_err(|error| error.to_string())?;
        let file = open_directory(&canonical).map_err(|error| error.to_string())?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_dir() {
            return Err(format!("{} must be a directory", path.display()));
        }
        Ok(Self {
            requested: path.to_owned(),
            canonical,
            identity: source_formats::file_publication::FileIdentity::from_file(&file)
                .map_err(|error| error.to_string())?,
            modified: metadata.modified().ok(),
        })
    }

    fn verify(&self) -> Result<(), String> {
        let current = Self::capture(&self.requested)?;
        if self.canonical != current.canonical
            || self.identity != current.identity
            || self.modified != current.modified
        {
            return Err(format!(
                "Content directory changed during profile validation: {}",
                self.requested.display()
            ));
        }
        Ok(())
    }
}

pub(super) struct Candidate {
    game: Arc<GameFs>,
    catalog: Catalog,
    options: compiler::Options,
    install: game::Install,
    files: Vec<Source>,
    directories: Vec<Directory>,
}

fn capture_file(path: &Path, limit: u64) -> Result<Source, String> {
    Source::capture(path, limit).map_err(|error| format!("{}: {error}", path.display()))
}

fn mount_layout(game: &GameFs) -> Vec<(Vec<String>, PathBuf)> {
    game.mounts
        .iter()
        .map(|mount| (mount.ids.clone(), mount.mount.location().to_owned()))
        .collect()
}

impl Candidate {
    pub(super) fn load(config: &Config, control: &compiler::Control) -> Result<Self, String> {
        control.checkpoint()?;
        config.validate()?;
        let options = config.build.options()?;
        let mut files = vec![capture_file(&config.gameinfo, MAX_SOURCE_BYTES)?];
        let initial = GameFs::from_gameinfo(&config.gameinfo).map_err(|error| error.to_string())?;
        if initial.mounts.len() > MAX_MOUNTS || initial.mounts.is_empty() {
            return Err(format!(
                "Content profiles require 1 to {MAX_MOUNTS} mounted paths"
            ));
        }
        let layout = mount_layout(&initial);
        let mut directories = Vec::new();
        for mount in &initial.mounts {
            match &mount.mount {
                Mount::Directory(path) => directories.push(Directory::capture(path)?),
                Mount::Pak(vpk) => vpk.verify_index().map_err(|error| error.to_string())?,
            }
        }
        drop(initial);
        control.checkpoint()?;
        let game =
            Arc::new(GameFs::from_gameinfo(&config.gameinfo).map_err(|error| error.to_string())?);
        if mount_layout(&game) != layout {
            return Err("Content mounts changed during profile validation; try again".into());
        }
        let fgd = config
            .fgd
            .clone()
            .or_else(|| crate::steam::find_css_fgd(&game))
            .ok_or("No installed CS:S SDK FGD was found; choose an FGD path")?;
        files.push(capture_file(&fgd, MAX_SOURCE_BYTES)?);
        let sources = Catalog::load_controlled(&fgd, &mut || control.checkpoint())?.sources;
        for path in &sources {
            files.push(capture_file(path, MAX_SOURCE_BYTES)?);
        }
        let catalog = Catalog::load_controlled(&fgd, &mut || control.checkpoint())?;
        if catalog.sources != sources {
            return Err("FGD include sources changed during profile validation; try again".into());
        }
        let install = game::Install::from_game_with_root(&game, config.engine_root.as_deref())?;
        if let Some(root) = &config.engine_root {
            directories.push(Directory::capture(root)?);
            files.push(capture_file(
                &root.join("cstrike/gameinfo.txt"),
                MAX_SOURCE_BYTES,
            )?);
        }
        if let Some(root) = &config.steam_root {
            directories.push(Directory::capture(root)?);
            directories.push(Directory::capture(&root.join("userdata"))?);
            for relative in ["config/loginusers.vdf", "steamapps/libraryfolders.vdf"] {
                files.push(capture_file(&root.join(relative), MAX_SOURCE_BYTES)?);
            }
        }
        if options.generate_reflections || options.physics == compiler::PhysicsMode::Engine {
            let engine_root = config
                .engine_root
                .clone()
                .unwrap_or_else(|| game.base_dir.clone());
            files.push(capture_file(
                &engine_root.join("cstrike/gameinfo.txt"),
                MAX_SOURCE_BYTES,
            )?);
            for name in ["engine.so", "vtex_dll.so", "bsppack.so", "libsteam_api.so"] {
                files.push(capture_file(
                    &engine_root.join("bin/linux64").join(name),
                    MAX_NATIVE_LIBRARY_BYTES,
                )?);
            }
            let environment = CaptureEnvironment {
                engine_root,
                steam_root: config.steam_root.clone().unwrap_or_default(),
                mounts: game
                    .mounts
                    .iter()
                    .map(|mount| ContentMount {
                        ids: mount.ids.clone(),
                        path: mount.mount.location().to_owned(),
                    })
                    .collect(),
            };
            native_runtime::validate_environment(&environment)
                .map_err(|error| error.to_string())?;
        }
        let candidate = Self {
            game,
            catalog,
            options,
            install,
            files,
            directories,
        };
        candidate.verify()?;
        control.checkpoint()?;
        Ok(candidate)
    }

    pub(super) fn verify(&self) -> Result<(), String> {
        for mount in &self.game.mounts {
            if let Mount::Pak(vpk) = &mount.mount {
                vpk.verify_index().map_err(|error| error.to_string())?;
            }
        }
        for file in &self.files {
            file.verify().map_err(|error| error.to_string())?;
        }
        for directory in &self.directories {
            directory.verify()?;
        }
        self.install.verify()
    }
}

impl Session {
    pub(crate) fn use_application_content_profiles(&mut self, authority: Authority) {
        self.content_profiles.authority = authority;
    }

    pub(crate) fn initialize_content_profile_config(&mut self, config: Option<Config>) {
        self.content_profiles.current = config;
        self.content_profiles.active = None;
    }

    pub(super) fn current_content_profile_config(&self) -> Option<Config> {
        let mut config = self.content_profiles.current.clone()?;
        config.build = BuildSettings::from_options(self.compile_options());
        config.engine_root = self.native_capture.engine_root().map(Path::to_owned);
        config.steam_root = self.native_capture.steam_root().map(Path::to_owned);
        Some(config)
    }

    pub(super) fn content_profile_intent(&self) -> u64 {
        let intent = Intent {
            config: self.current_content_profile_config(),
            definitions: self.definitions_serial,
            definitions_status: self.definitions_status_serial(),
            content: self.content_epoch,
            document: self.document_generation,
        };
        if self.content_profiles.observed.borrow().as_ref() != Some(&intent) {
            self.content_profiles.intent.set(
                self.content_profiles
                    .intent
                    .get()
                    .checked_add(1)
                    .expect("Content profile intent revisions exhausted"),
            );
            *self.content_profiles.observed.borrow_mut() = Some(intent);
        }
        self.content_profiles.intent.get()
    }

    fn ensure_content_profile_apply_available(&self) -> Result<(), String> {
        if self.recent_map_worker_busy() {
            return Err("Wait for the recent-map worker before applying a content profile".into());
        }
        self.ensure_content_transition_available(false)
    }

    pub(super) fn ensure_content_transition_available(
        &self,
        discard_drafts: bool,
    ) -> Result<(), String> {
        self.ensure_not_closing()?;
        self.native_cleanup_result()?;
        if self.compiling() || self.launching() || self.game_running() {
            return Err(
                "Wait for builds, launches and the managed game before applying a content profile"
                    .into(),
            );
        }
        if self.content_workers_busy()
            || self.definitions_load_busy()
            || self.recovery.close_work()["pending"].as_bool() == Some(true)
        {
            return Err("Wait for active content, definitions or recovery workers before applying a content profile".into());
        }
        let native = self.native_activity_status();
        if native["available"].as_bool() == Some(false) {
            return Err(native["message"]
                .as_str()
                .unwrap_or("Native activity is busy")
                .into());
        }
        if !self.sounds.close_playback()["playback"].is_null() {
            return Err("Stop the sound preview before applying a content profile".into());
        }
        if let Some(kind) = self.document_close_review().first_draft_except("")
            && !discard_drafts
        {
            return Err(format!(
                "Apply or cancel the pending {kind} draft before applying a content profile"
            ));
        }
        Ok(())
    }

    fn content_profiles_with_snapshot(&self, snapshot: &Snapshot) -> Value {
        let current = self.current_content_profile_config();
        let active = self.content_profiles.active.as_ref();
        let available = !snapshot.blocked
            && active
                .is_some_and(|active| snapshot.profiles.iter().any(|profile| profile == active));
        let blocker = if self.content_profile_worker_busy() {
            Some("Wait for the current content profile validation worker".into())
        } else {
            self.ensure_content_profile_apply_available().err()
        };
        let mut report = json!({
            "revision": snapshot.revision,
            "profiles": snapshot.profiles.iter().map(Profile::json).collect::<Vec<_>>(),
            "current_config": current,
            "active_profile": active.map(Profile::json),
            "active_matches_current": active.is_some_and(|active| Some(&active.config) == current.as_ref()),
            "active_profile_available": available,
            "intent": self.content_profile_intent(),
            "persistence": snapshot.persistence(),
            "path": snapshot.path,
            "blocked": snapshot.blocked,
            "error": snapshot.error,
            "can_apply": !snapshot.blocked && blocker.is_none(),
            "apply_blocker": snapshot.error.as_ref().filter(|_|snapshot.blocked).or(blocker.as_ref()),
            "worker_active": self.content_profile_worker_busy(),
            "validation_pending": self.content_profiles.pending.is_some(),
            "validation_error": self.content_profiles.error,
            "content_error": self.game_error,
            "definitions_error": self.definitions_error,
            "definition_sources": self.definitions.sources,
        });
        if let Some(config) = current {
            report["current_config"]["build"] = config.build.report();
        }
        report
    }

    pub fn content_profiles_snapshot(&self) -> Value {
        self.content_profiles_with_snapshot(&self.content_profiles.authority.snapshot())
    }

    pub fn save_content_profile(
        &mut self,
        actor: Actor,
        name: &str,
        config: Option<Config>,
        replace: Option<&str>,
        expected_revision: Option<u64>,
    ) -> Result<Value, String> {
        let request = self.begin_content_profile_save(name, config, replace, expected_revision)?;
        self.finish_content_profile(actor, request.run())
    }

    pub fn apply_content_profile(
        &mut self,
        actor: Actor,
        id: &str,
        expected_revision: Option<u64>,
        expected_intent: Option<u64>,
    ) -> Result<Value, String> {
        let request = self.begin_content_profile_apply(id, expected_revision, expected_intent)?;
        self.finish_content_profile(actor, request.run())
    }

    fn begin_content_profile_request(
        &mut self,
        config: Config,
        operation: Operation,
        revision: u64,
        intent: Option<u64>,
    ) -> Result<Request, String> {
        self.ensure_not_closing()?;
        config.validate()?;
        let worker = self
            .content_profiles
            .worker
            .begin()
            .ok_or("Wait for the current content profile validation worker")?;
        self.content_profiles.serial += 1;
        let token = self.content_profiles.serial;
        let identity = Arc::new(());
        let control = compiler::Control::default();
        self.content_profiles.pending = Some(Pending {
            identity: identity.clone(),
            control: control.clone(),
        });
        self.content_profiles.error = None;
        self.view_serial += 1;
        Ok(Request {
            token,
            identity,
            config,
            operation,
            revision,
            intent,
            control,
            _worker: worker,
        })
    }

    pub fn begin_content_profile_save(
        &mut self,
        name: &str,
        config: Option<Config>,
        replace: Option<&str>,
        expected_revision: Option<u64>,
    ) -> Result<Request, String> {
        self.ensure_not_closing()?;
        let snapshot = self.content_profiles.authority.snapshot();
        if expected_revision.is_some_and(|expected| expected != snapshot.revision) {
            return Err("Content profiles changed; refresh the list before saving".into());
        }
        if snapshot.blocked {
            return Err(snapshot
                .error
                .unwrap_or_else(|| "Content profile storage is unavailable".into()));
        }
        let intent = config.is_none().then(|| self.content_profile_intent());
        let config = config
            .or_else(|| self.current_content_profile_config())
            .ok_or("Current content configuration is unavailable; supply explicit profile paths")?;
        self.begin_content_profile_request(
            config,
            Operation::Save {
                name: name.into(),
                replace: replace.map(str::to_owned),
            },
            snapshot.revision,
            intent,
        )
    }

    pub fn begin_content_profile_apply(
        &mut self,
        id: &str,
        expected_revision: Option<u64>,
        expected_intent: Option<u64>,
    ) -> Result<Request, String> {
        if expected_intent.is_some_and(|expected| expected != self.content_profile_intent()) {
            return Err(
                "Document content configuration changed; refresh profiles before applying".into(),
            );
        }
        self.ensure_content_profile_apply_available()?;
        let (profile, snapshot) = self
            .content_profiles
            .authority
            .find(id, expected_revision)?;
        self.begin_content_profile_request(
            profile.config,
            Operation::Apply {
                id: profile.id,
                name: profile.name,
            },
            snapshot.revision,
            Some(self.content_profile_intent()),
        )
    }

    pub fn finish_content_profile(
        &mut self,
        actor: Actor,
        completed: Completed,
    ) -> Result<Value, String> {
        let Completed { request, result } = completed;
        let pending = self
            .content_profiles
            .pending
            .as_ref()
            .ok_or("Content profile validation was cancelled or replaced")?;
        if request.token != self.content_profiles.serial
            || !Arc::ptr_eq(&pending.identity, &request.identity)
        {
            return Err(
                "Content profile validation belongs to a different or replaced request".into(),
            );
        }
        let result = self.finish_content_profile_inner(actor, &request, result);
        self.content_profiles.pending = None;
        self.content_profiles.error = result.as_ref().err().cloned();
        self.view_serial += 1;
        drop(request);
        result.map(|(snapshot, profile, applied)| {
            let mut report = self.content_profiles_with_snapshot(&snapshot);
            report[if applied {
                "applied_profile"
            } else {
                "affected_profile"
            }] = profile.json();
            report
        })
    }

    fn finish_content_profile_inner(
        &mut self,
        actor: Actor,
        request: &Request,
        result: Result<Candidate, String>,
    ) -> Result<(Snapshot, Profile, bool), String> {
        request.control.checkpoint()?;
        self.ensure_not_closing()?;
        if request
            .intent
            .is_some_and(|intent| intent != self.content_profile_intent())
        {
            return Err(
                "Document content configuration changed during validation; refresh profiles".into(),
            );
        }
        let candidate = result?;
        candidate.verify()?;
        let authority = self.content_profiles.authority.clone();
        match &request.operation {
            Operation::Save { name, replace } => {
                let (profile, snapshot) = authority.edit(
                    Some(request.revision),
                    Edit::Save {
                        name,
                        replace: replace.as_deref(),
                        config: &request.config,
                    },
                )?;
                self.log(actor, format!("Saved content profile {}", profile.name));
                Ok((snapshot, profile, false))
            }
            Operation::Apply { id, name } => {
                self.ensure_content_profile_apply_available()?;
                let (profile, snapshot) =
                    authority.use_profile(id, Some(request.revision), |profile, snapshot| {
                        if profile.name != *name || profile.config != request.config {
                            return Err(
                                "Content profile changed during validation; refresh profiles"
                                    .into(),
                            );
                        }
                        candidate.verify()?;
                        self.adopt_content_profile(candidate, profile.clone());
                        Ok((profile, snapshot))
                    })?;
                self.log(actor, format!("Applied content profile {}", profile.name));
                self.remember_current_map_context();
                Ok((snapshot, profile, true))
            }
        }
    }

    pub fn fail_content_profile(&mut self, token: u64, error: String) {
        if token == self.content_profiles.serial {
            self.shutdown_content_profile();
            self.content_profiles.pending = None;
            self.content_profiles.error = Some(error);
            self.view_serial += 1;
        }
    }

    pub(crate) fn content_profile_worker_busy(&self) -> bool {
        self.content_profiles.worker.busy()
    }

    pub(super) fn content_profile_close_work(&self) -> Value {
        json!({
            "sequence": self.content_profiles.serial,
            "worker_active": self.content_profile_worker_busy(),
            "pending": self.content_profiles.pending.is_some(),
        })
    }

    pub(super) fn shutdown_content_profile(&mut self) {
        if let Some(pending) = &self.content_profiles.pending {
            let _ = pending.control.cancel();
        }
    }

    fn adopt_content_profile(&mut self, candidate: Candidate, profile: Profile) {
        self.adopt_prepared_content(candidate, profile.config.clone(), Some(profile));
    }

    pub(super) fn adopt_prepared_content(
        &mut self,
        candidate: Candidate,
        config: Config,
        active: Option<Profile>,
    ) {
        self.game = candidate.game;
        self.game_error = None;
        self.definitions = candidate.catalog;
        self.definitions_error = None;
        self.definitions_serial += 1;
        self.native_capture
            .adopt(config.engine_root.clone(), config.steam_root.clone());
        self.adopt_profile_build_settings(candidate.options);
        self.reset_mounted_content_assets();
        self.content_profiles.current = Some(config);
        self.content_profiles.active = active;
    }

    pub fn remove_content_profile(
        &mut self,
        actor: Actor,
        id: &str,
        expected_revision: Option<u64>,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        let (profile, snapshot) = self
            .content_profiles
            .authority
            .edit(expected_revision, Edit::Delete { id })?;
        self.log(actor, format!("Removed content profile {}", profile.name));
        let mut report = self.content_profiles_with_snapshot(&snapshot);
        report["affected_profile"] = profile.json();
        Ok(report)
    }

    pub fn reload_content_profiles(
        &mut self,
        actor: Actor,
        expected_revision: Option<u64>,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        let snapshot = self.content_profiles.authority.reload(expected_revision)?;
        self.log(actor, "Reloaded content profiles");
        Ok(self.content_profiles_with_snapshot(&snapshot))
    }
}
