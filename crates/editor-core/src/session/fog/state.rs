use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use map_core::{Document, fog::Timeline};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{catalog::Catalog, environment};
use crate::session::worker::{Slot, Ticket};
use crate::session::{Actor, Session};

pub const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_ADVANCE_SECONDS: f64 = 60.0;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    #[default]
    Explicit,
    Environment,
}

impl SelectionMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "explicit" => Ok(Self::Explicit),
            "environment" => Ok(Self::Environment),
            _ => Err("Fog selection must be explicit or environment".into()),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Environment => "environment",
        }
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    #[default]
    Ldr,
    Hdr,
}

impl Channel {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ldr" => Ok(Self::Ldr),
            "hdr" => Ok(Self::Hdr),
            _ => Err("Fog channel must be ldr or hdr".into()),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Ldr => "ldr",
            Self::Hdr => "hdr",
        }
    }

    pub(super) fn validate(self) -> Result<(), String> {
        if self == Self::Hdr {
            return Err("HDR fog is unavailable: the installed framebuffer color consumer has not been qualified".into());
        }
        Ok(())
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub controller: Option<u32>,
    pub selection: SelectionMode,
    pub channel: Channel,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Overrides {
    pub enabled: Option<bool>,
    pub controller: Option<u32>,
    pub selection: Option<SelectionMode>,
    pub channel: Option<Channel>,
}

#[derive(Debug, Clone, Copy)]
pub enum Command {
    Settings(Settings),
    PrepareEnvironment,
    Input {
        controller: u32,
        input: map_core::fog::Input,
    },
    Play,
    Pause,
    Seek {
        time_seconds: f64,
    },
    Advance {
        seconds: f64,
    },
    Reset,
    Cancel,
    Clear,
}

pub struct FogView {
    pub fog: Option<render::fog::ViewFog>,
    pub report: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct Clock {
    pub snapshot: String,
    pub serial: u64,
    pub frame_serial: u64,
    pub status: &'static str,
    pub playing: bool,
    pub time_seconds: f64,
    pub requested_time_seconds: f64,
    pub busy: bool,
    pub presentation_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Key {
    generation: u64,
    revision: u64,
    document: usize,
    document_revision: u64,
    scene: Option<u64>,
    content: u64,
}

impl Key {
    pub(super) fn capture(session: &Session) -> Self {
        let document = session.render_document();
        Self {
            generation: session.document_generation,
            revision: session.document.revision(),
            document: document as *const Document as usize,
            document_revision: document.revision(),
            scene: (!std::ptr::eq(document, &session.document)).then_some(session.scene_serial),
            content: session.content_epoch,
        }
    }

    pub(super) fn current(&self, session: &Session) -> bool {
        !session.closing && *self == Self::capture(session)
    }

    pub(super) fn snapshot(&self, scope: &str, control: u64) -> String {
        let mut hash = Sha256::new();
        hash.update(scope.as_bytes());
        for value in [
            control,
            self.generation,
            self.revision,
            self.document as u64,
            self.document_revision,
            u64::from(self.scene.is_some()),
            self.scene.unwrap_or_default(),
            self.content,
        ] {
            hash.update(value.to_le_bytes());
        }
        format!("{:x}", hash.finalize())
    }
}

pub(super) struct PreparedEnvironment {
    pub snapshot: environment::Snapshot,
    pub control: compiler::Control,
}

pub(super) struct Pending {
    pub actor: Actor,
    pub token: String,
    pub identity: Arc<()>,
    pub control: compiler::Control,
}

pub(crate) struct ClockLease {
    pub scope: String,
    control: compiler::Control,
    _ticket: Ticket,
}

impl ClockLease {
    pub(crate) fn cancelled(&self) -> bool {
        self.control.cancelled()
    }
}

pub(in crate::session) struct State {
    pub(super) worker: Slot,
    pub(super) clock: Slot,
    pub(super) clock_control: Option<compiler::Control>,
    pub(super) scope: String,
    pub(super) serial: u64,
    pub(super) frame_serial: u64,
    pub(super) control_serial: u64,
    pub(super) request_serial: u64,
    pub(super) key: Option<Key>,
    pub(super) invalidated: bool,
    pub(super) catalog: Option<Catalog>,
    pub(super) settings: Settings,
    pub(super) timelines: BTreeMap<u32, Timeline>,
    pub(super) time: f64,
    pub(super) playing: bool,
    pub(super) presentation_active: bool,
    pub(super) pending: Option<Pending>,
    pub(super) environment: Option<Arc<PreparedEnvironment>>,
    pub(super) verified: Option<Instant>,
    pub(super) error: Option<String>,
    pub(super) last_actor: Option<Actor>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            worker: Slot::default(),
            clock: Slot::default(),
            clock_control: None,
            scope: crate::recovery::unique_id(),
            serial: 0,
            frame_serial: 0,
            control_serial: 0,
            request_serial: 0,
            key: None,
            invalidated: false,
            catalog: None,
            settings: Settings::default(),
            timelines: BTreeMap::new(),
            time: 0.0,
            playing: false,
            presentation_active: true,
            pending: None,
            environment: None,
            verified: None,
            error: None,
            last_actor: None,
        }
    }
}

pub(super) fn cancel(control: &compiler::Control) {
    control
        .cancel()
        .expect("Fog preview controls remain cancellable");
}

impl State {
    pub(super) fn stop_clock(&self) {
        if let Some(control) = &self.clock_control {
            cancel(control);
        }
    }

    pub(super) fn cancel(&self) {
        if let Some(pending) = &self.pending {
            cancel(&pending.control);
        }
        if let Some(environment) = &self.environment {
            cancel(&environment.control);
        }
        self.stop_clock();
    }

    pub(super) fn reset(&mut self) {
        self.cancel();
        let worker = std::mem::take(&mut self.worker);
        let clock = std::mem::take(&mut self.clock);
        let presentation_active = self.presentation_active;
        *self = Self {
            worker,
            clock,
            presentation_active,
            ..Self::default()
        };
    }

    pub(super) fn lease(&mut self) -> Option<ClockLease> {
        let ticket = self.clock.begin()?;
        let control = compiler::Control::default();
        self.clock_control = Some(control.clone());
        Some(ClockLease {
            scope: self.scope.clone(),
            control,
            _ticket: ticket,
        })
    }
}
