use std::collections::BTreeMap;
use std::sync::Arc;

use map_core::fog::MAX_TIMELINE_SECONDS;
use serde_json::Value;

use super::catalog::Catalog;
use super::state::{Key, Pending, cancel};
use super::{ClockLease, Command, Completed, Request, SelectionMode, Settings, preparation};
use crate::session::{Actor, Session};

pub(crate) enum Update {
    State(Value),
    Work(Box<Request>),
}

fn checked_time(time: f64) -> Result<f64, String> {
    if !time.is_finite() || !(0.0..=MAX_TIMELINE_SECONDS).contains(&time) {
        return Err(format!(
            "Fog time must be finite between 0 and {MAX_TIMELINE_SECONDS} seconds"
        ));
    }
    Ok(time)
}

impl Session {
    fn fog_changed(&mut self, control: bool) {
        self.fog_preview.serial += 1;
        self.fog_preview.frame_serial += 1;
        if control {
            self.fog_preview.control_serial += 1;
            self.view_serial += 1;
        }
    }

    fn retire_fog_bounds_reviews(&mut self) {
        let changed = self.area_preview.take().is_some() || self.volume_preview().is_some();
        self.volume.retire_presentation_preview();
        if changed {
            self.interaction_epoch += 1;
        }
    }

    pub(crate) fn preflight_fog_preview(&mut self) {
        let stale = self
            .fog_preview
            .key
            .as_ref()
            .is_some_and(|key| !key.current(self));
        if stale && !self.fog_preview.invalidated {
            if let Some(pending) = &self.fog_preview.pending {
                cancel(&pending.control);
            }
            if let Some(environment) = &self.fog_preview.environment {
                cancel(&environment.control);
            }
            self.fog_preview.environment = None;
            self.fog_preview.timelines.clear();
            self.fog_preview.input_events.clear();
            self.fog_preview.player = None;
            self.fog_preview.time = 0.0;
            self.fog_preview.playing = false;
            self.fog_preview.invalidated = true;
            self.fog_preview.error = Some(
                "Fog source changed; apply settings or Reset to capture a new baseline".into(),
            );
            self.retire_fog_bounds_reviews();
            self.fog_changed(true);
        }
    }

    fn fog_baseline(&mut self, catalog: Option<Catalog>) {
        self.fog_preview.key = Some(Key::capture(self));
        self.fog_preview.catalog = catalog;
        self.fog_preview.timelines.clear();
        self.fog_preview.input_events.clear();
        self.fog_preview.player = None;
        self.fog_preview.environment = None;
        self.fog_preview.verified = None;
        self.fog_preview.time = 0.0;
        self.fog_preview.playing = false;
        self.fog_preview.invalidated = false;
        self.fog_preview.error = None;
    }

    fn ensure_fog_baseline(&self) -> Result<(), String> {
        if !self
            .fog_preview
            .key
            .as_ref()
            .is_some_and(|key| key.current(self))
        {
            return Err(
                "Apply Fog preview settings or Reset before controlling its timeline".into(),
            );
        }
        Ok(())
    }

