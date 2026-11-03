use std::sync::Arc;

use serde_json::{Value, json};
use source_formats::{
    fgd::Catalog,
    gamefs::{GameFs, Mount},
};

use crate::bounded_file::Source;
use crate::content_profiles::Config;
use crate::content_watch::{NativeWatch, Scope, Snapshot};

use super::content_profiles::{Candidate, MAX_SOURCE_BYTES};
use super::worker::{Slot, Ticket};
use super::{Actor, Session};

const MAX_ERROR_BYTES: usize = 512;

#[derive(Default)]
pub(super) struct State {
    worker: Slot,
    pending: Option<Pending>,
    serial: u64,
    outcome: Option<&'static str>,
    error: Option<String>,
    watch: Watch,
}

#[derive(Default)]
struct Watch {
    enabled: bool,
    epoch: u64,
    native: Option<NativeWatch>,
    pending: bool,
    error: Option<String>,
}

struct Pending {
    identity: Arc<()>,
    control: compiler::Control,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct WatchStamp {
    epoch: u64,
    revision: Option<u64>,
}

pub struct Request {
    token: u64,
    identity: Arc<()>,
    config: Config,
    intent: u64,
    watch: WatchStamp,
    watch_enabled: bool,
    control: compiler::Control,
    worker: Option<Ticket>,
}

pub struct Completed {
    request: Request,
    result: Result<Prepared, String>,
}

struct Prepared {
    candidate: Candidate,
    watch: Option<NativeWatch>,
}

pub(super) struct WatchPublication {
    stamp: WatchStamp,
    native: Option<NativeWatch>,
}

impl Request {
    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn run(self) -> Completed {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let candidate = Candidate::load(&self.config, &self.control)?;
            let watch = if self.watch_enabled {
                Some(NativeWatch::start(candidate.watch_scope(&self.config)?)?)
            } else {
                None
            };
            candidate.verify()?;
            self.control.checkpoint()?;
            Ok(Prepared { candidate, watch })
        }))
        .unwrap_or_else(|_| Err("Content reload worker panicked".into()));
        Completed {
            request: self,
            result,
        }
    }
}

pub(super) fn watch_scope(
    game: &GameFs,
    catalog: &Catalog,
    config: Option<&Config>,
) -> Result<Scope, String> {
    let mut scope = Scope {
        files: catalog.sources.clone(),
        ..Default::default()
    };
    let gameinfo = if let Some(config) = config {
        Some(config.gameinfo.clone())
    } else if !game.game_dir.as_os_str().is_empty() {
        Some(game.game_dir.join("gameinfo.txt"))
    } else {
        None
    };
    if let Some(gameinfo) = gameinfo {
        let bytes = Source::capture(&gameinfo, MAX_SOURCE_BYTES)
            .and_then(Source::read)
            .map_err(|error| format!("{}: {error}", gameinfo.display()))?;
        let text =
            String::from_utf8(bytes).map_err(|error| format!("{}: {error}", gameinfo.display()))?;
        let dependencies = game
            .search_path_dependencies(&gameinfo, &text)
            .map_err(|error| error.to_string())?;
        scope.discoveries = dependencies.wildcard_directories;
        scope.literal_sources = dependencies.literal_mounts;
        scope.files.push(gameinfo);
    }
    for mounted in &game.mounts {
        match &mounted.mount {
            Mount::Directory(path) => scope.directories.push(path.clone()),
            Mount::Pak(_) => scope.archives.push(mounted.mount.location().to_owned()),
        }
    }
    Ok(scope)
}

fn bounded_error(error: &str) -> String {
    map_core::entity_helpers::bounded_preview(error, MAX_ERROR_BYTES)
}

impl Session {
    fn content_watch_snapshot(&self) -> Option<Snapshot> {
        self.content_refresh
            .watch
            .native
            .as_ref()
            .map(NativeWatch::snapshot)
    }

    fn content_watch_stamp(&self) -> WatchStamp {
        WatchStamp {
            epoch: self.content_refresh.watch.epoch,
            revision: self
                .content_watch_snapshot()
                .map(|snapshot| snapshot.revision),
        }
    }

    fn ensure_content_refresh_available(&self) -> Result<(), String> {
        if self.content_profile_worker_busy() || self.recent_map_worker_busy() {
            return Err(
                "Wait for content-profile and recent-map workers before reloading content".into(),
            );
        }
        self.ensure_content_transition_available(false)?;
        if self.current_content_profile_config().is_none() {
            return Err(
                "Current content configuration is unavailable; select content paths first".into(),
            );
        }
        Ok(())
    }

