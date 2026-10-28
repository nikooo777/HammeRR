use glam::{DQuat, DVec3, EulerRot};
use source_formats::{fgd::Class, vmf::Entity};

use super::MAX_HELPER_EXTENT;

pub const MAX_VALUE_BYTES: usize = 256;
pub const MAX_PITCH_SCALES: usize = 16;
pub const ORIENTATION_POLICY: &str = "Authored Source Pitch Yaw Roll, Rz(yaw) * Ry(pitch) * Rx(roll); zero angles face +X with right -Y and up +Z. Positive pitch points down. FGD frustum() pitch_scale is retained but not evaluated.";
pub const EDGES: [[usize; 2]; 12] = [
    [0, 1],
    [1, 2],
    [2, 3],
    [3, 0],
    [4, 5],
    [5, 6],
    [6, 7],
    [7, 4],
    [0, 4],
    [1, 5],
    [2, 6],
    [3, 7],
];

#[derive(Debug, Clone)]
pub struct Input {
    pub property: &'static str,
    pub property_type: Option<String>,
    pub raw: Option<String>,
    pub raw_truncated: bool,
    pub value_source: &'static str,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PitchScale {
    pub argument_count: usize,
    pub declaration_state: &'static str,
    pub raw: Option<String>,
    pub raw_truncated: bool,
}

#[derive(Debug, Clone)]
pub struct Projector {
    pub inputs: Vec<Input>,
    pub helper_pitch_scales: Vec<PitchScale>,
    pub omitted_helper_declarations: usize,
    pub fov_degrees: Option<f64>,
    pub near_z: Option<f64>,
    pub far_z: Option<f64>,
    pub color: Option<[u8; 3]>,
    pub intensity: Option<f64>,
    pub origin: Option<[f64; 3]>,
    pub angles: Option<[f64; 3]>,
    pub corners: Option<[[f64; 3]; 8]>,
    pub guide_bounds: Option<[[f64; 3]; 2]>,
    pub state: &'static str,
    pub error: Option<String>,
}

fn bounded(value: &str) -> String {
    super::values::bounded(value, MAX_VALUE_BYTES)
}

fn input(class: &Class, entity: &Entity, key: &'static str, kind: &str) -> Input {
    let value = super::values::Value::resolve(class, entity, key, key != "origin");
    let (property, raw) = (value.property, value.raw);
    let error = value.validate(MAX_VALUE_BYTES, (key != "origin").then_some(&[kind][..])).err().map(|error| match error {
        super::values::Error::Duplicate => format!("Source projector {key} occurs more than once"),
        super::values::Error::Oversized => format!("Source projector {key} exceeds the {MAX_VALUE_BYTES}-byte value limit; oversized input was not parsed"),
        super::values::Error::Kind => format!("Source projector {key} requires a declared {kind} property"),
        super::values::Error::Invalid(error) => error,
    });
    Input {
        property: key,
        property_type: property.map(|property| bounded(&property.kind)),
        raw: raw.map(bounded),
        raw_truncated: raw.is_some_and(|raw| raw.len() > MAX_VALUE_BYTES),
        value_source: value.source,
        error,
    }
}

fn numbers(input: &Input, counts: &[usize]) -> Result<Vec<f64>, String> {
    let raw = input.raw.as_deref().ok_or_else(|| {
        format!(
            "Source projector {} has no authored value or FGD default",
            input.property
        )
    })?;
    let values = super::values::numbers(raw, counts).ok_or_else(|| {
        format!(
            "Source projector {} needs {} finite native numbers",
            input.property,
            counts
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(" or ")
        )
    })?;
    Ok(values)
}

pub(super) struct Geometry {
    pub corners: [[f64; 3]; 8],
    pub bounds: [[f64; 3]; 2],
}

pub(super) fn square_geometry(
    origin: DVec3,
    angles: DVec3,
    fov: f64,
    near: f64,
    far: f64,
) -> Result<Geometry, String> {
    let radians = angles.map(|angle| angle.rem_euclid(360.0).to_radians());
    let rotation = DQuat::from_euler(EulerRot::ZYX, radians.y, radians.x, radians.z);
    let forward = rotation * DVec3::X;
    let right = rotation * DVec3::NEG_Y;
    let up = rotation * DVec3::Z;
    let tangent = (fov.to_radians() * 0.5).tan();
    let signs = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    let corners = std::array::from_fn::<_, 8, _>(|index| {
        let distance = if index < 4 { near } else { far };
        let (horizontal, vertical) = signs[index % 4];
        origin + forward * distance + (right * horizontal + up * vertical) * (distance * tangent)
    });
    let native = corners.map(|point| point.as_vec3().as_dvec3());
    let coincident = native
        .iter()
        .enumerate()
        .any(|(index, point)| native[index + 1..].contains(point));
    let degenerate_plane = [0, 4].into_iter().any(|start| {
        let [a, b, c, d] = std::array::from_fn::<_, 4, _>(|index| native[start + index]);
        [(b - a).cross(c - a), (c - a).cross(d - a)]
            .into_iter()
            .any(|area| !area.is_finite() || area.length_squared() == 0.0)
    });
    if corners.iter().any(|point| {
        !point.is_finite()
            || !point.as_vec3().is_finite()
            || (*point - origin).abs().max_element() > MAX_HELPER_EXTENT
    }) || coincident
        || degenerate_plane
    {
        return Err(format!(
            "Source projector needs eight finite distinct corners and nonzero plane areas at native precision, with coordinate offsets within ±{MAX_HELPER_EXTENT} units of the origin"
        ));
    }
    Ok(Geometry {
        corners: corners.map(|point| point.to_array()),
        bounds: [
            corners
                .iter()
                .copied()
                .fold(DVec3::splat(f64::INFINITY), DVec3::min)
                .to_array(),
            corners
                .iter()
                .copied()
                .fold(DVec3::splat(f64::NEG_INFINITY), DVec3::max)
                .to_array(),
        ],
    })
}