    pub(crate) fn fog_preview_command(
        &mut self,
        actor: Actor,
        expected: &str,
        command: Command,
    ) -> Result<Update, String> {
        self.ensure_not_closing()?;
        if expected != self.fog_preview_snapshot() {
            return Err(
                "Fog source or controls changed; refresh Fog preview before applying".into(),
            );
        }
        self.preflight_fog_preview();
        if self.fog_preview.worker.busy()
            && !matches!(command, Command::Pause | Command::Cancel | Command::Clear)
        {
            return Err(
                "Wait for fog environment preparation or cancel it before changing fog controls"
                    .into(),
            );
        }
        match command {
            Command::Pause | Command::Cancel => {
                self.fog_preview.playing = false;
                if matches!(command, Command::Cancel)
                    && let Some(pending) = &self.fog_preview.pending
                {
                    cancel(&pending.control);
                }
            }
            Command::Clear => {
                if let Some(pending) = &self.fog_preview.pending {
                    cancel(&pending.control);
                }
                if let Some(environment) = &self.fog_preview.environment {
                    cancel(&environment.control);
                }
                self.fog_preview.settings = Settings::default();
                self.fog_preview.catalog = None;
                self.fog_preview.key = None;
                self.fog_preview.timelines.clear();
                self.fog_preview.input_events.clear();
                self.fog_preview.player = None;
                self.fog_preview.environment = None;
                self.fog_preview.verified = None;
                self.fog_preview.time = 0.0;
                self.fog_preview.playing = false;
                self.fog_preview.error = None;
                self.fog_preview.invalidated = false;
            }
            Command::Settings(settings) => {
                settings.channel.validate()?;
                let context_changed = settings.initial_radial_context
                    != self.fog_preview.settings.initial_radial_context;
                let sequence_changed = context_changed
                    || settings.controller != self.fog_preview.settings.controller
                    || settings.selection != self.fog_preview.settings.selection;
                if sequence_changed {
                    self.validate_player_fog(self.fog_preview.time)?;
                }
                let fresh = !self
                    .fog_preview
                    .key
                    .as_ref()
                    .is_some_and(|key| key.current(self));
                let captured = (settings.enabled && (fresh || self.fog_preview.catalog.is_none()))
                    .then(|| Catalog::capture(self.render_document().vmf()))
                    .transpose()?;
                if sequence_changed
                    && !settings.enabled
                    && settings.selection == SelectionMode::Explicit
                    && let Some(id) = settings.controller
                {
                    self.initial_fog_profile(id)?;
                }
                if settings.enabled && settings.selection == SelectionMode::Explicit {
                    let id = settings.controller.ok_or(
                        "Choose an existing env_fog_controller; no controller is selected",
                    )?;
                    if let Some(catalog) = &captured {
                        catalog
                            .controllers
                            .get(&id)
                            .ok_or("Selected fog controller no longer exists")?
                            .parsed
                            .as_ref()
                            .map_err(Clone::clone)?;
                    } else if !context_changed {
                        self.fog_profile(id, self.fog_preview.time)?;
                    } else {
                        self.initial_fog_profile(id)?;
                    }
                }
                if fresh {
                    self.fog_baseline(captured);
                } else if let Some(catalog) = captured {
                    self.fog_preview.catalog = Some(catalog);
                }
                if sequence_changed {
                    self.fog_preview.timelines.clear();
                    self.fog_preview.input_events.clear();
                    self.fog_preview.player = None;
                    self.fog_preview.time = 0.0;
                    self.fog_preview.playing = false;
                }
                self.fog_preview.settings = settings;
                if !settings.enabled {
                    self.fog_preview.playing = false;
                }
                self.fog_preview.error = None;
            }
            Command::PrepareEnvironment => {
                if let Some(error) = &self.game_error {
                    return Err(format!("Fog environment content is unavailable: {error}"));
                }
                let fresh = !self
                    .fog_preview
                    .key
                    .as_ref()
                    .is_some_and(|key| key.current(self));
                let captured = fresh
                    .then(|| Catalog::capture(self.render_document().vmf()))
                    .transpose()?;
                let document = self.render_document();
                map_core::references::admit_snapshot_storage(
                    document.vmf(),
                    super::state::MAX_SOURCE_BYTES,
                    &mut || Ok(()),
                )?;
                let document = Box::new(document.preview_snapshot_uncached());
                let ticket = self
                    .fog_preview
                    .worker
                    .begin()
                    .ok_or("Fog environment worker is already active")?;
                if let Some(catalog) = captured {
                    self.fog_baseline(Some(catalog));
                }
                let control = compiler::Control::default();
                let identity = Arc::new(());
                self.fog_preview.request_serial += 1;
                let token = format!(
                    "{}:{}",
                    self.fog_preview.scope, self.fog_preview.request_serial
                );
                self.fog_preview.pending = Some(Pending {
                    actor,
                    token: token.clone(),
                    identity: identity.clone(),
                    control: control.clone(),
                });
                self.fog_preview.playing = false;
                self.fog_preview.error = None;
                self.fog_preview.last_actor = Some(actor);
                self.retire_fog_bounds_reviews();
                self.fog_changed(true);
                let request = Request {
                    token,
                    identity,
                    control,
                    key: Key::capture(self),
                    game: self.game.clone(),
                    input: preparation::Input::Prepare(document),
                    ticket,
                };
                self.log(actor, "preparing fog environment");
                return Ok(Update::Work(Box::new(request)));
            }
            Command::Input { controller, input } => {
                self.append_controller_input(controller, input)?;
                self.fog_preview.error = None;
            }
            Command::SwitchPlayerController { controller } => {
                self.ensure_fog_baseline()?;
                self.append_player_switch(controller)?;
                self.fog_preview.error = None;
            }
            Command::Play => {
                self.ensure_fog_baseline()?;
                if !self.fog_preview.settings.enabled {
                    return Err("Enable Fog preview before playing".into());
                }
                if !self.fog_preview.presentation_active {
                    return Err(
                        "Activate this document before playing; Seek and Advance remain available"
                            .into(),
                    );
                }
                if self.fog_preview.time >= MAX_TIMELINE_SECONDS {
                    return Err(
                        "Fog playback reached its time limit; Seek or Reset before playing".into(),
                    );
                }
                self.fog_render_state_for_view(
                    &self.camera(),
                    self.view.surface,
                    super::Overrides::default(),
                )?;
                self.fog_preview.playing = true;
            }
            Command::Seek { time_seconds } => {
                self.ensure_fog_baseline()?;
                let time = checked_time(time_seconds)?;
                Self::validate_fog_timelines(&self.fog_preview.timelines, time)?;
                self.validate_player_fog(time)?;
                self.fog_preview.time = time;
                self.fog_preview.playing = false;
                self.fog_preview.error = None;
            }
            Command::Advance { seconds } => {
                self.ensure_fog_baseline()?;
                if !seconds.is_finite()
                    || !(0.0..=super::state::MAX_ADVANCE_SECONDS).contains(&seconds)
                {
                    return Err("Fog Advance seconds must be finite between 0 and 60".into());
                }
                let time = checked_time(self.fog_preview.time + seconds)?;
                Self::validate_fog_timelines(&self.fog_preview.timelines, time)?;
                self.validate_player_fog(time)?;
                self.fog_preview.time = time;
                self.fog_preview.error = None;
            }
            Command::Reset => {
                let fresh = !self
                    .fog_preview
                    .key
                    .as_ref()
                    .is_some_and(|key| key.current(self));
                if fresh {
                    let catalog = Catalog::capture(self.render_document().vmf())?;
                    self.fog_baseline(Some(catalog));
                }
                self.fog_preview.timelines = BTreeMap::new();
                self.fog_preview.input_events.clear();
                self.fog_preview.player = None;
                self.fog_preview.time = 0.0;
                self.fog_preview.playing = false;
                self.fog_preview.error = None;
            }
        }
        self.fog_preview.last_actor = Some(actor);
        if matches!(
            command,
            Command::Settings(_)
                | Command::Input {
                    input: map_core::fog::Input::SetFarZ(_),
                    ..
                }
                | Command::Reset
                | Command::Clear
                | Command::SwitchPlayerController { .. }
        ) {
            self.retire_fog_bounds_reviews();
        }
        self.fog_changed(true);
        self.log(actor, format!("fog preview control: {command:?}"));
        Ok(Update::State(self.fog_preview_state()))
    }

