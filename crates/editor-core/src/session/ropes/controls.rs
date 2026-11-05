use std::sync::Arc;

use map_core::Document;
use render::ropes::{self, Preview};
use serde_json::Value;

use super::state::{Key, MAX_ADVANCE_SECONDS, MAX_SOURCE_BYTES, Machine, Pending, cancel_control};
use super::{Clock, Command, Completed, Frame, Request, Settings, playback};
use crate::session::{Actor, Session};

pub(crate) enum Update {
    State(Value),
    Work(Box<Request>),
}

impl Session {
    fn rope_frame_command(&mut self, actor: Actor, command: Command) -> Result<Update, String> {
        let (time, fresh, replay, pause, status) = match command {
            Command::Prepare => (self.rope_preview.requested, true, false, true, "preparing"),
            Command::Seek { time_seconds } => {
                (checked_time(time_seconds)?, false, true, true, "seeking")
            }
            Command::Reset => (0.0, false, true, true, "resetting"),
            Command::Advance { seconds } => {
                if !seconds.is_finite() || !(0.0..=MAX_ADVANCE_SECONDS).contains(&seconds) {
                    return Err(format!(
                        "Advance seconds must be finite between 0 and {MAX_ADVANCE_SECONDS}"
                    ));
                }
                let current = self
                    .rope_preview_frame()
                    .ok_or("Prepare the rope preview before advancing")?
                    .preview
                    .time;
                if self.rope_preview.machine.is_none() {
                    return Err("Prepare the rope preview before advancing".into());
                }
                if seconds == 0.0 {
                    return Ok(Update::State(self.rope_preview_state()));
                }
                (
                    (current + seconds).min(ropes::MAX_TIME_SECONDS),
                    false,
                    false,
                    false,
                    "advancing",
                )
            }
            _ => unreachable!(),
        };
        let request = self.begin_rope_request(actor, time, fresh, replay, false, status)?;
        if pause {
            self.rope_preview.playing = false;
        }
        self.rope_preview.control_serial += 1;
        self.rope_preview.last_actor = Some(actor);
        self.log(actor, format!("{status} rope preview at {time:.3} seconds"));
        Ok(Update::Work(Box::new(request)))
    }

