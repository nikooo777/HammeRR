use source_formats::vmf::Entity;

use super::{Fog, number_property, property};

pub const MIN_FAR_Z: f32 = 4.0;
pub const MAX_FIELD_MAGNITUDE: f32 = 1_000_000.0;
pub const MAX_SKY_SCALE: u32 = 128;
const MAX_VALUE_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Profile {
    pub color: [f32; 3],
    pub color2: [f32; 3],
    pub direction: [f32; 3],
    pub start: f32,
    pub end: f32,
    pub max_density: f32,
    pub far_z: f32,
    pub lerp_time: f32,
    pub enabled: bool,
    pub blend: bool,
    pub radial: bool,
}

impl Profile {
    pub fn initial() -> Self {
        Self {
            color: [0.0; 3],
            color2: [0.0; 3],
            direction: [0.0; 3],
            start: 0.0,
            end: 0.0,
            max_density: 1.0,
            far_z: 0.0,
            lerp_time: 0.0,
            enabled: false,
            blend: false,
            radial: false,
        }
    }

    pub fn new(
        color: [f32; 3],
        start: f32,
        end: f32,
        max_density: f32,
        enabled: bool,
    ) -> Result<Self, String> {
        let profile = Self {
            color,
            start,
            end,
            max_density,
            enabled,
            ..Self::initial()
        };
        profile.validate()?;
        Ok(profile)
    }

    pub fn validate(&self) -> Result<(), String> {
        for (key, value) in ["fogstart", "fogend", "fogmaxdensity", "farz"]
            .into_iter()
            .zip([self.start, self.end, self.max_density, self.far_z])
        {
            bounded(value, key)?;
        }
        for value in self
            .color
            .into_iter()
            .chain(self.color2)
            .chain(self.direction)
        {
            bounded(value, "fog color or direction")?;
        }
        if self.far_z > 0.0 && self.far_z <= MIN_FAR_Z {
            return Err(format!(
                "Positive farz must exceed the preview near plane {MIN_FAR_Z}"
            ));
        }
        if !self.lerp_time.is_finite()
            || !(0.0..=super::MAX_TIMELINE_SECONDS as f32).contains(&self.lerp_time)
        {
            return Err(format!(
                "foglerptime must be between 0 and {} seconds",
                super::MAX_TIMELINE_SECONDS
            ));
        }
        self.range_parameters()?;
        Ok(())
    }

    pub fn range_parameters(&self) -> Result<[f32; 2], String> {
        range_parameters(self.start, self.end)
    }

    pub fn scaled_range(mut self, scale: f32) -> Result<Self, String> {
        if !scale.is_finite()
            || !(1.0..=MAX_SKY_SCALE as f32).contains(&scale)
            || scale.fract() != 0.0
        {
            return Err(format!(
                "Sky fog scale must be an integer from 1 to {MAX_SKY_SCALE}"
            ));
        }
        self.validate()?;
        let reciprocal = 1.0 / scale;
        self.start *= reciprocal;
        self.end *= reciprocal;
        self.validate()?;
        Ok(self)
    }

    pub fn color(self) -> [f32; 3] {
        self.color
    }
    pub fn color2(self) -> [f32; 3] {
        self.color2
    }
    pub fn direction(self) -> [f32; 3] {
        self.direction
    }
    pub fn received_direction(self) -> [f32; 3] {
        self.direction.map(received_coordinate)
    }
    pub fn start(self) -> f32 {
        self.start
    }
    pub fn end(self) -> f32 {
        self.end
    }
    pub fn max_density(self) -> f32 {
        self.max_density
    }
    pub fn far_z(self) -> f32 {
        self.far_z
    }
    pub fn enabled(self) -> bool {
        self.enabled
    }
    pub fn blend(self) -> bool {
        self.blend
    }
    pub fn radial(self) -> bool {
        self.radial
    }
}

pub fn received_coordinate(value: f32) -> f32 {
    let integer = value.abs().trunc() as u32;
    let fraction = ((value * 32.0).trunc().abs() as u32) & 0x1f;
    let received_integer = if integer > 0 {
        ((integer - 1) & 0x3fff) + 1
    } else {
        0
    };
    let received = received_integer as f32 + fraction as f32 / 32.0;
    if received == 0.0 {
        0.0
    } else if value < 0.0 {
        -received
    } else {
        received
    }
}

