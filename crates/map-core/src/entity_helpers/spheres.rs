use super::values::bounded;
use std::collections::BTreeSet;

use glam::DVec3;
use source_formats::{
    fgd::{Class, Helper},
    vmf::Entity,
};

use super::MAX_HELPER_EXTENT;

pub const MAX_SPHERES_PER_ENTITY: usize = 16;
pub const SPHERE_SEGMENTS: usize = 32;
pub const MAX_SPHERE_KEY_BYTES: usize = 128;
pub const MAX_SPHERE_VALUE_BYTES: usize = 256;
pub const MAX_SPHERE_ERROR_BYTES: usize = 512;

#[derive(Debug, Clone)]
pub struct Sphere {
    pub property: String,
    pub raw: Option<String>,
    pub raw_truncated: bool,
    pub property_truncated: bool,
    pub implicit_property: bool,
    pub value_source: &'static str,
    pub property_type: Option<String>,
    pub radius: Option<f64>,
    pub center: Option<[f64; 3]>,
    pub guide_bounds: Option<[[f64; 3]; 2]>,
    pub state: &'static str,
    pub error: Option<String>,
}

pub(super) fn supported(class: &Class, helper: &Helper) -> bool {
    helper.name == "sphere"
        && property_key(helper).is_some_and(|key| {
            key.len() <= MAX_SPHERE_KEY_BYTES
                && class.property(key).is_some_and(|property| {
                    matches!(property.kind.as_str(), "float" | "integer" | "string")
                })
        })
}

fn property_key(helper: &Helper) -> Option<&str> {
    match helper.arguments.as_slice() {
        [] => Some("radius"),
        [key] => Some(key),
        _ => None,
    }
}

pub fn resolve(class: &Class, entity: &Entity) -> Vec<Sphere> {
    let mut result = Vec::new();
    let mut keys = BTreeSet::new();
    for helper in class
        .helpers
        .iter()
        .filter(|helper| helper.name == "sphere")
    {
        if property_key(helper).is_some_and(|key| {
            key.len() <= MAX_SPHERE_KEY_BYTES && !keys.insert(key.to_ascii_lowercase())
        }) {
            continue;
        }
        if result.len() == MAX_SPHERES_PER_ENTITY {
            result.push(Sphere {
                property: String::new(), raw: None, raw_truncated: false, property_truncated: false, implicit_property: false, value_source: "missing", property_type: None,
                radius: None, center: None, guide_bounds: None, state: "unsupported",
                error: Some(format!("Sphere guides support at most {MAX_SPHERES_PER_ENTITY} declarations per entity; remaining declarations were omitted")),
            });
            break;
        }
        result.push(resolve_one(class, entity, helper));
    }
    result
}

fn resolve_one(class: &Class, entity: &Entity, helper: &Helper) -> Sphere {
    let mut result = Sphere {
        property: property_key(helper)
            .map(|key| bounded(key, MAX_SPHERE_KEY_BYTES))
            .unwrap_or_default(),
        property_truncated: helper
            .arguments
            .first()
            .is_some_and(|key| key.len() > MAX_SPHERE_KEY_BYTES),
        implicit_property: helper.arguments.is_empty(),
        raw_truncated: false,
        raw: None,
        value_source: "missing",
        property_type: None,
        radius: None,
        center: entity
            .origin()
            .filter(|origin| origin.is_finite())
            .map(|point| point.to_array()),
        guide_bounds: None,
        state: "unsupported",
        error: None,
    };
    if !supported(class, helper) {
        result.error = Some("sphere() requires a declared float, integer or numeric-string radius property; sphere(property) names a declared numeric property. Other forms are unavailable".into());
        return result;
    }
    let property = class.property(&result.property).unwrap();
    result.property = property.name.clone();
    result.property_type = Some(property.kind.clone());
    let value = super::values::Value::resolve(class, entity, &property.name, true);
    let raw = value.raw;
    result.raw = raw.map(|raw| bounded(raw, MAX_SPHERE_VALUE_BYTES));
    result.raw_truncated = raw.is_some_and(|raw| raw.len() > MAX_SPHERE_VALUE_BYTES);
    result.value_source = value.source;
    let duplicate = value.duplicate;
    let check = || -> Result<(f64, [[f64; 3]; 2]), String> {
        if duplicate {
            return Err(format!(
                "Sphere radius property {} occurs more than once",
                property.name
            ));
        }
        if result.raw_truncated {
            return Err(format!(
                "Sphere radius values support at most {MAX_SPHERE_VALUE_BYTES} bytes; oversized input was not parsed"
            ));
        }
        let raw = raw.ok_or_else(|| {
            format!(
                "Sphere radius property {} has no authored value or FGD default",
                property.name
            )
        })?;
        property.validate(raw.trim())?;
        let radius = super::values::numbers(raw, &[1])
            .map(|values| values[0])
            .ok_or("Sphere radius must be one finite native number")?;
        if radius.abs() > MAX_HELPER_EXTENT {
            return Err(format!(
                "Sphere radius must stay within ±{MAX_HELPER_EXTENT} editor units"
            ));
        }
        let center = result
            .center
            .map(DVec3::from_array)
            .ok_or("Sphere guide needs a finite authored origin")?;
        let bounds = [center - DVec3::splat(radius), center + DVec3::splat(radius)];
        if bounds
            .iter()
            .any(|point| !point.is_finite() || !point.as_vec3().is_finite())
            || (radius > 0.0 && !(bounds[0].as_vec3().cmplt(bounds[1].as_vec3()).all()))
        {
            return Err(
                "Sphere guide cannot be represented at native world-coordinate precision".into(),
            );
        }
        Ok((radius, bounds.map(|point| point.to_array())))
    };
    match check() {
        Ok((radius, bounds)) => {
            result.radius = Some(radius);
            if radius > 0.0 {
                result.state = "ready";
                result.guide_bounds = Some(bounds);
            } else {
                result.state = "nonpositive";
                result.error = Some("Nonpositive authored/default radius has no finite sphere guide; runtime sentinel or fallback behavior is entity-specific".into());
            }
        }
        Err(error) => {
            result.state = "invalid";
            result.error = Some(bounded(&error, MAX_SPHERE_ERROR_BYTES));
        }
    }
    result
}

impl Sphere {
    pub fn lines(&self) -> Vec<[[f64; 3]; 2]> {
        if self.state != "ready" {
            return Vec::new();
        }
        let (Some(center), Some(radius)) = (self.center, self.radius) else {
            return Vec::new();
        };
        let center = DVec3::from_array(center);
        let mut lines = Vec::with_capacity(3 * SPHERE_SEGMENTS);
        for [u, v] in [
            [DVec3::X, DVec3::Y],
            [DVec3::X, DVec3::Z],
            [DVec3::Y, DVec3::Z],
        ] {
            for index in 0..SPHERE_SEGMENTS {
                let point = |index: usize| {
                    let angle = index as f64 * std::f64::consts::TAU / SPHERE_SEGMENTS as f64;
                    (center + (u * angle.cos() + v * angle.sin()) * radius).to_array()
                };
                lines.push([point(index), point((index + 1) % SPHERE_SEGMENTS)]);
            }
        }
        lines
    }
}