    fn begin_rope_request(
        &mut self,
        actor: Actor,
        target: f64,
        fresh: bool,
        replay: bool,
        verify_only: bool,
        status: &'static str,
    ) -> Result<Request, String> {
        checked_time(target)?;
        if self.rope_preview.worker.busy() {
            return Err("Wait for the active rope preview worker or cancel it first".into());
        }
        if let Some(error) = &self.game_error {
            let error = format!("Rope content is unavailable: {}", bounded(error));
            self.unavailable_rope_preview("failed", Some(error.clone()));
            return Err(error);
        }
        let input = if verify_only {
            playback::Input::Verify(
                self.rope_preview_frame()
                    .ok_or("Rope pose is unavailable; prepare again")?
                    .sources
                    .clone(),
            )
        } else if !fresh && self.rope_preview.machine.is_some() {
            playback::Input::Continue {
                machine: Box::new(self.rope_preview.machine.take().unwrap()),
                replay,
            }
        } else {
            let document = self.render_document();
            let snapshot_bytes = match map_core::references::admit_snapshot_storage(
                document.vmf(),
                MAX_SOURCE_BYTES,
                &mut || Ok(()),
            ) {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.unavailable_rope_preview("failed", Some(error.clone()));
                    return Err(error);
                }
            };
            playback::Input::Source {
                document: Box::new(document.preview_snapshot_uncached()),
                choice: self
                    .model_choice_override(document)
                    .map(|(ids, asset)| (ids.to_vec(), asset.clone())),
                included: self.rope_source_visibility(document),
                snapshot_bytes,
            }
        };
        let worker = self
            .rope_preview
            .worker
            .begin()
            .ok_or("Rope preview worker is already active")?;
        self.rope_preview.request_serial += 1;
        let token = format!(
            "{}:{}",
            self.rope_preview.scope, self.rope_preview.request_serial
        );
        let identity = Arc::new(());
        let control = compiler::Control::default();
        let key = Key::capture(self);
        self.rope_preview.pending = Some(Pending {
            actor,
            token: token.clone(),
            identity: identity.clone(),
            control: control.clone(),
            key: key.clone(),
            preserve_cancelled_frame: false,
        });
        self.rope_preview.requested = target;
        self.rope_preview.status = status;
        self.rope_preview.error = None;
        self.rope_preview.issues.clear();
        self.rope_preview.omitted = 0;
        if !verify_only {
            self.retire_rope_bounds_reviews();
        }
        self.rope_preview_changed(false);
        Ok(Request {
            token,
            identity,
            control,
            key,
            game: self.game.clone(),
            settings: self.rope_preview.settings,
            target,
            input,
            verify_only,
            worker,
        })
    }

    pub(crate) fn begin_rope_tick(&mut self, seconds: f64) -> Result<Option<Request>, String> {
        self.ensure_not_closing()?;
        self.preflight_rope_preview();
        if !seconds.is_finite() || !(0.0..=MAX_ADVANCE_SECONDS).contains(&seconds) {
            return Err("Rope clock delta must be finite between 0 and 1 second".into());
        }
        if !self.view.rope_preview || !self.rope_preview.presentation_active {
            return Ok(None);
        }
        if self.rope_preview.playing {
            self.rope_preview.tick_remainder =
                (self.rope_preview.tick_remainder + seconds).min(0.1);
            if self.rope_preview.worker.busy()
                || self.rope_preview.tick_remainder < ropes::STEP_SECONDS
            {
                return Ok(None);
            }
            let delta = std::mem::take(&mut self.rope_preview.tick_remainder);
            let target = (self.rope_preview.requested + delta).min(ropes::MAX_TIME_SECONDS);
            return self
                .begin_rope_request(Actor::System, target, false, false, false, "advancing")
                .map(Some);
        }
        self.begin_rope_verification(false)
    }

    pub(crate) fn begin_rope_verification(
        &mut self,
        force: bool,
    ) -> Result<Option<Request>, String> {
        self.ensure_not_closing()?;
        self.preflight_rope_preview();
        if self.rope_preview.worker.busy() {
            if force {
                return Err("Wait for the current rope preview request before rendering or picking its frame".into());
            }
            return Ok(None);
        }
        let Some(frame) = self.rope_preview_frame() else {
            return Ok(None);
        };
        if !force && frame.verified.elapsed() < playback::VERIFY_INTERVAL {
            return Ok(None);
        }
        let time = self
            .rope_preview_frame()
            .ok_or("Rope pose is unavailable; prepare again")?
            .preview
            .time;
        self.begin_rope_request(Actor::System, time, false, false, true, "verifying")
            .map(Some)
    }

    pub(crate) fn finish_rope_preview(&mut self, completed: Completed) -> Result<(), String> {
        let matches = self.rope_preview.pending.as_ref().is_some_and(|pending| {
            pending.token == completed.token && Arc::ptr_eq(&pending.identity, &completed.identity)
        });
        if !matches {
            return Err("Rope preview result belongs to an expired request".into());
        }
        let preserve_cancelled_frame = self
            .rope_preview
            .pending
            .as_ref()
            .unwrap()
            .preserve_cancelled_frame;
        if !completed.key.current(self) {
            self.unavailable_rope_preview(
                "stale",
                Some("Rope source changed while preparing; inspect and prepare again".into()),
            );
        } else if completed.control.cancelled() {
            if !completed.verify_only {
                self.rope_preview.machine = None;
            }
            self.rope_preview.playing = false;
            self.rope_preview.status = "cancelled";
            self.rope_preview.error = None;
            if !preserve_cancelled_frame {
                self.rope_preview.frame = None;
            }
            self.rope_preview.requested = self
                .rope_preview_frame()
                .map_or(0.0, |frame| frame.preview.time);
            self.rope_preview_changed(!preserve_cancelled_frame);
        } else {
            match completed.result {
                Ok(playback::Output::Ready(ready)) => self.publish_rope_preview(*ready)?,
                Ok(playback::Output::Verified(verified)) => {
                    if let Some(frame) = &mut self.rope_preview.frame {
                        frame.verified = verified;
                    }
                    if let Some(machine) = &mut self.rope_preview.machine {
                        machine.verified = verified;
                    }
                    self.rope_preview.status = "ready";
                    self.rope_preview_changed(false);
                }
                Err(error) => {
                    self.rope_preview.issues = completed.diagnostics.issues;
                    self.rope_preview.omitted = completed.diagnostics.omitted;
                    self.unavailable_rope_preview("failed", Some(error));
                }
            }
        }
        self.rope_preview.pending = None;
        drop(completed.worker);
        Ok(())
    }

    fn publish_rope_preview(&mut self, ready: playback::Ready) -> Result<(), String> {
        let checked = {
            let library = self.models.borrow();
            ready.model_sources.iter().try_for_each(|asset| {
                library
                    .check_cached_source(asset)
                    .map_err(|error| format!("Rope parent model {}: {error}", asset.path))
            })
        };
        if let Err(error) = checked {
            self.unavailable_rope_preview("failed", Some(error.clone()));
            return Err(error);
        }
        let (appearance, appearance_sources) = match ready.appearance {
            playback::Appearance::New(assets) => match assets.bind(&mut self.materials) {
                Ok((appearance, sources)) => (Arc::new(appearance), sources),
                Err(error) => {
                    self.unavailable_rope_preview("failed", Some(error.clone()));
                    return Err(error);
                }
            },
            playback::Appearance::Cached {
                appearance,
                sources,
            } => (appearance, sources),
        };
        let key = Key::capture(self);
        let collision = super::state::CollisionSummary::capture(ready.collision.as_ref());
        let parent_models = ready.model_sources.len();
        let prepared = ready.prepared.clone();
        let sources = ready.sources.unwrap_or_else(|| {
            Arc::new(super::state::Sources {
                control: compiler::Control::default(),
                appearance: appearance_sources.clone(),
                collision: ready
                    .collision
                    .as_ref()
                    .map(|collision| collision.sources.clone()),
                models: ready.model_sources.clone(),
            })
        });
        self.rope_preview.machine = Some(Machine {
            simulation: ready.simulation,
            prepared: ready.prepared,
            collision: ready.collision,
            appearance: appearance.clone(),
            appearance_sources,
            model_sources: ready.model_sources,
            snapshot_bytes: ready.snapshot_bytes,
            verified: ready.verified,
            sources: sources.clone(),
        });
        self.rope_preview.frame = Some(Frame {
            preview: ready.preview,
            prepared,
            appearance,
            collision,
            parent_models,
            simulation_work: ready.progress.work,
            collision_work: ready.collision_work,
            warmup_steps: ready.progress.warmup_steps,
            snapshot_bytes: ready.snapshot_bytes,
            key,
            verified: ready.verified,
            sources,
        });
        self.rope_preview.status = "ready";
        self.rope_preview.error = None;
        if self.rope_preview.requested >= ropes::MAX_TIME_SECONDS {
            self.rope_preview.playing = false;
        }
        self.rope_preview_changed(true);
        Ok(())
    }

    pub(crate) fn fail_rope_preview(&mut self, token: &str, error: String) -> Result<(), String> {
        if self
            .rope_preview
            .pending
            .as_ref()
            .is_none_or(|pending| pending.token != token)
        {
            return Err("Rope preview request is expired".into());
        }
        self.unavailable_rope_preview("failed", Some(error));
        Ok(())
    }

    pub fn fail_rope_preview_render(&mut self, error: String) {
        self.unavailable_rope_preview("failed", Some(format!("Rope rendering failed: {error}")));
    }

    pub(crate) fn rope_preview_render_guard(&self) -> Result<playback::RenderGuard, String> {
        self.ensure_not_closing()?;
        let frame = self
            .rope_preview_frame()
            .ok_or_else(|| self.rope_preview_summary())?;
        Ok(playback::RenderGuard {
            key: frame.key.clone(),
            sources: frame.sources.clone(),
            game: self.game.clone(),
        })
    }

    pub(crate) fn finish_rope_render_guard(
        &mut self,
        guard: playback::VerifiedRenderGuard,
    ) -> Result<(), String> {
        if !guard.key.current(self)
            || self
                .rope_preview_frame()
                .is_none_or(|frame| !Arc::ptr_eq(&frame.sources, &guard.sources))
        {
            return Err(
                "Rope sources or controls changed during verification; inspect again".into(),
            );
        }
        if let Err(error) = guard.result {
            self.unavailable_rope_preview("failed", Some(error.clone()));
            return Err(error);
        }
        Ok(())
    }
}