    pub fn set_fog_preview(
        &mut self,
        enabled: bool,
        controller: Option<u32>,
        snapshot: &str,
    ) -> Result<Value, String> {
        let settings = Settings {
            enabled,
            controller,
            selection: SelectionMode::Explicit,
            channel: self.fog_preview.settings.channel,
            initial_radial_context: self.fog_preview.settings.initial_radial_context,
        };
        match self.fog_preview_command(Actor::User, snapshot, Command::Settings(settings))? {
            Update::State(report) => Ok(report),
            Update::Work(_) => unreachable!(),
        }
    }

    pub(crate) fn finish_fog_preview(&mut self, completed: Completed) -> Result<(), String> {
        let matches = self.fog_preview.pending.as_ref().is_some_and(|pending| {
            pending.token == completed.token && Arc::ptr_eq(&pending.identity, &completed.identity)
        });
        if !matches {
            return Err("Fog environment result belongs to an expired request".into());
        }
        let cancelled = completed.control.cancelled();
        self.fog_preview.pending = None;
        if !completed.key.current(self) {
            self.preflight_fog_preview();
        } else if cancelled {
            self.fog_preview.playing = false;
        } else {
            match completed.result {
                Ok(environment) => {
                    self.fog_preview.environment =
                        Some(Arc::new(super::state::PreparedEnvironment {
                            snapshot: environment,
                            control: compiler::Control::default(),
                        }));
                    self.fog_preview.verified = Some(std::time::Instant::now());
                    self.fog_preview.error = None;
                }
                Err(error) => {
                    self.fog_preview.environment = None;
                    self.fog_preview.error = Some(error);
                    self.fog_preview.playing = false;
                }
            }
        }
        drop(completed.ticket);
        self.retire_fog_bounds_reviews();
        self.fog_changed(true);
        Ok(())
    }