pub fn resolve(class: &Class, entity: &Entity) -> Projector {
    let helpers = class
        .helpers
        .iter()
        .filter(|helper| helper.name == "frustum");
    let mut result = Projector {
        inputs: [
            ("origin", "origin"),
            ("angles", "angle"),
            ("lightfov", "float"),
            ("nearz", "float"),
            ("farz", "float"),
            ("lightcolor", "color255"),
            ("target", "target_destination"),
            ("parentname", "target_destination"),
            ("cameraspace", "integer"),
        ]
        .map(|(key, kind)| input(class, entity, key, kind))
        .into(),
        helper_pitch_scales: helpers
            .clone()
            .take(MAX_PITCH_SCALES)
            .map(|helper| {
                let raw = (helper.arguments.len() == 5).then(|| &helper.arguments[4]);
                PitchScale {
                    argument_count: helper.arguments.len(),
                    declaration_state: if raw.is_some() {
                        "declared_unevaluated"
                    } else {
                        "unsupported_helper_form"
                    },
                    raw: raw.map(|raw| bounded(raw)),
                    raw_truncated: raw.is_some_and(|raw| raw.len() > MAX_VALUE_BYTES),
                }
            })
            .collect(),
        omitted_helper_declarations: helpers.count().saturating_sub(MAX_PITCH_SCALES),
        fov_degrees: None,
        near_z: None,
        far_z: None,
        color: None,
        intensity: None,
        origin: None,
        angles: None,
        corners: None,
        guide_bounds: None,
        state: "invalid",
        error: None,
    };
    let check = |result: &mut Projector| -> Result<(), String> {
        if !entity
            .classname()
            .is_some_and(|name| name.eq_ignore_ascii_case("env_projectedtexture"))
        {
            result.state = "unsupported";
            return Err("Source projector guides support only env_projectedtexture".into());
        }
        if let Some(error) = result.inputs.iter().find_map(|input| input.error.clone()) {
            return Err(error);
        }
        if entity.properties.get("angle").is_some() {
            result.state = "unsupported";
            return Err(
                "Legacy angle is outside the Source projector guide's authored angles policy"
                    .into(),
            );
        }
        for input in &result.inputs[6..8] {
            if input.raw.as_deref().is_some_and(|raw| !raw.is_empty()) {
                result.state = "unsupported";
                return Err(format!(
                    "Source projector {} branches are not evaluated",
                    input.property
                ));
            }
        }
        if numbers(&result.inputs[8], &[1])?[0] != 0.0 {
            result.state = "unsupported";
            return Err("Source projector cameraspace branches are not evaluated".into());
        }
        let origin = DVec3::from_slice(&numbers(&result.inputs[0], &[3])?);
        let angles = DVec3::from_slice(&numbers(&result.inputs[1], &[3])?);
        let fov = numbers(&result.inputs[2], &[1])?[0];
        let near = numbers(&result.inputs[3], &[1])?[0];
        let far = numbers(&result.inputs[4], &[1])?[0];
        if !(0.0 < fov && fov < 180.0 && 0.0 < near && near < far && far <= MAX_HELPER_EXTENT) {
            return Err(format!(
                "Source projector requires 0 < FOV < 180 and 0 < nearz < farz <= {MAX_HELPER_EXTENT}; values are not clamped"
            ));
        }
        let color = numbers(&result.inputs[5], &[3, 4])?;
        if !color[..3]
            .iter()
            .all(|value| value.fract() == 0.0 && (0.0..=255.0).contains(value))
            || color.get(3).is_some_and(|value| *value < 0.0)
        {
            return Err("Source projector guide RGB must be integers 0–255 and optional intensity nonnegative; sentinel colors are not substituted".into());
        }
        let geometry = square_geometry(origin, angles, fov, near, far)?;
        result.origin = Some(origin.to_array());
        result.angles = Some(angles.to_array());
        result.fov_degrees = Some(fov);
        result.near_z = Some(near);
        result.far_z = Some(far);
        result.color = Some([color[0] as u8, color[1] as u8, color[2] as u8]);
        result.intensity = color.get(3).copied();
        result.guide_bounds = Some(geometry.bounds);
        result.corners = Some(geometry.corners);
        Ok(())
    };
    match check(&mut result) {
        Ok(()) => result.state = "ready",
        Err(error) => result.error = Some(error),
    }
    result
}

impl Projector {
    pub fn lines(&self) -> Vec<[[f64; 3]; 2]> {
        self.corners
            .map(|corners| EDGES.map(|[a, b]| [corners[a], corners[b]]).into())
            .unwrap_or_default()
    }
}