fn bounded(error: &str) -> String {
    map_core::entity_helpers::bounded_preview(error, 1024)
}

fn checked_time(time: f64) -> Result<f64, String> {
    if !time.is_finite() || !(0.0..=ropes::MAX_TIME_SECONDS).contains(&time) {
        return Err(format!(
            "Rope time must be finite between 0 and {} seconds",
            ropes::MAX_TIME_SECONDS
        ));
    }
    Ok(time)
}

impl Session {
    pub fn rope_preview_frame(&self) -> Option<&Frame> {
        self.rope_preview
            .frame
            .as_ref()
            .filter(|frame| frame.key.current(self))
    }

    pub(in crate::session) fn rope_preview_geometry_in(&self, document: &Document) -> Arc<Preview> {
        self.rope_preview_frame()
            .filter(|frame| frame.key.document_matches(document))
            .map_or_else(
                || self.rope_preview.empty.clone(),
                |frame| frame.preview.clone(),
            )
    }

    pub(in crate::session) fn rope_preview_appearance(
        &self,
    ) -> Option<Arc<render::ropes::materials::Prepared>> {
        self.rope_preview_frame()
            .map(|frame| frame.appearance.clone())
    }

    pub fn rope_preview_serial(&self) -> u64 {
        self.rope_preview.frame_serial
    }

