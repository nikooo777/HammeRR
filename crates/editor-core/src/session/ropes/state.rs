use std::sync::Arc;
use std::time::Instant;

use compiler::asset_sources::AssetSourceGuard;
use glam::DVec3;
use map_core::Document;
use render::ropes::{Environment, Issue, Prepared, Preview, Simulation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::collision;
use crate::session::worker::{Slot, Ticket};
use crate::session::{Actor, Session};

pub const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_ADVANCE_SECONDS: f64 = 1.0;
pub const MAX_METADATA_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn cancel_control(control: &compiler::Control) {
    control
        .cancel()
        .expect("Rope preview controls remain cancellable");
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub gravity: f64,
    pub wind: [f64; 3],
}

impl Default for Settings {
    fn default() -> Self {
        let environment = Environment::default();
        Self {
            gravity: environment.gravity,
            wind: environment.wind.to_array(),
        }
    }
}

impl Settings {
    pub(super) fn environment(self) -> Result<Environment, String> {
        let environment = Environment {
            gravity: self.gravity,
            wind: DVec3::from_array(self.wind),
        };
        environment.validate()?;
        Ok(environment)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Command {
    Settings(Settings),
    Prepare,
    Play,
    Pause,
    Seek { time_seconds: f64 },
    Advance { seconds: f64 },
    Reset,
    Cancel,
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

pub struct Frame {
    pub preview: Arc<Preview>,
    pub prepared: Arc<Prepared>,
    pub appearance: Arc<render::ropes::materials::Prepared>,
    pub collision: CollisionSummary,
    pub parent_models: usize,
    pub simulation_work: usize,
    pub collision_work: usize,
    pub warmup_steps: usize,
    pub snapshot_bytes: usize,
    pub(super) key: Key,
    pub(super) verified: Instant,
    pub(super) sources: Arc<Sources>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CollisionSummary {
    pub enabled: bool,
    pub brushes: usize,
    pub triangles: usize,
    pub terrain_patches: usize,
    pub stationary_terrain_profile: Option<&'static str>,
    pub excluded_brush_entities: usize,
    pub excluded_model_entities: usize,
    pub disabled_terrain_faces: usize,
}

impl CollisionSummary {
    pub(super) fn capture(snapshot: Option<&collision::Snapshot>) -> Self {
        snapshot.map_or_else(Self::default, |snapshot| Self {
            enabled: true,
            brushes: snapshot.world.brushes(),
            triangles: snapshot.world.triangles(),
            terrain_patches: snapshot.world.terrain_patches(),
            stationary_terrain_profile: Some(snapshot.world.stationary_terrain_profile()),
            excluded_brush_entities: snapshot.excluded_brush_entities,
            excluded_model_entities: snapshot.excluded_model_entities,
            disabled_terrain_faces: snapshot.disabled_terrain_faces,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Key {
    generation: u64,
    revision: u64,
    document: usize,
    document_revision: u64,
    scene: Option<u64>,
    visibility: u64,
    content: u64,
    definitions: u64,
    models: u64,
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
            visibility: session.working_region.serial,
            content: session.content_epoch,
            definitions: session.definitions_serial,
            models: session.models.borrow().reload_generation(),
        }
    }

    pub(super) fn current(&self, session: &Session) -> bool {
        !session.closing && *self == Self::capture(session)
    }

    pub(super) fn document_matches(&self, document: &Document) -> bool {
        self.document == document as *const Document as usize
            && self.document_revision == document.revision()
    }

    pub(super) fn snapshot(&self, scope: &str, control: u64) -> String {
        let mut digest = Sha256::new();
        digest.update(scope.as_bytes());
        for value in [
            control,
            self.generation,
            self.revision,
            self.document as u64,
            self.document_revision,
            u64::from(self.scene.is_some()),
            self.scene.unwrap_or_default(),
            self.visibility,
            self.content,
            self.definitions,
            self.models,
        ] {
            digest.update(value.to_le_bytes());
        }
        format!("{:x}", digest.finalize())
    }
}

pub(super) struct Machine {
    pub simulation: Simulation,
    pub prepared: Arc<Prepared>,
    pub collision: Option<collision::Snapshot>,
    pub appearance: Arc<render::ropes::materials::Prepared>,
    pub appearance_sources: AssetSourceGuard,
    pub model_sources: Vec<Arc<render::models::Asset>>,
    pub snapshot_bytes: usize,
    pub verified: Instant,
    pub sources: Arc<Sources>,
}

pub(super) struct Sources {
    pub control: compiler::Control,
    pub appearance: AssetSourceGuard,
    pub collision: Option<AssetSourceGuard>,
    pub models: Vec<Arc<render::models::Asset>>,
}

pub(super) struct Pending {
    pub actor: Actor,
    pub token: String,
    pub identity: Arc<()>,
    pub control: compiler::Control,
    pub key: Key,
    pub preserve_cancelled_frame: bool,
}

pub(in crate::session) struct State {
    pub(super) worker: Slot,
    pub(super) clock: Slot,
    pub(super) clock_control: Option<compiler::Control>,
    pub(super) presentation_active: bool,
    pub(super) scope: String,
    pub(super) serial: u64,
    pub(super) frame_serial: u64,
    pub(super) control_serial: u64,
    pub(super) request_serial: u64,
    pub(super) settings: Settings,
    pub(super) requested: f64,
    pub(super) tick_remainder: f64,
    pub(super) playing: bool,
    pub(super) pending: Option<Pending>,
    pub(super) frame: Option<Frame>,
    pub(super) machine: Option<Machine>,
    pub(super) status: &'static str,
    pub(super) error: Option<String>,
    pub(super) issues: Vec<Issue>,
    pub(super) omitted: usize,
    pub(super) last_actor: Option<Actor>,
    pub(super) empty: Arc<Preview>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            worker: Slot::default(),
            clock: Slot::default(),
            clock_control: None,
            presentation_active: true,
            scope: crate::recovery::unique_id(),
            serial: 0,
            frame_serial: 0,
            control_serial: 0,
            request_serial: 0,
            settings: Settings::default(),
            requested: 0.0,
            tick_remainder: 0.0,
            playing: false,
            pending: None,
            frame: None,
            machine: None,
            status: "not_prepared",
            error: None,
            issues: Vec::new(),
            omitted: 0,
            last_actor: None,
            empty: Arc::new(Preview::default()),
        }
    }
}

impl State {
    pub(super) fn cancel(&self) {
        if let Some(pending) = &self.pending {
            cancel_control(&pending.control);
        }
    }

    pub(super) fn reset(&mut self) {
        self.cancel();
        self.stop_clock();
        if let Some(frame) = &self.frame {
            cancel_control(&frame.sources.control);
        }
        let worker = std::mem::take(&mut self.worker);
        let presentation_active = self.presentation_active;
        *self = Self {
            worker,
            presentation_active,
            ..Self::default()
        };
    }

    pub(super) fn stop_clock(&self) {
        if let Some(control) = &self.clock_control {
            cancel_control(control);
        }
    }

    pub(super) fn clock_lease(&mut self) -> Option<ClockLease> {
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