/// Returns the installed fog shader's [X, W] range parameters.
pub fn range_parameters(start: f32, end: f32) -> Result<[f32; 2], String> {
    bounded(start, "fogstart")?;
    bounded(end, "fogend")?;
    let result = if start == end {
        [end, 1.0]
    } else {
        let inverse = 1.0 / (end - start);
        [start * inverse, inverse]
    };
    if !result.into_iter().all(f32::is_finite) {
        return Err("Fog range cannot produce finite shader parameters".into());
    }
    Ok(result)
}

impl From<Fog> for Profile {
    fn from(fog: Fog) -> Self {
        Self {
            color: fog.color().map(f32::from),
            start: fog.start(),
            end: fog.end(),
            max_density: fog.max_density(),
            enabled: fog.enabled(),
            ..Self::initial()
        }
    }
}

#[derive(Clone, Debug)]
pub struct ParsedController {
    pub profile: Profile,
    pub defaults: Vec<&'static str>,
    pub inert_keys: Vec<String>,
    pub parent_chain: Vec<u32>,
}

pub(super) fn bounded(value: f32, key: &str) -> Result<f32, String> {
    if !value.is_finite() || value.abs() > MAX_FIELD_MAGNITUDE {
        return Err(format!(
            "{key} must be finite within ±{MAX_FIELD_MAGNITUDE}"
        ));
    }
    Ok(value)
}

pub(super) fn vector(raw: &str, key: &str) -> Result<[f32; 3], String> {
    if raw.len() > MAX_VALUE_BYTES {
        return Err(format!("{key} exceeds {MAX_VALUE_BYTES} bytes"));
    }
    let value = source_formats::vmf::parse_vec3(raw)
        .ok_or_else(|| format!("{key} needs three finite coordinates"))?;
    let values = value.as_vec3().to_array();
    for value in values {
        bounded(value, key)?;
    }
    Ok(values)
}

pub(super) fn angles_direction(angles: [f32; 3]) -> [f32; 3] {
    let pitch = angles[0].to_radians();
    let yaw = angles[1].to_radians();
    [
        -pitch.cos() * yaw.cos(),
        -pitch.cos() * yaw.sin(),
        pitch.sin(),
    ]
}

pub(super) fn color(raw: &str, key: &str) -> Result<[u8; 3], String> {
    if raw.len() > MAX_VALUE_BYTES {
        return Err(format!("{key} exceeds {MAX_VALUE_BYTES} bytes"));
    }
    let values = raw
        .split_whitespace()
        .map(str::parse::<u8>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| format!("{key} needs three RGB bytes and an optional alpha byte"))?;
    if values.len() != 3 && values.len() != 4 {
        return Err(format!(
            "{key} needs three RGB bytes and an optional alpha byte"
        ));
    }
    Ok([values[0], values[1], values[2]])
}

fn value<'a>(
    entity: &'a Entity,
    key: &'static str,
    defaults: &mut Vec<&'static str>,
) -> Result<Option<&'a str>, String> {
    let value = property(entity, key)?;
    if value.is_some_and(|raw| raw.len() > MAX_VALUE_BYTES) {
        return Err(format!("{key} exceeds {MAX_VALUE_BYTES} bytes"));
    }
    if value.is_none() {
        defaults.push(key);
    }
    Ok(value)
}

fn flag(
    entity: &Entity,
    key: &'static str,
    defaults: &mut Vec<&'static str>,
) -> Result<bool, String> {
    value(entity, key, defaults)?.map_or(Ok(false), |raw| {
        raw.parse::<i32>()
            .map(|value| value != 0)
            .map_err(|_| format!("{key} must be an integer boolean"))
    })
}

fn number(
    entity: &Entity,
    key: &'static str,
    default: f32,
    defaults: &mut Vec<&'static str>,
) -> Result<f32, String> {
    value(entity, key, defaults)?;
    bounded(number_property(entity, key, Some(default))?, key)
}

pub fn parse_controller(entity: &Entity) -> Result<ParsedController, String> {
    parse(
        entity,
        Kind::Controller {
            fixed_parents: false,
        },
    )
}

pub(super) fn parse_fixed_parent_controller(entity: &Entity) -> Result<ParsedController, String> {
    parse(
        entity,
        Kind::Controller {
            fixed_parents: true,
        },
    )
}

pub fn parse_sky_camera(entity: &Entity) -> Result<ParsedController, String> {
    validate_sky_camera_binding(entity)?;
    parse(entity, Kind::SkyCamera)
}