    pub fn rope_preview_settings(&self) -> Settings {
        self.rope_preview.settings
    }

    pub fn rope_preview_clock(&self) -> Clock {
        let frame = self.rope_preview_frame();
        let stale = (self.rope_preview.frame.is_some() && frame.is_none())
            || self
                .rope_preview
                .pending
                .as_ref()
                .is_some_and(|pending| !pending.key.current(self));
        Clock {
            snapshot: Key::capture(self)
                .snapshot(&self.rope_preview.scope, self.rope_preview.control_serial),
            serial: self.rope_preview.serial,
            frame_serial: self.rope_preview.frame_serial,
            status: if stale {
                "stale"
            } else {
                self.rope_preview.status
            },
            playing: self.rope_preview.playing && !stale,
            time_seconds: frame.map_or(0.0, |frame| frame.preview.time),
            requested_time_seconds: self.rope_preview.requested,
            busy: self.rope_preview.worker.busy(),
            presentation_active: self.rope_preview.presentation_active,
        }
    }

    fn check_rope_snapshot(&self, expected: &str) -> Result<(), String> {
        if expected.len() != 64 || self.rope_preview_clock().snapshot != expected {
            return Err(
                "Rope preview controls or source changed; inspect the preview again".into(),
            );
        }
        Ok(())
    }

    fn retire_rope_bounds_reviews(&mut self) {
        let changed = self.area_preview.take().is_some() || self.volume_preview().is_some();
        self.volume.retire_presentation_preview();
        if changed {
            self.interaction_epoch += 1;
            self.rope_preview.frame_serial += 1;
            self.view_serial += 1;
        }
    }

    fn rope_preview_changed(&mut self, geometry: bool) {
        self.rope_preview.serial += 1;
        if geometry {
            self.retire_rope_bounds_reviews();
            self.rope_preview.frame_serial += 1;
        }
        self.view_serial += 1;
    }

    fn unavailable_rope_preview(&mut self, status: &'static str, error: Option<String>) {
        self.rope_preview.cancel();
        if let Some(frame) = &self.rope_preview.frame {
            cancel_control(&frame.sources.control);
        }
        self.rope_preview.pending = None;
        self.rope_preview.machine = None;
        self.rope_preview.frame = None;
        self.rope_preview.playing = false;
        self.rope_preview.status = status;
        self.rope_preview.error = error.map(|error| bounded(&error));
        self.rope_preview.control_serial += 1;
        self.rope_preview_changed(true);
    }

    pub fn preflight_rope_preview(&mut self) {
        let stale = self
            .rope_preview
            .frame
            .as_ref()
            .is_some_and(|frame| !frame.key.current(self))
            || self
                .rope_preview
                .pending
                .as_ref()
                .is_some_and(|pending| !pending.key.current(self));
        if stale {
            self.unavailable_rope_preview(
                "stale",
                Some("Map, geometry, visibility, definitions or content changed; prepare the rope preview again".into()),
            );
        } else if self.rope_preview.status == "cancelling"
            && !self.rope_preview.worker.busy()
            && self.rope_preview.pending.is_none()
        {
            self.rope_preview.status = "cancelled";
            self.rope_preview_changed(false);
        }
    }

    pub(in crate::session) fn rope_preview_view_changed(&mut self, enabled: bool) {
        if !enabled {
            self.rope_preview.playing = false;
            if self.rope_preview.pending.is_some() {
                self.rope_preview.status = "pausing";
            }
        }
        self.rope_preview.control_serial += 1;
        self.rope_preview_changed(true);
    }

    pub(in crate::session) fn reset_rope_preview(&mut self) {
        self.rope_preview.reset();
    }

    pub(in crate::session) fn shutdown_rope_preview(&mut self) {
        self.rope_preview.stop_clock();
        self.unavailable_rope_preview("closed", None);
    }

    pub(crate) fn begin_rope_clock(&mut self) -> Option<super::state::ClockLease> {
        if self.closing {
            None
        } else {
            self.rope_preview.clock_lease()
        }
    }

    pub(crate) fn rope_clock_current(&self, scope: &str) -> bool {
        !self.closing && self.rope_preview.scope == scope
    }