    pub fn content_refresh_report(&self) -> Value {
        let state = &self.content_refresh;
        let coverage = self.content_watch_snapshot();
        let watch_pending = state.watch.pending
            || coverage
                .as_ref()
                .is_some_and(|snapshot| snapshot.revision != 0);
        let worker_active = state.worker.busy();
        let blocker = if worker_active {
            Some("Wait for the current content reload worker".into())
        } else {
            self.ensure_content_refresh_available().err()
        };
        let retained_old =
            state.pending.is_some() || matches!(state.outcome, Some("failed" | "cancelled"));
        let mut summary: String = if let Some(pending) = &state.pending {
            if pending.control.cancelled() {
                "Cancelling content reload; previous content retained until owned work drains"
                    .into()
            } else {
                "Preparing current content; previous content retained".into()
            }
        } else {
            match state.outcome {
                Some("reloaded") => "Current content reloaded; map, history, selection, cameras and build/launch settings retained".into(),
                Some("failed") => "Content reload failed; previous content retained".into(),
                Some("cancelled") => "Content reload cancelled; previous content retained".into(),
                _ => "Current content configuration is in use".into(),
            }
        };
        if let Some(error) = &state.error {
            summary += &format!(". {error}");
        }
        if !state.watch.enabled {
            summary += ". Native watching is off";
        } else if let Some(error) = &state.watch.error {
            summary += &format!(". Native watch coverage unavailable: {error}");
        } else if let Some(coverage) = &coverage {
            summary += &format!(
                ". Native watch: {} events, {} changed path samples; {} coverage",
                coverage.events,
                coverage.paths.len(),
                if coverage.complete {
                    "complete"
                } else {
                    "incomplete"
                },
            );
            if coverage.overflow {
                summary += "; event accounting overflowed";
            }
            for error in &coverage.errors {
                summary += &format!("; {error}");
            }
        }
        if watch_pending {
            summary += ". Content changes are pending; Reload content explicitly to adopt them";
        }
        let config = self.current_content_profile_config();
        let mut report = json!({
            "status": summary,
            "summary": summary,
            "outcome": state.outcome,
            "intent": self.content_profile_intent(),
            "can_reload": blocker.is_none(),
            "blocker": blocker,
            "error": state.error,
            "retained_old": retained_old,
            "worker_active": worker_active,
            "pending": state.pending.as_ref().map(|pending| json!({
                "token": state.serial.to_string(),
                "cancel_requested": pending.control.cancelled(),
            })),
            "current_config": config,
            "definitions_count": self.definitions.classes().count(),
            "definition_sources": self.definitions.sources,
            "content_epoch": self.content_epoch,
            "content_error": self.game_error,
            "definitions_error": self.definitions_error,
            "watch": {
                "enabled": state.watch.enabled,
                "epoch": state.watch.epoch,
                "pending_changes": watch_pending,
                "error": state.watch.error,
                "coverage": coverage,
                "automatic_reload": false,
                "persistent": false,
            },
        });
        if let Some(config) = config {
            report["current_config"]["build"] = config.build.report();
        }
        report
    }

    pub fn begin_content_refresh(
        &mut self,
        expected_intent: Option<u64>,
    ) -> Result<Request, String> {
        self.ensure_content_refresh_available()?;
        let intent = self.content_profile_intent();
        if expected_intent.is_some_and(|expected| expected != intent) {
            return Err("Content configuration changed; inspect it before reloading".into());
        }
        let config = self
            .current_content_profile_config()
            .ok_or("Current content configuration is unavailable")?;
        let worker = self
            .content_refresh
            .worker
            .begin()
            .ok_or("Wait for the current content reload worker")?;
        self.content_refresh.serial += 1;
        let token = self.content_refresh.serial;
        let identity = Arc::new(());
        let control = compiler::Control::default();
        let watch = self.content_watch_stamp();
        self.content_refresh.pending = Some(Pending {
            identity: identity.clone(),
            control: control.clone(),
        });
        self.content_refresh.error = None;
        self.view_serial += 1;
        Ok(Request {
            token,
            identity,
            config,
            intent,
            watch,
            watch_enabled: self.content_refresh.watch.enabled,
            control,
            worker: Some(worker),
        })
    }

    pub fn finish_content_refresh(
        &mut self,
        actor: Actor,
        completed: Completed,
    ) -> Result<Value, String> {
        let Completed {
            mut request,
            result,
        } = completed;
        let pending = self
            .content_refresh
            .pending
            .as_ref()
            .ok_or("Content reload was cancelled or replaced")?;
        if request.token != self.content_refresh.serial
            || !Arc::ptr_eq(&request.identity, &pending.identity)
        {
            return Err("Content reload belongs to a different or replaced request".into());
        }
        drop(request.worker.take());
        let result = self.publish_content_refresh(&request, result);
        self.content_refresh.pending = None;
        self.content_refresh.error = result.as_ref().err().map(|error| bounded_error(error));
        self.content_refresh.outcome = Some(if request.control.cancelled() {
            "cancelled"
        } else if result.is_ok() {
            "reloaded"
        } else {
            "failed"
        });
        self.view_serial += 1;
        match result {
            Ok(()) => {
                self.log(actor, "Reloaded current content configuration");
                Ok(self.content_refresh_report())
            }
            Err(error) => {
                self.log(
                    actor,
                    format!(
                        "Content reload retained previous content: {}",
                        bounded_error(&error)
                    ),
                );
                Err(error)
            }
        }
    }

