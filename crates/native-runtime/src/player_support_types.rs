use std::collections::BTreeMap;
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
pub const MAX_PLAYER_SUPPORT_RESULT_BYTES: usize = 4 * 1024 * 1024;
pub const MIN_PLAYER_SUPPORT_PROBE_DISTANCE: u16 = 1;
pub const MAX_PLAYER_SUPPORT_PROBE_DISTANCE: u16 = 128;
pub(crate) const PLAYER_SUPPORT_MOVEMENT_MASK: u32 = 0x0201400b;
pub(crate) const PLAYER_SUPPORT_COLLISION_GROUP: i32 = 8;
pub const MAX_PLAYER_SUPPORT_PROBE_COORDINATE: f32 = 1_048_576.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerSupportInput {
    #[default]
    Idle,
    Forward,
    Crouch,
    ForwardCrouch,
}

impl PlayerSupportInput {
    pub const ALL: [Self; 4] = [Self::Idle, Self::Forward, Self::Crouch, Self::ForwardCrouch];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Forward => "forward",
            Self::Crouch => "crouch",
            Self::ForwardCrouch => "forward_crouch",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Forward => "Forward",
            Self::Crouch => "Crouch",
            Self::ForwardCrouch => "Forward + Crouch",
        }
    }

    pub const fn expected_buttons(self) -> i32 {
        match self {
            Self::Idle => 0,
            Self::Forward => 8,
            Self::Crouch => 4,
            Self::ForwardCrouch => 12,
        }
    }
}

impl std::str::FromStr for PlayerSupportInput {
    type Err = io::Error;

    fn from_str(value: &str) -> io::Result<Self> {
        Self::ALL
            .into_iter()
            .find(|input| input.as_str() == value)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Player support input must be idle, forward, crouch or forward_crouch",
                )
            })
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerSupportSettings {
    pub team: u8,
    #[serde(default)]
    pub input: PlayerSupportInput,
    #[serde(default)]
    pub probe_distance: Option<u16>,
    pub duration_ms: u64,
    pub interval_ms: u64,
    pub max_samples: usize,
}

impl Default for PlayerSupportSettings {
    fn default() -> Self {
        Self {
            team: DEFAULT_PLAYER_SUPPORT_TEAM,
            input: PlayerSupportInput::Idle,
            probe_distance: None,
            duration_ms: DEFAULT_PLAYER_SUPPORT_DURATION_MS,
            interval_ms: DEFAULT_PLAYER_SUPPORT_INTERVAL_MS,
            max_samples: DEFAULT_PLAYER_SUPPORT_MAX_SAMPLES,
        }
    }
}

