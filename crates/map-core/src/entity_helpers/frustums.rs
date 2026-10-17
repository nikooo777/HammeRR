use glam::DVec3;
use source_formats::{
    fgd::{Class, Helper},
    vmf::Entity,
};

use super::{MAX_HELPER_EXTENT, projectors};

pub const MAX_DECLARATIONS: usize = 16;
pub const MAX_BINDING_BYTES: usize = 79;
pub const MAX_VALUE_BYTES: usize = 256;
pub const ORIENTATION_POLICY: &str = "Hammer2009 FGD authoring guide: T(origin) * Rz(yaw) * Ry(-pitch_scale * raw_pitch) * Rx(roll); multiply pitch before angle reduction. Zero angles face +X. This is not an engine projection or runtime-state preview.";

#[derive(Debug, Clone)]
pub struct Input {
    pub property: String,
    pub property_type: Option<String>,
    pub raw: Option<String>,
    pub raw_truncated: bool,
    pub value_source: &'static str,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Frustum {
    pub arguments: Vec<String>,
    pub arguments_truncated: bool,
    pub inputs: Vec<Input>,
    pub pitch_scale: Option<f64>,
    pub pitch_scale_source: &'static str,
    pub fov_degrees: Option<f64>,
    pub near_z: Option<f64>,
    pub far_z: Option<f64>,
    pub color: Option<[u8; 3]>,
    pub intensity: Option<f64>,
    pub origin: Option<[f64; 3]>,
    pub angles: Option<[f64; 3]>,
    pub effective_angles: Option<[f64; 3]>,
    pub corners: Option<[[f64; 3]; 8]>,
    pub guide_bounds: Option<[[f64; 3]; 2]>,
    pub state: &'static str,
    pub error: Option<String>,
}

fn bounded(value: &str) -> String {
    super::values::bounded(value, MAX_VALUE_BYTES)
}

fn input(class: &Class, entity: &Entity, key: &str, kinds: &[&str]) -> Input {
    let value = super::values::Value::resolve(class, entity, key, key != "origin");
    let (property, raw) = (value.property, value.raw);
    let error = value
        .validate(MAX_VALUE_BYTES, (key != "origin").then_some(kinds))
        .err()
        .map(|error| match error {
            super::values::Error::Duplicate => format!("FGD frustum {key} occurs more than once"),
            super::values::Error::Oversized => {
                format!("FGD frustum {key} exceeds the {MAX_VALUE_BYTES}-byte input limit")
            }
            super::values::Error::Kind => format!(
                "FGD frustum {key} requires a declared {} property",
                kinds.join(" or ")
            ),
            super::values::Error::Invalid(error) => format!("FGD frustum {key}: {error}"),
        });
    Input {
        property: key.to_owned(),
        property_type: property.map(|p| bounded(&p.kind)),
        raw: raw.map(bounded),
        raw_truncated: raw.is_some_and(|raw| raw.len() > MAX_VALUE_BYTES),
        value_source: value.source,
        error,
    }
}

fn numbers(input: &Input, counts: &[usize]) -> Result<Vec<f64>, String> {
    input.raw.as_deref().and_then(|raw| super::values::numbers(raw, counts)).ok_or_else(|| {
        format!("FGD frustum {} requires {} finite native numbers from an authored value or FGD default", input.property, counts.iter().map(usize::to_string).collect::<Vec<_>>().join(" or "))
    })
}

