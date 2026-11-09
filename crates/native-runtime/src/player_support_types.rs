use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::CaptureEnvironment;

pub const MAX_PLAYER_SUPPORT_SAMPLES: usize = 1024;
pub const MAX_PLAYER_SUPPORT_DURATION_MS: u64 = 30_000;
pub const MIN_PLAYER_SUPPORT_SAMPLES: usize = 3;
pub const MIN_PLAYER_SUPPORT_DURATION_MS: u64 = 100;
pub const MIN_PLAYER_SUPPORT_INTERVAL_MS: u64 = 15;
pub const MAX_PLAYER_SUPPORT_INTERVAL_MS: u64 = 1000;
pub const DEFAULT_PLAYER_SUPPORT_DURATION_MS: u64 = 5000;
pub const DEFAULT_PLAYER_SUPPORT_INTERVAL_MS: u64 = 50;
pub const DEFAULT_PLAYER_SUPPORT_MAX_SAMPLES: usize = 128;
pub const DEFAULT_PLAYER_SUPPORT_TEAM: u8 = 3;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerSupportSettings {
    pub team: u8,
    pub duration_ms: u64,
    pub interval_ms: u64,
    pub max_samples: usize,
}

impl Default for PlayerSupportSettings {
    fn default() -> Self {
        Self {
            team: DEFAULT_PLAYER_SUPPORT_TEAM,
            duration_ms: DEFAULT_PLAYER_SUPPORT_DURATION_MS,
            interval_ms: DEFAULT_PLAYER_SUPPORT_INTERVAL_MS,
            max_samples: DEFAULT_PLAYER_SUPPORT_MAX_SAMPLES,
        }
    }
}

impl PlayerSupportSettings {
    pub fn validate(&self) -> io::Result<()> {
        if !matches!(self.team, 2 | 3)
            || !(MIN_PLAYER_SUPPORT_DURATION_MS..=MAX_PLAYER_SUPPORT_DURATION_MS)
                .contains(&self.duration_ms)
            || !(MIN_PLAYER_SUPPORT_INTERVAL_MS..=MAX_PLAYER_SUPPORT_INTERVAL_MS)
                .contains(&self.interval_ms)
            || !(MIN_PLAYER_SUPPORT_SAMPLES..=MAX_PLAYER_SUPPORT_SAMPLES)
                .contains(&self.max_samples)
            || self.duration_ms < self.interval_ms * 2
            || self.duration_ms.div_ceil(self.interval_ms) + 1 > self.max_samples as u64
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Player support observation requires 100–30000ms duration, 15–1000ms intervals, and capacity for every requested sample (3–1024 rows)",
            ));
        }
        Ok(())
    }
}

pub struct PlayerSupportRequest {
    pub environment: CaptureEnvironment,
    pub bsp: Vec<u8>,
    pub map_name: String,
    pub artifacts_parent: PathBuf,
    pub settings: PlayerSupportSettings,
}