    pub(crate) fn fail_fog_preview(&mut self, token: &str, error: String) -> Result<(), String> {
        if !self
            .fog_preview
            .pending
            .as_ref()
            .is_some_and(|pending| pending.token == token)
        {
            return Err("Fog task belongs to an expired request".into());
        }
        self.fog_preview.pending = None;
        self.fog_preview.environment = None;
        self.fog_preview.error = Some(error);
        self.fog_preview.playing = false;
        self.fog_changed(true);
        Ok(())
    }

    pub(crate) fn begin_fog_clock(&mut self) -> Option<ClockLease> {
        if self.closing {
            None
        } else {
            self.fog_preview.lease()
        }
    }

    pub(crate) fn fog_clock_draining(&self) -> bool {
        self.fog_preview.clock.busy()
            && self
                .fog_preview
                .clock_control
                .as_ref()
                .is_none_or(|control| control.cancelled())
    }

    pub(crate) fn fog_clock_scope(&self) -> String {
        self.fog_preview.scope.clone()
    }

    pub(crate) fn fog_clock_current(&self, scope: &str) -> bool {
        !self.closing && self.fog_preview.scope == scope
    }

    pub(crate) fn fog_clock_due(&self) -> bool {
        !self.closing
            && self.fog_preview.presentation_active
            && (self
                .fog_preview
                .key
                .as_ref()
                .is_some_and(|key| !key.current(self))
                && !self.fog_preview.invalidated
                || self.fog_preview.playing
                || self.fog_verification_due())
    }

    pub(crate) fn tick_fog_clock(&mut self, scope: &str, seconds: f64) -> Result<bool, String> {
        if !self.fog_clock_current(scope) {
            return Err("Fog clock is expired".into());
        }
        self.preflight_fog_preview();
        if !self.fog_preview.presentation_active
            || !self.fog_preview.playing
            || self.fog_preview.worker.busy()
        {
            return Ok(false);
        }
        let time = (self.fog_preview.time + seconds).min(MAX_TIMELINE_SECONDS);
        if let Err(error) = Self::validate_fog_timelines(&self.fog_preview.timelines, time) {
            self.fog_preview.playing = false;
            self.fog_preview.error = Some(error.clone());
            self.fog_changed(false);
            return Err(error);
        }
        if let Err(error) = self.validate_player_fog(time) {
            self.fog_preview.playing = false;
            self.fog_preview.error = Some(error.clone());
            self.fog_changed(false);
            return Err(error);
        }
        self.fog_preview.time = time;
        if time >= MAX_TIMELINE_SECONDS {
            self.fog_preview.playing = false;
        }
        self.fog_changed(false);
        Ok(true)
    }

    pub fn set_fog_presentation_active(&mut self, active: bool) -> Result<(), String> {
        self.ensure_not_closing()?;
        if self.fog_preview.presentation_active != active {
            self.fog_preview.presentation_active = active;
            self.fog_preview.serial += 1;
        }
        Ok(())
    }

    pub(in crate::session) fn reset_fog_preview(&mut self) {
        self.fog_preview.reset();
    }

    pub(in crate::session) fn shutdown_fog_preview(&mut self) {
        self.fog_preview.cancel();
        self.fog_preview.playing = false;
        self.fog_preview.environment = None;
        self.fog_changed(true);
    }

    pub(in crate::session) fn fog_preview_worker_busy(&self) -> bool {
        self.fog_preview.worker.busy()
    }

    pub(crate) fn fog_preview_shutdown_busy(&self) -> bool {
        self.fog_preview.worker.busy() || self.fog_preview.clock.busy()
    }

    pub(crate) fn fail_fog_clock(&mut self, scope: &str, error: String) {
        if self.fog_clock_current(scope) {
            self.fog_preview.playing = false;
            self.fog_preview.error = Some(error);
            self.fog_changed(false);
        }
    }
}