pub fn resolve(class: &Class, entity: &Entity, helper: &Helper) -> Frustum {
    let mut result = Frustum {
        arguments: helper
            .arguments
            .iter()
            .take(5)
            .map(|v| bounded(v))
            .collect(),
        arguments_truncated: helper.arguments.len() > 5
            || helper.arguments.iter().any(|v| v.len() > MAX_VALUE_BYTES),
        inputs: Vec::new(),
        pitch_scale: None,
        pitch_scale_source: if helper.arguments.len() == 5 {
            "helper_argument"
        } else {
            "helper_default"
        },
        fov_degrees: None,
        near_z: None,
        far_z: None,
        color: None,
        intensity: None,
        origin: None,
        angles: None,
        effective_angles: None,
        corners: None,
        guide_bounds: None,
        state: "invalid",
        error: None,
    };
    let check = |result: &mut Frustum| -> Result<(), String> {
        if helper.arguments.len() > 5 {
            result.state = "unsupported";
            return Err("FGD frustum supports zero to five Hammer2009 arguments; literal-value/RGB extensions are not evaluated".into());
        }
        let mut bindings = ["_fov", "_NearPlane", "_FarPlane", "_light"];
        for (index, argument) in helper.arguments.iter().take(4).enumerate() {
            bindings[index] = argument;
        }
        for (index, key) in bindings.iter().enumerate() {
            if key.is_empty() || key.len() > MAX_BINDING_BYTES {
                return Err(format!(
                    "FGD frustum binding names require 1–{MAX_BINDING_BYTES} UTF-8 bytes; no native truncation is applied"
                ));
            }
            if ["origin", "angles"]
                .iter()
                .any(|reserved| key.eq_ignore_ascii_case(reserved))
                || bindings[..index]
                    .iter()
                    .any(|other| key.eq_ignore_ascii_case(other))
            {
                result.state = "unsupported";
                return Err("FGD frustum binding aliases and reserved origin/angles bindings are outside the supported subset".into());
            }
        }
        let scale = if let Some(raw) = helper.arguments.get(4) {
            if raw.len() > MAX_VALUE_BYTES {
                return Err("FGD frustum pitch_scale exceeds the input limit".into());
            }
            raw.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && (*v as f32).is_finite())
                .ok_or_else(|| {
                    "FGD frustum pitch_scale requires one finite native literal number".to_owned()
                })?
        } else {
            1.0
        };
        result.pitch_scale = Some(scale);
        for (key, kinds) in [
            ("origin", &["origin"][..]),
            ("angles", &["angle"][..]),
            (bindings[0], &["float", "integer"][..]),
            (bindings[1], &["float", "integer"][..]),
            (bindings[2], &["float", "integer"][..]),
            (bindings[3], &["color255"][..]),
        ] {
            result.inputs.push(input(class, entity, key, kinds));
        }
        if let Some(error) = result.inputs.iter().find_map(|input| input.error.clone()) {
            return Err(error);
        }
        if entity.properties.get("angle").is_some() {
            result.state = "unsupported";
            return Err("FGD frustum legacy angle is outside the authored angles policy".into());
        }
        let origin = DVec3::from_slice(&numbers(&result.inputs[0], &[3])?);
        let angles = DVec3::from_slice(&numbers(&result.inputs[1], &[3])?);
        let pitch = -scale * angles.x;
        if !pitch.is_finite() || !(pitch as f32).is_finite() {
            return Err(
                "FGD frustum pitch_scale × raw pitch must remain finite at native precision".into(),
            );
        }
        let effective = DVec3::new(pitch, angles.y, angles.z);
        let fov = numbers(&result.inputs[2], &[1])?[0];
        let near = numbers(&result.inputs[3], &[1])?[0];
        let far = numbers(&result.inputs[4], &[1])?[0];
        if !(0.02..=178.0).contains(&fov) || !(0.0 < near && near < far && far <= MAX_HELPER_EXTENT)
        {
            return Err(format!(
                "FGD frustum requires nominal FOV 0.02–178 degrees and 0 < near < far <= {MAX_HELPER_EXTENT}; native clamp branches are not substituted"
            ));
        }
        let color = numbers(&result.inputs[5], &[3, 4])?;
        if !color[..3]
            .iter()
            .all(|v| v.fract() == 0.0 && (0.0..=255.0).contains(v))
            || color.get(3).is_some_and(|v| *v < 0.0)
        {
            return Err("FGD frustum RGB requires integers 0–255 and optional nonnegative intensity; intensity is not guide opacity".into());
        }
        let geometry = projectors::square_geometry(origin, effective, fov, near, far)
            .map_err(|error| error.replace("Source projector", "FGD frustum"))?;
        result.origin = Some(origin.to_array());
        result.angles = Some(angles.to_array());
        result.effective_angles = Some(effective.to_array());
        result.fov_degrees = Some(fov);
        result.near_z = Some(near);
        result.far_z = Some(far);
        result.color = Some([color[0] as u8, color[1] as u8, color[2] as u8]);
        result.intensity = color.get(3).copied();
        result.corners = Some(geometry.corners);
        result.guide_bounds = Some(geometry.bounds);
        Ok(())
    };
    match check(&mut result) {
        Ok(()) => result.state = "ready",
        Err(error) => result.error = Some(error),
    }
    result
}

impl Frustum {
    pub fn lines(&self) -> Vec<[[f64; 3]; 2]> {
        self.corners
            .map(|corners| {
                projectors::EDGES
                    .map(|[a, b]| [corners[a], corners[b]])
                    .into()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