pub struct PlayerSupportCapture {
    pub samples: Vec<PlayerSupportSample>,
    pub observation: PlayerSupportObservationReceipt,
    pub artifacts: PathBuf,
    pub elapsed_ms: u64,
    pub preservation_passed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerSupportEntity {
    pub index: u32,
    pub serial: u32,
    pub reference_handle: u32,
    pub address: u64,
    pub classname: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerSupportSurface {
    pub index: i32,
    pub name: String,
    pub data_address: u64,
    pub friction: f32,
    pub material: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerSupportSample {
    pub phase: String,
    pub callback_sequence: u64,
    pub prior_simulating_callback_sequence: u64,
    pub prior_server_tick: i32,
    pub prior_server_time: f32,
    pub simulating: bool,
    pub callback_thread: u64,
    pub spawn_generation: u64,
    pub player_simulation_tick: i32,
    pub ordinal: usize,
    pub server_tick: i32,
    pub server_time: f32,
    pub server_frame_time: f32,
    pub server_tick_interval: f32,
    pub player_tickbase: i32,
    pub origin: [f32; 3],
    pub velocity: [f32; 3],
    pub absolute_velocity: [f32; 3],
    pub base_velocity: [f32; 3],
    pub collision_mins: [f32; 3],
    pub collision_maxs: [f32; 3],
    pub hull_mins: [f32; 3],
    pub hull_maxs: [f32; 3],
    pub model_scale: f32,
    pub ground: Option<PlayerSupportEntity>,
    pub ground_handle: u32,
    pub ground_resolution: String,
    pub surface: Option<PlayerSupportSurface>,
    pub cached_surface_index: i32,
    pub cached_surface_data_address: u64,
    pub surface_is_cached: bool,
    pub flags: u32,
    pub buttons: i32,
    pub health: i32,
    pub team: i32,
    pub move_type: u8,
    pub ducked: bool,
    pub player: PlayerSupportEntity,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerSupportObservationReceipt {
    pub profile: String,
    pub engine_sha256: String,
    pub server_sha256: String,
    pub vphysics_sha256: String,
    pub bsp_sha256: String,
    pub map_name: String,
    pub process: serde_json::Value,
    pub readiness: serde_json::Value,
    pub observer: serde_json::Value,
    pub resumed: bool,
}

pub fn validate_player_support_samples(
    samples: &[PlayerSupportSample],
    settings: &PlayerSupportSettings,
) -> io::Result<()> {
    settings.validate()?;
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Player support rows violate the completed passive sample contract",
        )
    };
    if samples.len() < 3 || samples.len() > settings.max_samples {
        return Err(invalid());
    }
    let first = &samples[0];
    for (ordinal, row) in samples.iter().enumerate() {
        if row.ordinal != ordinal
            || row.phase != "pre_server_gameframe_after_completed_step"
            || !row.simulating
            || row.callback_sequence < 2
            || row.prior_simulating_callback_sequence == 0
            || row.prior_simulating_callback_sequence >= row.callback_sequence
            || row.callback_thread == 0
            || row.callback_thread != first.callback_thread
            || row.spawn_generation == 0
            || row.spawn_generation != first.spawn_generation
            || row.player.address != first.player.address
            || row.player.reference_handle != first.player.reference_handle
            || row.player.index != first.player.index
            || row.player.serial != first.player.serial
            || row.player.classname != "player"
            || row.player.address == 0
            || !(1..=64).contains(&row.player.index)
            || row.player.reference_handle != ((row.player.serial << 16) | row.player.index)
            || !row.server_time.is_finite()
            || !row.server_frame_time.is_finite()
            || row.server_frame_time < 0.0
            || !row.server_tick_interval.is_finite()
            || row.server_tick_interval <= 0.0
            || row.server_tick_interval > 1.0
            || !row.prior_server_time.is_finite()
            || row.server_time <= row.prior_server_time
            || row.server_time - row.prior_server_time > 1.0
            || row.server_tick <= row.prior_server_tick
            || row.player_simulation_tick != row.prior_server_tick
            || row.player_tickbase < 0
            || row.health <= 0
            || row.team != i32::from(settings.team)
            || row.move_type != 2
            || row.flags & 0x40 != 0
            || row.buttons != 0
            || row.ducked != (row.flags & 2 != 0)
            || !row.model_scale.is_finite()
            || row.model_scale <= 0.0
            || row.model_scale > 1024.0
            || row.surface_is_cached != row.ground.is_none()
        {
            return Err(invalid());
        }
        for vector in [
            row.origin,
            row.velocity,
            row.absolute_velocity,
            row.base_velocity,
            row.hull_mins,
            row.hull_maxs,
            row.collision_mins,
            row.collision_maxs,
        ] {
            if vector.iter().any(|value| !value.is_finite()) {
                return Err(invalid());
            }
        }
        if (0..3).any(|axis| {
            row.hull_mins[axis] >= row.hull_maxs[axis]
                || row.collision_mins[axis] >= row.collision_maxs[axis]
        }) {
            return Err(invalid());
        }
        match &row.ground {
            Some(ground) => {
                if ground.index != 0
                    || ground.classname != "worldspawn"
                    || ground.address == 0
                    || ground.reference_handle != row.ground_handle
                    || ground.serial != ground.reference_handle >> 16
                    || row.ground_resolution != "serial_resolved_world"
                    || row.surface.is_none()
                {
                    return Err(invalid());
                }
            }
            None => {
                let expected = match row.ground_handle {
                    0 => "null_handle",
                    u32::MAX => "invalid_handle_resolves_null",
                    _ => return Err(invalid()),
                };
                if row.ground_resolution != expected {
                    return Err(invalid());
                }
            }
        }
        if let Some(surface) = &row.surface {
            if !(0..128).contains(&surface.index)
                || surface.index != row.cached_surface_index
                || surface.data_address == 0
                || surface.data_address != row.cached_surface_data_address
                || surface.name.is_empty()
                || surface.name.len() > 255
                || surface.name.as_bytes().contains(&0)
                || !surface.friction.is_finite()
                || !(0.0..=1.0).contains(&surface.friction)
            {
                return Err(invalid());
            }
        } else if row.cached_surface_data_address != 0 {
            return Err(invalid());
        }
        if ordinal > 0 {
            let previous = &samples[ordinal - 1];
            if row.callback_sequence <= previous.callback_sequence
                || row.prior_simulating_callback_sequence < previous.callback_sequence
                || row.server_tick <= previous.server_tick
                || row.server_time <= previous.server_time
                || row.player_tickbase < previous.player_tickbase
            {
                return Err(invalid());
            }
        }
    }
    let last = samples.last().ok_or_else(invalid)?;
    if (last.server_time - first.server_time) * 1000.0 < settings.duration_ms as f32
        || last.player_tickbase == first.player_tickbase
    {
        return Err(invalid());
    }
    Ok(())
}