    fn publish_content_refresh(
        &mut self,
        request: &Request,
        result: Result<Prepared, String>,
    ) -> Result<(), String> {
        if request.control.cancelled() {
            return Err("Content reload cancelled; previous content retained".into());
        }
        self.ensure_content_refresh_available()?;
        if request.intent != self.content_profile_intent()
            || request.config
                != self
                    .current_content_profile_config()
                    .ok_or("Current content configuration is unavailable")?
        {
            return Err(
                "Content configuration changed during reload; previous content retained".into(),
            );
        }
        if request.watch.epoch != self.content_refresh.watch.epoch {
            return Err(
                "Watch configuration changed during reload; previous content retained".into(),
            );
        }
        let prepared = result?;
        prepared.candidate.verify()?;
        if let Some(watch) = &prepared.watch {
            let snapshot = watch.snapshot();
            if !snapshot.complete {
                return Err(format!(
                    "Prepared native watch coverage is incomplete; previous content retained: {}",
                    snapshot.errors.join("; ")
                ));
            }
        }
        request.control.checkpoint()?;
        self.adopt_refreshed_content(prepared.candidate);
        self.content_sources_changed(Some(WatchPublication {
            stamp: request.watch,
            native: prepared.watch,
        }));
        Ok(())
    }

    pub fn cancel_content_refresh(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        if token != self.content_refresh.serial {
            return Err("Content reload token is stale".into());
        }
        let pending = self
            .content_refresh
            .pending
            .as_ref()
            .ok_or("No content reload is pending")?;
        pending.control.cancel()?;
        self.view_serial += 1;
        self.log(
            actor,
            "Cancelled content reload; waiting for owned work to drain",
        );
        Ok(self.content_refresh_report())
    }

    pub fn fail_content_refresh(&mut self, token: u64, error: String) {
        if token == self.content_refresh.serial && self.content_refresh.pending.is_some() {
            if let Some(pending) = &self.content_refresh.pending {
                let _ = pending.control.cancel();
            }
            self.content_refresh.pending = None;
            self.content_refresh.outcome = Some("failed");
            self.content_refresh.error = Some(bounded_error(&error));
            self.view_serial += 1;
        }
    }

    pub fn set_content_watch(&mut self, actor: Actor, enabled: bool) -> Result<Value, String> {
        self.ensure_not_closing()?;
        self.content_refresh.watch.enabled = enabled;
        self.rebind_content_watch(None);
        self.view_serial += 1;
        if let Some(error) = &self.content_refresh.watch.error {
            return Err(error.clone());
        }
        self.log(
            actor,
            if enabled {
                "Enabled native content watching"
            } else {
                "Disabled native content watching"
            },
        );
        Ok(self.content_refresh_report())
    }

    pub(super) fn content_sources_changed(&mut self, publication: Option<WatchPublication>) {
        self.content_refresh.outcome = None;
        self.content_refresh.error = None;
        self.rebind_content_watch(publication);
    }

    fn rebind_content_watch(&mut self, publication: Option<WatchPublication>) {
        let before = self.content_watch_stamp();
        let mut pending = self.content_refresh.watch.pending
            || before.revision.is_some_and(|revision| revision != 0);
        let replacement = if let Some(publication) = publication {
            pending = before != publication.stamp;
            Ok(publication.native)
        } else if self.content_refresh.watch.enabled {
            watch_scope(
                &self.game,
                &self.definitions,
                self.current_content_profile_config().as_ref(),
            )
            .and_then(NativeWatch::start)
            .map(Some)
        } else {
            Ok(None)
        };
        pending |= before != self.content_watch_stamp();
        self.content_refresh.watch.epoch += 1;
        match replacement {
            Ok(native) => {
                self.content_refresh.watch.native = native;
                self.content_refresh.watch.error = None;
            }
            Err(error) => {
                self.content_refresh.watch.native = None;
                self.content_refresh.watch.error = Some(bounded_error(&error));
                pending = true;
                self.log(
                    Actor::System,
                    format!(
                        "Native content watch coverage unavailable: {}",
                        bounded_error(&error)
                    ),
                );
            }
        }
        self.content_refresh.watch.pending = pending;
    }

    pub(crate) fn content_refresh_worker_busy(&self) -> bool {
        self.content_refresh.worker.busy()
    }

    pub(super) fn content_refresh_close_work(&self) -> Value {
        json!({
            "sequence": self.content_refresh.serial,
            "worker_active": self.content_refresh_worker_busy(),
            "pending": self.content_refresh.pending.as_ref().map(|pending| json!({
                "token": self.content_refresh.serial.to_string(),
                "cancel_requested": pending.control.cancelled(),
            })),
        })
    }

    pub(super) fn shutdown_content_refresh(&mut self) {
        if let Some(pending) = &self.content_refresh.pending {
            let _ = pending.control.cancel();
        }
        self.content_refresh.watch.native = None;
    }
}