    pub(crate) fn rope_clock_due(&self) -> bool {
        if self.closing || !self.rope_preview.presentation_active || !self.view.rope_preview {
            return false;
        }
        if self.rope_preview.frame.is_some() && self.rope_preview_frame().is_none() {
            return true;
        }
        if self.rope_preview.worker.busy() {
            return false;
        }
        self.rope_preview.playing
            || self
                .rope_preview_frame()
                .is_some_and(|frame| frame.verified.elapsed() >= playback::VERIFY_INTERVAL)
    }

    pub fn set_rope_presentation_active(&mut self, active: bool) -> Result<(), String> {
        self.ensure_not_closing()?;
        self.rope_preview.presentation_active = active;
        self.rope_preview.tick_remainder = 0.0;
        Ok(())
    }

    pub(crate) fn fail_rope_clock(&mut self, scope: &str, error: String) {
        if self.rope_clock_current(scope) {
            self.unavailable_rope_preview("failed", Some(error));
        }
    }

    pub(crate) fn rope_preview_command(
        &mut self,
        actor: Actor,
        expected: &str,
        command: Command,
    ) -> Result<Update, String> {
        self.ensure_not_closing()?;
        self.check_rope_snapshot(expected)?;
        self.preflight_rope_preview();
        match command {
            Command::Pause | Command::Cancel => {
                let clear = matches!(command, Command::Cancel);
                self.rope_preview.playing = false;
                if clear || self.rope_preview.pending.is_none() {
                    self.rope_preview.requested = self
                        .rope_preview_frame()
                        .map_or(0.0, |frame| frame.preview.time);
                }
                if let Some(pending) = &mut self.rope_preview.pending {
                    if clear {
                        pending.preserve_cancelled_frame = false;
                        cancel_control(&pending.control);
                    }
                    self.rope_preview.status = if clear { "cancelling" } else { "pausing" };
                } else {
                    self.rope_preview.status = if clear {
                        "cancelled"
                    } else if self.rope_preview.frame.is_some() {
                        "ready"
                    } else {
                        "not_prepared"
                    };
                }
                if clear {
                    if let Some(frame) = &self.rope_preview.frame {
                        cancel_control(&frame.sources.control);
                    }
                    self.rope_preview.frame = None;
                    self.rope_preview.machine = None;
                    self.rope_preview.error = None;
                    self.rope_preview.issues.clear();
                    self.rope_preview.omitted = 0;
                }
                self.rope_preview.control_serial += 1;
                self.rope_preview.last_actor = Some(actor);
                self.rope_preview_changed(clear);
                self.log(
                    actor,
                    if clear {
                        "cancelled rope preview"
                    } else {
                        "paused rope preview"
                    },
                );
                Ok(Update::State(self.rope_preview_state()))
            }
            Command::Settings(settings) => {
                settings.environment()?;
                if self.rope_preview.worker.busy() {
                    return Err("Pause or cancel the rope preview and wait for its worker before changing settings".into());
                }
                if self.rope_preview.settings != settings {
                    self.rope_preview.settings = settings;
                    self.rope_preview.control_serial += 1;
                    self.rope_preview.last_actor = Some(actor);
                    self.unavailable_rope_preview("not_prepared", None);
                    self.rope_preview.issues.clear();
                    self.rope_preview.omitted = 0;
                    self.log(actor, "changed rope preview environment");
                }
                Ok(Update::State(self.rope_preview_state()))
            }
            Command::Play => {
                if !self.rope_preview.presentation_active {
                    return Err(
                        "Activate this document before playing; Seek and Advance remain available"
                            .into(),
                    );
                }
                if !self.view.rope_preview {
                    return Err("Enable Rope preview before playing".into());
                }
                if self.rope_preview.worker.busy() {
                    return Err("Wait for rope preparation or cancellation before playing".into());
                }
                let frame = self
                    .rope_preview_frame()
                    .ok_or("Prepare the rope preview before playing")?;
                if self.rope_preview.machine.is_none() {
                    return Err("Prepare the rope preview before playing".into());
                }
                if frame.preview.time >= ropes::MAX_TIME_SECONDS {
                    return Err(
                        "Rope playback reached its time limit; Reset or Seek before playing".into(),
                    );
                }
                if !self.rope_preview.playing {
                    self.rope_preview.playing = true;
                    self.rope_preview.control_serial += 1;
                    self.rope_preview.last_actor = Some(actor);
                    self.rope_preview_changed(false);
                    self.log(actor, "playing rope preview");
                }
                Ok(Update::State(self.rope_preview_state()))
            }
            Command::Prepare | Command::Seek { .. } | Command::Advance { .. } | Command::Reset => {
                self.rope_frame_command(actor, command)
            }
        }
    }
}