impl PlayerSupportSettings {
    pub fn validate(&self) -> io::Result<()> {
        if !matches!(self.team, 2 | 3)
            || self.probe_distance.is_some_and(|distance| {
                !(MIN_PLAYER_SUPPORT_PROBE_DISTANCE..=MAX_PLAYER_SUPPORT_PROBE_DISTANCE)
                    .contains(&distance)
            })
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
                "Player support observation requires team 2 or 3, an optional 1–128 unit probe, 100–30000ms duration, 15–1000ms intervals, and capacity for every requested sample (3–1024 rows)",
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlayerSupportGroundState {
    World {
        edict: u64,
    },
    FuncMovelinear {
        edict: u64,
        hammer_id: i32,
        model_name: String,
        bsp_model_index: u32,
        model_index: u16,
        move_parent_handle: u32,
        move_type: u8,
        solid_type: u8,
        solid_flags: u16,
        origin: [f32; 3],
        local_origin: [f32; 3],
        angles: [f32; 3],
        local_angles: [f32; 3],
        velocity: [f32; 3],
        absolute_velocity: [f32; 3],
        base_velocity: [f32; 3],
        collision_mins: [f32; 3],
        collision_maxs: [f32; 3],
        eflags: u32,
    },
}

impl PlayerSupportGroundState {
    pub const fn edict(&self) -> u64 {
        match self {
            Self::World { edict } | Self::FuncMovelinear { edict, .. } => *edict,
        }
    }

    pub fn model_name(&self) -> Option<&str> {
        match self {
            Self::World { .. } => None,
            Self::FuncMovelinear { model_name, .. } => Some(model_name),
        }
    }

    pub(crate) fn same_identity(&self, other: &Self) -> bool {
        if self.edict() != other.edict() || self.model_name() != other.model_name() {
            return false;
        }
        match (self, other) {
            (Self::World { .. }, Self::World { .. }) => true,
            (
                Self::FuncMovelinear {
                    hammer_id: first_hammer,
                    bsp_model_index: first_bsp,
                    model_index: first_native,
                    ..
                },
                Self::FuncMovelinear {
                    hammer_id: second_hammer,
                    bsp_model_index: second_bsp,
                    model_index: second_native,
                    ..
                },
            ) => {
                first_hammer == second_hammer
                    && first_bsp == second_bsp
                    && first_native == second_native
            }
            _ => false,
        }
    }

    pub(crate) fn same_bits(&self, other: &Self) -> bool {
        if self != other {
            return false;
        }
        let vectors = |value: &Self| match value {
            Self::World { .. } => None,
            Self::FuncMovelinear {
                origin,
                local_origin,
                angles,
                local_angles,
                velocity,
                absolute_velocity,
                base_velocity,
                collision_mins,
                collision_maxs,
                ..
            } => Some([
                *origin,
                *local_origin,
                *angles,
                *local_angles,
                *velocity,
                *absolute_velocity,
                *base_velocity,
                *collision_mins,
                *collision_maxs,
            ]),
        };
        vectors(self).map(|values| values.map(|vector| vector.map(f32::to_bits)))
            == vectors(other).map(|values| values.map(|vector| vector.map(f32::to_bits)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerSupportProbeStatus {
    Miss,
    Impact,
    StartSolid,
    AllSolid,
}

impl PlayerSupportProbeStatus {
    pub(crate) fn from_native(fraction: f32, start_solid: bool, all_solid: bool) -> Self {
        if all_solid {
            Self::AllSolid
        } else if start_solid {
            Self::StartSolid
        } else if fraction < 1.0 {
            Self::Impact
        } else {
            Self::Miss
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerSupportProbePlane {
    pub available: bool,
    pub normal: [f32; 3],
    pub distance: f32,
    pub plane_type: u8,
    pub sign_bits: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerSupportProbe {
    pub distance: u16,
    pub start: [f32; 3],
    pub requested_end: [f32; 3],
    pub mask: u32,
    pub collision_group: i32,
    pub native_start: [f32; 3],
    pub native_end: [f32; 3],
    pub fraction: f32,
    pub fraction_left_solid: f32,
    pub all_solid: bool,
    pub start_solid: bool,
    pub status: PlayerSupportProbeStatus,
    pub plane: PlayerSupportProbePlane,
    pub contents: i32,
    pub displacement_flags: u16,
    pub hit_group: i32,
    pub physics_bone: i16,
    pub hitbox: i32,
    pub hit_entity_address: u64,
    pub hit_entity: Option<PlayerSupportEntity>,
    pub surface_name_address: u64,
    pub collision_surface_name_available: bool,
    pub surface_properties: i16,
    pub surface_flags: u16,
    pub surface: Option<PlayerSupportSurface>,
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
    #[serde(deserialize_with = "Option::deserialize")]
    pub ground_state: Option<PlayerSupportGroundState>,
    pub ground_handle: u32,
    pub ground_resolution: String,
    pub surface: Option<PlayerSupportSurface>,
    pub cached_surface_index: i32,
    pub cached_surface_data_address: u64,
    pub surface_is_cached: bool,
    pub flags: u32,
    pub buttons: i32,
    pub input: PlayerSupportInput,
    pub expected_buttons: i32,
    pub input_acknowledged: bool,
    pub health: i32,
    pub team: i32,
    pub move_type: u8,
    pub ducked: bool,
    pub probe: Option<PlayerSupportProbe>,
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

pub(crate) fn validate_probe(
    probe: &PlayerSupportProbe,
    distance: u16,
    origin: [f32; 3],
) -> io::Result<()> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Player support diagnostic sweep violates the native query contract",
        )
    };
    let status =
        PlayerSupportProbeStatus::from_native(probe.fraction, probe.start_solid, probe.all_solid);
    if probe.distance != distance
        || !(MIN_PLAYER_SUPPORT_PROBE_DISTANCE..=MAX_PLAYER_SUPPORT_PROBE_DISTANCE)
            .contains(&distance)
        || probe.start.map(f32::to_bits) != origin.map(f32::to_bits)
        || probe.requested_end[0].to_bits() != origin[0].to_bits()
        || probe.requested_end[1].to_bits() != origin[1].to_bits()
        || probe.requested_end[2] >= origin[2]
        || probe.mask != PLAYER_SUPPORT_MOVEMENT_MASK
        || probe.collision_group != PLAYER_SUPPORT_COLLISION_GROUP
        || !probe.fraction.is_finite()
        || !(0.0..=1.0).contains(&probe.fraction)
        || !probe.fraction_left_solid.is_finite()
        || !(0.0..=1.0).contains(&probe.fraction_left_solid)
        || probe.status != status
        || probe.plane.available != (status == PlayerSupportProbeStatus::Impact)
        || !probe.plane.distance.is_finite()
        || probe.surface.is_some() != (status == PlayerSupportProbeStatus::Impact)
        || probe.hit_entity.is_some() != (probe.hit_entity_address != 0)
        || (status == PlayerSupportProbeStatus::Impact && probe.hit_entity.is_none())
        || probe.collision_surface_name_available
    {
        return Err(invalid());
    }
    for vector in [
        probe.start,
        probe.requested_end,
        probe.native_start,
        probe.native_end,
        probe.plane.normal,
    ] {
        if vector.into_iter().any(|value| !value.is_finite()) {
            return Err(invalid());
        }
    }
    if [probe.start, probe.requested_end]
        .into_iter()
        .flatten()
        .any(|value| value.abs() > MAX_PLAYER_SUPPORT_PROBE_COORDINATE)
    {
        return Err(invalid());
    }
    let exact_end = f64::from(origin[2]) - f64::from(distance);
    if exact_end <= f64::from(probe.requested_end[2].next_down())
        || exact_end >= f64::from(probe.requested_end[2].next_up())
    {
        return Err(invalid());
    }
    if let Some(hit) = &probe.hit_entity
        && (hit.address != probe.hit_entity_address
            || hit.index >= 8192
            || hit.reference_handle == u32::MAX
            || hit.reference_handle != ((hit.serial << 16) | hit.index)
            || hit.classname.is_empty()
            || hit.classname.len() > 127
            || hit.classname.as_bytes().contains(&0))
    {
        return Err(invalid());
    }
    if let Some(surface) = &probe.surface
        && (!valid_surface(surface) || surface.index != i32::from(probe.surface_properties))
    {
        return Err(invalid());
    }
    Ok(())
}

fn valid_surface(surface: &PlayerSupportSurface) -> bool {
    (0..128).contains(&surface.index)
        && surface.data_address != 0
        && !surface.name.is_empty()
        && surface.name.len() <= 255
        && !surface.name.as_bytes().contains(&0)
        && surface.friction.is_finite()
        && surface.friction >= 0.0
}

pub fn validate_player_support_samples(
    samples: &[PlayerSupportSample],
    settings: &PlayerSupportSettings,
) -> io::Result<()> {
    settings.validate()?;
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Player support rows violate the completed sample contract",
        )
    };
    if samples.len() < 3 || samples.len() > settings.max_samples {
        return Err(invalid());
    }
    let first = &samples[0];
    let mut grounds = BTreeMap::<u32, (&PlayerSupportEntity, &PlayerSupportGroundState)>::new();
    let mut edicts = BTreeMap::<u64, u32>::new();
    let mut declarations = BTreeMap::<i32, u32>::new();
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
            || row.input != settings.input
            || row.expected_buttons != settings.input.expected_buttons()
            || row.buttons != row.expected_buttons
            || !row.input_acknowledged
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
        match (&row.ground, &row.ground_state) {
            (Some(ground), Some(state)) => {
                if ground.address == 0
                    || ground.index >= 8192
                    || ground.reference_handle != row.ground_handle
                    || matches!(ground.reference_handle, 0 | u32::MAX)
                    || ground.serial != ground.reference_handle >> 16
                    || ground.reference_handle != ((ground.serial << 16) | ground.index)
                    || state.edict() == 0
                    || row.surface.is_none()
                {
                    return Err(invalid());
                }
                if let Some((previous_entity, previous_state)) = grounds.get(&ground.index)
                    && (**previous_entity != *ground || !previous_state.same_identity(state))
                {
                    return Err(invalid());
                }
                if edicts
                    .insert(state.edict(), ground.index)
                    .is_some_and(|index| index != ground.index)
                {
                    return Err(invalid());
                }
                grounds.insert(ground.index, (ground, state));
                match state {
                    PlayerSupportGroundState::World { .. } => {
                        if ground.index != 0
                            || ground.classname != "worldspawn"
                            || row.ground_resolution != "serial_resolved_world"
                        {
                            return Err(invalid());
                        }
                    }
                    PlayerSupportGroundState::FuncMovelinear {
                        hammer_id,
                        model_name,
                        bsp_model_index,
                        move_parent_handle,
                        origin,
                        local_origin,
                        angles,
                        local_angles,
                        velocity,
                        absolute_velocity,
                        base_velocity,
                        collision_mins,
                        collision_maxs,
                        eflags,
                        ..
                    } => {
                        if declarations
                            .insert(*hammer_id, ground.index)
                            .is_some_and(|index| index != ground.index)
                        {
                            return Err(invalid());
                        }
                        if ground.index == 0
                            || ground.classname != "func_movelinear"
                            || row.ground_resolution != "serial_resolved_compiled_func_movelinear"
                            || *hammer_id <= 0
                            || *bsp_model_index == 0
                            || model_name != &format!("*{bsp_model_index}")
                            || !matches!(*move_parent_handle, 0 | u32::MAX)
                            || eflags & 0x1800 != 0
                            || (0..3).any(|axis| collision_mins[axis] >= collision_maxs[axis])
                            || [
                                origin,
                                local_origin,
                                angles,
                                local_angles,
                                velocity,
                                absolute_velocity,
                                base_velocity,
                                collision_mins,
                                collision_maxs,
                            ]
                            .into_iter()
                            .flatten()
                            .any(|value| !value.is_finite())
                        {
                            return Err(invalid());
                        }
                    }
                }
            }
            (None, None) => {
                let expected = match row.ground_handle {
                    0 => "null_handle",
                    u32::MAX => "invalid_handle_resolves_null",
                    _ => return Err(invalid()),
                };
                if row.ground_resolution != expected {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
        match (&row.probe, settings.probe_distance) {
            (Some(probe), Some(distance)) => validate_probe(probe, distance, row.origin)?,
            (None, None) => {}
            _ => return Err(invalid()),
        }
        if let Some(surface) = &row.surface {
            if !valid_surface(surface)
                || surface.index != row.cached_surface_index
                || surface.data_address != row.cached_surface_data_address
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