pub fn validate_sky_camera_binding(entity: &Entity) -> Result<(), String> {
    if property(entity, "parentname")?.is_some_and(|value| !value.is_empty()) {
        return Err(
            "Parented sky_camera is unavailable: native sky origin uses the parent-local frame"
                .into(),
        );
    }
    if property(entity, "parentattachment")?.is_some_and(|value| !value.is_empty() && value != "0")
    {
        return Err("Sky camera parent attachments are unavailable".into());
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Kind {
    Controller { fixed_parents: bool },
    SkyCamera,
}

impl Kind {
    fn classname(self) -> &'static str {
        match self {
            Self::Controller { .. } => "env_fog_controller",
            Self::SkyCamera => "sky_camera",
        }
    }
}

fn parse(entity: &Entity, kind: Kind) -> Result<ParsedController, String> {
    if entity.hidden {
        return Err(match kind {
            Kind::Controller { .. } => {
                "Authored-hidden fog controller is unavailable; show it before previewing".into()
            }
            Kind::SkyCamera => {
                "Authored-hidden sky camera is unavailable; show it before previewing".into()
            }
        });
    }
    if !entity.solids.is_empty() {
        return Err(format!("{} must be a point entity", kind.classname()));
    }
    if !property(entity, "classname")?
        .is_some_and(|class| class.eq_ignore_ascii_case(kind.classname()))
    {
        return Err(match kind {
            Kind::Controller { .. } => "Selected entity is not an env_fog_controller".into(),
            Kind::SkyCamera => "Selected entity is not a sky_camera".into(),
        });
    }
    let mut defaults = Vec::new();
    let mut profile = Profile::initial();
    profile.enabled = flag(entity, "fogenable", &mut defaults)?;
    profile.blend = flag(entity, "fogblend", &mut defaults)?;
    profile.radial = flag(entity, "fogradial", &mut defaults)?;
    profile.start = number(entity, "fogstart", 0.0, &mut defaults)?;
    profile.end = number(entity, "fogend", 0.0, &mut defaults)?;
    profile.max_density = number(entity, "fogmaxdensity", 1.0, &mut defaults)?;
    if matches!(kind, Kind::Controller { .. }) {
        profile.far_z = number(entity, "farz", 0.0, &mut defaults)?;
        profile.lerp_time = number(entity, "foglerptime", 0.0, &mut defaults)?;
    } else {
        value(entity, "scale", &mut defaults)?;
    }
    if let Some(raw) = value(entity, "fogcolor", &mut defaults)? {
        profile.color = color(raw, "fogcolor")?.map(f32::from);
    }
    if let Some(raw) = value(entity, "fogcolor2", &mut defaults)? {
        profile.color2 = color(raw, "fogcolor2")?.map(f32::from);
    }
    if let Some(raw) = value(entity, "fogdir", &mut defaults)? {
        profile.direction = vector(raw, "fogdir")?;
    }
    if flag(entity, "use_angles", &mut defaults)? {
        if !matches!(
            kind,
            Kind::Controller {
                fixed_parents: true
            }
        ) && property(entity, "parentname")?.is_some_and(|parent| !parent.is_empty())
        {
            return Err(
                "use_angles on a parented fog controller needs a resolved world orientation".into(),
            );
        }
        let angles = property(entity, "angles")?;
        let angle = property(entity, "angle")?;
        if angles
            .into_iter()
            .chain(angle)
            .any(|raw| raw.len() > MAX_VALUE_BYTES)
        {
            return Err(format!("Fog orientation exceeds {MAX_VALUE_BYTES} bytes"));
        }
        if angles.is_none() && angle.is_none() {
            defaults.push("angles");
        }
        if angles.is_some() && number_property(entity, "angle", Some(0.0))? != 0.0 {
            return Err("Fog orientation has conflicting angles and angle fields".into());
        }
        let orientation = crate::query::point_entity_angles(entity)
            .ok_or("Fog controller needs finite angles")?
            .as_vec3();
        for component in orientation.to_array() {
            bounded(component, "angles")?;
        }
        profile.direction = angles_direction(orientation.to_array());
    }
    profile.validate()?;
    let inert_candidates: &[&str] = if matches!(kind, Kind::SkyCamera) {
        &["farz", "foglerptime", "HDRColorScale", "ZoomFogScale"]
    } else {
        &["HDRColorScale", "ZoomFogScale"]
    };
    let inert_keys = inert_candidates
        .iter()
        .copied()
        .filter(|key| {
            entity
                .properties
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(key))
        })
        .map(str::to_owned)
        .collect();
    Ok(ParsedController {
        profile,
        defaults,
        inert_keys,
        parent_chain: Vec::new(),
    })
}
