use glam::DVec3;
use source_formats::vmf::parse_vec3;
use std::collections::BTreeMap;

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
