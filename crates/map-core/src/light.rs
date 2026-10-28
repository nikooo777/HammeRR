use glam::DVec3;
use source_formats::vmf::parse_vec3;
use std::collections::BTreeMap;

const MAX_PARAMETER_NAME_BYTES: usize = 64;

pub fn parameter_name(value: &str) -> Option<&str> {
    let name = value.strip_prefix('$')?;
    let bytes = name.as_bytes();
    if bytes.is_empty()
        || bytes.len() > MAX_PARAMETER_NAME_BYTES
        || !(bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        return None;
    }
    Some(name)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    Color,
    HdrGain,
    FalloffDistance,
    FalloffRadius,
    HardFalloff,
    AttenuationCoefficient,
    SunSpreadAngle,
    SpotlightAngle,
    SpotlightFocus,
    SpotlightPitch,
}

const ALL_VALUE_CLASSES: &[&str] = &["light", "light_spot", "light_environment"];
const POINT_VALUE_CLASSES: &[&str] = &["light", "light_spot"];
const SUN_VALUE_CLASSES: &[&str] = &["light_environment"];
const SPOT_VALUE_CLASSES: &[&str] = &["light_spot"];
static VALUE_FIELDS: [(&str, &[&str], ValueKind); 18] = [
    ("_light", ALL_VALUE_CLASSES, ValueKind::Color),
    ("_lightHDR", ALL_VALUE_CLASSES, ValueKind::Color),
    ("_ambient", SUN_VALUE_CLASSES, ValueKind::Color),
    ("_ambientHDR", SUN_VALUE_CLASSES, ValueKind::Color),
    ("_lightscaleHDR", ALL_VALUE_CLASSES, ValueKind::HdrGain),
    ("_AmbientScaleHDR", SUN_VALUE_CLASSES, ValueKind::HdrGain),
    (
        "_fifty_percent_distance",
        POINT_VALUE_CLASSES,
        ValueKind::FalloffDistance,
    ),
    (
        "_zero_percent_distance",
        POINT_VALUE_CLASSES,
        ValueKind::FalloffDistance,
    ),
    (
        "SunSpreadAngle",
        SUN_VALUE_CLASSES,
        ValueKind::SunSpreadAngle,
    ),
    ("_inner_cone", SPOT_VALUE_CLASSES, ValueKind::SpotlightAngle),
    ("_cone", SPOT_VALUE_CLASSES, ValueKind::SpotlightAngle),
    ("_exponent", SPOT_VALUE_CLASSES, ValueKind::SpotlightFocus),
    ("pitch", SPOT_VALUE_CLASSES, ValueKind::SpotlightPitch),
    (
        "_constant_attn",
        POINT_VALUE_CLASSES,
        ValueKind::AttenuationCoefficient,
    ),
    (
        "_linear_attn",
        POINT_VALUE_CLASSES,
        ValueKind::AttenuationCoefficient,
    ),
    (
        "_quadratic_attn",
        POINT_VALUE_CLASSES,
        ValueKind::AttenuationCoefficient,
    ),
    ("_distance", POINT_VALUE_CLASSES, ValueKind::FalloffRadius),
    ("_hardfalloff", POINT_VALUE_CLASSES, ValueKind::HardFalloff),
];

pub fn value_fields() -> impl Iterator<Item = (&'static str, &'static [&'static str], ValueKind)> {
    VALUE_FIELDS.iter().copied()
}

pub fn value_kind(classname: &str, key: &str) -> Option<ValueKind> {
    value_fields().find_map(|(name, classes, kind)| {
        (classes.contains(&classname) && key.eq_ignore_ascii_case(name)).then_some(kind)
    })
}

pub fn value_parameter<'a>(classname: &str, key: &str, value: &'a str) -> Option<&'a str> {
    value_kind(classname, key)?;
    parameter_name(value)
}

pub fn color_field(classname: &str, key: &str) -> bool {
    value_kind(classname, key) == Some(ValueKind::Color)
}

pub fn color_parameter<'a>(classname: &str, key: &str, value: &'a str) -> Option<&'a str> {
    if !color_field(classname, key) {
        return None;
    }
    parameter_name(value)
}

pub type Targets<'a> = BTreeMap<&'a str, Vec<DVec3>>;

pub fn target(name: Option<&str>, targets: &Targets<'_>) -> Result<Option<DVec3>, String> {
    let Some(name) = name.filter(|name| !name.is_empty()) else {
        return Ok(None);
    };
    let matches = targets
        .get(name)
        .ok_or("Spotlight target must name one visible entity with an origin")?;
    if matches.len() != 1 {
        return Err("Spotlight target resolves to multiple entities".into());
    }
    Ok(Some(matches[0]))
}

fn scalar(value: Option<&str>, name: &str) -> Result<f64, String> {
    let value = value
        .unwrap_or("0")
        .parse::<f64>()
        .map_err(|_| format!("Invalid light {name}"))?;
    if !value.is_finite() {
        return Err(format!("Light {name} must be finite"));
    }
    Ok(value)
}

pub fn direction(
    angles: Option<&str>,
    angle: Option<&str>,
    pitch: Option<&str>,
    origin: DVec3,
    target: Option<DVec3>,
) -> Result<DVec3, String> {
    read_direction(angles, angle, pitch, origin, target, false)?
        .ok_or_else(|| "Light pitch is unresolved".into())
}

pub fn template_direction(
    angles: Option<&str>,
    angle: Option<&str>,
    pitch: Option<&str>,
    origin: DVec3,
    target: Option<DVec3>,
) -> Result<Option<DVec3>, String> {
    read_direction(angles, angle, pitch, origin, target, true)
}

fn read_direction(
    angles: Option<&str>,
    angle: Option<&str>,
    pitch: Option<&str>,
    origin: DVec3,
    target: Option<DVec3>,
    defer_pitch: bool,
) -> Result<Option<DVec3>, String> {
    if let Some(target) = target {
        return (target - origin)
            .try_normalize()
            .filter(|point| point.is_finite())
            .map(Some)
            .ok_or_else(|| "Light target must be finite and distinct from its origin".into());
    }
    let angles = angles.map_or(Ok(DVec3::ZERO), |raw| {
        parse_vec3(raw)
            .filter(|point| point.is_finite())
            .ok_or_else(|| "Light angles must contain finite Pitch Yaw Roll".to_string())
    })?;
    let angle = scalar(angle, "angle")?;
    if angle == -1.0 {
        return Ok(Some(DVec3::Z));
    }
    if angle == -2.0 {
        return Ok(Some(DVec3::NEG_Z));
    }
    let yaw = if angle == 0.0 { angles.y } else { angle }
        .rem_euclid(360.0)
        .to_radians();
    if defer_pitch && pitch.and_then(parameter_name).is_some() {
        return Ok(None);
    }
    let pitch = scalar(pitch, "pitch")?;
    let pitch = if pitch == 0.0 { angles.x } else { pitch }
        .rem_euclid(360.0)
        .to_radians();
    Ok(Some(DVec3::new(
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        pitch.sin(),
    )))
}
