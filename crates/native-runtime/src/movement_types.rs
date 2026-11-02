use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{CaptureEnvironment, CleanupDisposition};

pub const MOVEMENT_STARTUP_SETTLE_MS: u16 = 500;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MovementTrainIdentity {
    pub hammer_id: i32,
    pub classname: String,
    pub targetname: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MovementMode {
    Stationary,
    StopStartStopStart,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MovementPhase {
    Stationary,
    Running,
    Stopped,
    Restarted,
}

pub struct MovementRequest {
    pub environment: CaptureEnvironment,
    pub bsp: Vec<u8>,
    pub map_name: String,
    pub source_sha256: String,
    pub train: MovementTrainIdentity,
    pub mode: MovementMode,
    pub sample_interval_ms: u16,
    pub samples_per_phase: u16,
    pub artifacts_parent: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MovementSample {
    pub sequence: usize,
    pub phase: MovementPhase,
    pub game_time: f64,
    pub origin: [f64; 3],
    pub entity_index: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MovementControlReceipt {
    pub phase: MovementPhase,
    pub input: String,
    pub game_time: f64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MovementReceipt {
    pub source_sha256: String,
    pub bsp_sha256: String,
    pub staged_bsp_sha256: String,
    pub train: MovementTrainIdentity,
    pub mode: MovementMode,
    pub samples: Vec<MovementSample>,
    pub controls: Vec<MovementControlReceipt>,
    pub artifacts: PathBuf,
    pub elapsed_ms: u64,
    pub startup_settle_ms: u16,
    pub verified_controls: bool,
    pub preservation_passed: bool,
    pub cleanup: CleanupDisposition,
    pub position_semantics: String,
    pub stationary_tolerance: f64,
    pub movement_threshold: f64,
}
