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
}

pub fn value_kind(classname: &str, key: &str) -> Option<ValueKind> {
    if !matches!(classname, "light" | "light_spot" | "light_environment") {
        return None;
    }
    if key.eq_ignore_ascii_case("_light")
        || key.eq_ignore_ascii_case("_lightHDR")
        || (classname == "light_environment"
            && (key.eq_ignore_ascii_case("_ambient") || key.eq_ignore_ascii_case("_ambientHDR")))
    {
        Some(ValueKind::Color)
    } else if key.eq_ignore_ascii_case("_lightscaleHDR")
        || (classname == "light_environment" && key.eq_ignore_ascii_case("_AmbientScaleHDR"))
    {
        Some(ValueKind::HdrGain)
    } else if classname != "light_environment"
        && (key.eq_ignore_ascii_case("_fifty_percent_distance")
            || key.eq_ignore_ascii_case("_zero_percent_distance"))
    {
        Some(ValueKind::FalloffDistance)
    } else {
        None
    }
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
    if let Some(target) = target {
        return (target - origin)
            .try_normalize()
            .filter(|point| point.is_finite())
            .ok_or_else(|| "Light target must be finite and distinct from its origin".into());
    }
    let angles = angles.map_or(Ok(DVec3::ZERO), |raw| {
        parse_vec3(raw)
            .filter(|point| point.is_finite())
            .ok_or_else(|| "Light angles must contain finite Pitch Yaw Roll".to_string())
    })?;
    let angle = scalar(angle, "angle")?;
    if angle == -1.0 {
        return Ok(DVec3::Z);
    }
    if angle == -2.0 {
        return Ok(DVec3::NEG_Z);
    }
    let yaw = if angle == 0.0 { angles.y } else { angle }
        .rem_euclid(360.0)
        .to_radians();
    let pitch = scalar(pitch, "pitch")?;
    let pitch = if pitch == 0.0 { angles.x } else { pitch }
        .rem_euclid(360.0)
        .to_radians();
    Ok(DVec3::new(
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        pitch.sin(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_parameters_require_whole_ascii_names_and_supported_color_fields() {
        for name in ["lamp", "Lamp_2", "_", &"a".repeat(64)] {
            let value = format!("${name}");
            assert_eq!(parameter_name(&value), Some(name));
            for class in ["light", "light_spot", "light_environment"] {
                for key in ["_light", "_lightHDR", "_LIGHTHDR"] {
                    assert!(color_field(class, key));
                    assert_eq!(color_parameter(class, key, &value), Some(name));
                }
            }
            for key in ["_ambient", "_ambientHDR", "_AMBIENTHDR"] {
                assert!(color_field("light_environment", key));
                assert_eq!(
                    color_parameter("light_environment", key, &value),
                    Some(name)
                );
            }
        }
        for value in [
            "lamp",
            "$",
            "$1lamp",
            "$é",
            " $lamp",
            "$lamp ",
            "$lamp suffix",
            "prefix$lamp",
            "$lamp\n",
            "$lamp-x",
            &format!("${}", "a".repeat(65)),
        ] {
            assert_eq!(parameter_name(value), None, "{value:?}");
        }
        for (class, key) in [
            ("light_dynamic", "_light"),
            ("prop_static", "_lightHDR"),
            ("light", "_lightscaleHDR"),
            ("light_spot", "targetname"),
            ("light", "_ambient"),
            ("light_spot", "_ambientHDR"),
            ("light_environment", "_AmbientScaleHDR"),
            ("light_environment", "SunSpreadAngle"),
        ] {
            assert!(!color_field(class, key));
            assert_eq!(color_parameter(class, key, "$lamp"), None);
        }
    }

    #[test]
    fn hdr_gain_parameters_share_token_grammar_without_widening_color_or_other_scalar_fields() {
        for (class, key) in [
            ("light", "_lightscaleHDR"),
            ("light_spot", "_LIGHTSCALEHDR"),
            ("light_environment", "_lightscaleHDR"),
            ("light_environment", "_AmbientScaleHDR"),
        ] {
            assert_eq!(value_kind(class, key), Some(ValueKind::HdrGain));
            assert_eq!(value_parameter(class, key, "$Gain_1"), Some("Gain_1"));
            assert_eq!(value_parameter(class, key, "$Gain suffix"), None);
            assert_eq!(color_parameter(class, key, "$Gain_1"), None);
        }
        for (class, key) in [
            ("light", "_AmbientScaleHDR"),
            ("light_spot", "_AmbientScaleHDR"),
            ("light_environment", "SunSpreadAngle"),
            ("light_environment", "pitch"),
            ("light", "_quadratic_attn"),
            ("light_dynamic", "_lightscaleHDR"),
        ] {
            assert_eq!(value_kind(class, key), None);
            assert_eq!(value_parameter(class, key, "$Gain"), None);
        }
    }

    #[test]
    fn falloff_distance_parameters_are_limited_to_point_and_spot_fields() {
        for class in ["light", "light_spot"] {
            for key in ["_fifty_percent_distance", "_ZERO_PERCENT_DISTANCE"] {
                assert_eq!(value_kind(class, key), Some(ValueKind::FalloffDistance));
                assert_eq!(
                    value_parameter(class, key, "$Distance_1"),
                    Some("Distance_1")
                );
                assert_eq!(value_parameter(class, key, "$Distance suffix"), None);
                assert_eq!(color_parameter(class, key, "$Distance_1"), None);
            }
        }
        for (class, key) in [
            ("light_environment", "_fifty_percent_distance"),
            ("light_environment", "_zero_percent_distance"),
            ("light_dynamic", "_fifty_percent_distance"),
            ("light", "_distance"),
            ("light", "_quadratic_attn"),
            ("light_spot", "_cone"),
        ] {
            assert_eq!(value_kind(class, key), None);
        }
    }

    #[test]
    fn light_directions_share_pitch_yaw_legacy_angles_and_target_precedence() {
        for (angles, angle, pitch, expected) in [
            ("0 90 0", "0", "0", DVec3::Y),
            (
                "30 0 0",
                "0",
                "0",
                DVec3::new(30_f64.to_radians().cos(), 0.0, 0.5),
            ),
            ("0 0 0", "0", "-90", DVec3::NEG_Z),
            ("0 0 0", "-1", "0", DVec3::Z),
            ("0 0 0", "-2", "0", DVec3::NEG_Z),
        ] {
            assert!(
                direction(Some(angles), Some(angle), Some(pitch), DVec3::ZERO, None)
                    .unwrap()
                    .distance(expected)
                    < 1e-10
            );
        }
        assert_eq!(
            direction(Some("invalid"), None, None, DVec3::ZERO, Some(DVec3::Y)).unwrap(),
            DVec3::Y
        );
        assert!(direction(None, None, None, DVec3::ZERO, Some(DVec3::ZERO)).is_err());
        assert!(direction(Some("0 NaN 0"), None, None, DVec3::ZERO, None).is_err());
        assert!(direction(None, None, Some("inf"), DVec3::ZERO, None).is_err());
    }
}
