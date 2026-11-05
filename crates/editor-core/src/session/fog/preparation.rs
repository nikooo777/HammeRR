use std::sync::Arc;
use std::time::{Duration, Instant};

use map_core::Document;
use source_formats::gamefs::GameFs;

use super::state::{Key, PreparedEnvironment};
use super::{Overrides, SelectionMode, environment};
use crate::session::Session;
use crate::session::worker::Ticket;

pub(super) const VERIFY_INTERVAL: Duration = Duration::from_secs(1);

pub(super) enum Input {
    Prepare(Box<Document>),
}

pub struct Request {
    pub(super) token: String,
    pub(super) identity: Arc<()>,
    pub(super) control: compiler::Control,
    pub(super) key: Key,
    pub(super) game: Arc<GameFs>,
    pub(super) input: Input,
    pub(super) ticket: Ticket,
}

pub struct Completed {
    pub(super) token: String,
    pub(super) identity: Arc<()>,
    pub(super) control: compiler::Control,
    pub(super) key: Key,
    pub(super) result: Result<environment::Snapshot, String>,
    pub(super) ticket: Ticket,
}

fn verify(environment: &PreparedEnvironment, game: &GameFs) -> Result<(), String> {
    environment.control.checkpoint()?;
    environment
        .snapshot
        .sources
        .verify_original_metadata(&environment.control, environment::MAX_SOURCE_METADATA)
        .map_err(|error| format!("Fog environment sources changed; prepare again: {error}"))?;
    environment
        .snapshot
        .sources
        .verify_sources(game, &environment.control)
        .map_err(|error| {
            format!("Fog environment source resolution changed; prepare again: {error}")
        })?;
    environment.control.checkpoint()
}

impl Request {
    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn run(self) -> Completed {
        let Self {
            token,
            identity,
            control,
            key,
            game,
            input,
            ticket,
        } = self;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            control.checkpoint()?;
            let Input::Prepare(document) = input;
            let snapshot = environment::prepare(document.vmf(), &game, &control)?;
            snapshot
                .sources
                .verify_original_metadata(&control, environment::MAX_SOURCE_METADATA)
                .map_err(|error| error.to_string())?;
            snapshot
                .sources
                .verify_sources(&game, &control)
                .map_err(|error| error.to_string())?;
            control.checkpoint()?;
            Ok(snapshot)
        }))
        .unwrap_or_else(|_| Err("Fog environment preparation panicked".into()));
        Completed {
            token,
            identity,
            control,
            key,
            result,
            ticket,
        }
    }
}

#[derive(Clone)]
pub(crate) struct RenderGuard {
    key: Key,
    control_serial: u64,
    environment: Option<Arc<PreparedEnvironment>>,
    game: Arc<GameFs>,
}

pub(crate) struct VerifiedGuard {
    guard: RenderGuard,
    result: Result<(), String>,
}

impl RenderGuard {
    pub(crate) fn verify_original_sources(&self) -> Result<(), String> {
        if let Some(environment) = &self.environment {
            verify(environment, &self.game)?;
        }
        Ok(())
    }

    pub(crate) fn run(self) -> VerifiedGuard {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.verify_original_sources()
        }))
        .unwrap_or_else(|_| Err("Fog source verification panicked".into()));
        VerifiedGuard {
            guard: self,
            result,
        }
    }
}

impl Session {
    pub(crate) fn has_current_fog_environment(&self) -> bool {
        self.fog_preview.environment.is_some()
            && self
                .fog_preview
                .key
                .as_ref()
                .is_some_and(|key| key.current(self))
    }

    pub(crate) fn fog_verification_due(&self) -> bool {
        self.fog_preview.settings.enabled
            && self.fog_preview.settings.selection == SelectionMode::Environment
            && self.fog_preview.environment.is_some()
            && self
                .fog_preview
                .verified
                .is_none_or(|time| time.elapsed() >= VERIFY_INTERVAL)
    }

    pub(crate) fn fog_preview_render_guard(
        &self,
        overrides: Overrides,
    ) -> Result<RenderGuard, String> {
        self.ensure_not_closing()?;
        let settings = self.fog_settings_for(overrides)?;
        let environment = if settings.enabled {
            settings.channel.validate()?;
            if self
                .fog_preview
                .key
                .as_ref()
                .is_some_and(|key| !key.current(self))
            {
                return Err("Fog source changed; apply settings or Reset before rendering".into());
            }
            if settings.selection == SelectionMode::Environment {
                Some(
                    self.fog_preview.environment.clone().ok_or(
                        "Prepare the fog environment before rendering automatic selection",
                    )?,
                )
            } else {
                None
            }
        } else {
            None
        };
        Ok(RenderGuard {
            key: Key::capture(self),
            control_serial: self.fog_preview.control_serial,
            environment,
            game: self.game.clone(),
        })
    }

    pub(crate) fn finish_fog_render_guard(
        &mut self,
        verified: VerifiedGuard,
    ) -> Result<(), String> {
        let VerifiedGuard { guard, result } = verified;
        let same_environment = guard.environment.as_ref().is_none_or(|expected| {
            self.fog_preview
                .environment
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(expected, current))
        });
        if !guard.key.current(self)
            || guard.control_serial != self.fog_preview.control_serial
            || !same_environment
        {
            return Err("Fog source or controls changed during verification".into());
        }
        match result {
            Ok(()) => {
                if guard.environment.is_some() {
                    self.fog_preview.verified = Some(Instant::now());
                }
                Ok(())
            }
            Err(error) => {
                if guard.environment.is_some() {
                    self.fog_preview.environment = None;
                    self.fog_preview.verified = None;
                    self.fog_preview.playing = false;
                    self.fog_preview.error = Some(error.clone());
                    self.fog_preview.control_serial += 1;
                    self.fog_preview.serial += 1;
                    self.fog_preview.frame_serial += 1;
                    self.view_serial += 1;
                }
                Err(error)
            }
        }
    }
}
