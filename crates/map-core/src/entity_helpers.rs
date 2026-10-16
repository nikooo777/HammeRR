use glam::DVec3;
use source_formats::fgd::{Class, Helper};
use source_formats::vmf::Entity;

use crate::ObjectId;
use crate::query::{Hit, Ray};

pub mod lines;
pub mod projectors;
pub mod spheres;

pub const MAX_HELPER_EXTENT: f64 = 65_536.0;

#[derive(Debug, Clone, Default)]
pub struct Appearance {
    pub local_bounds: Option<[DVec3; 2]>,
    pub invalid_size: bool,
    pub color: Option<[u8; 3]>,
    pub errors: Vec<String>,
    pub unsupported: Vec<String>,
}

impl Appearance {
    pub fn resolve(class: &Class) -> Self {
        let mut result = Self::default();
        for (name, count) in ["size", "color"].map(|name| {
            (
                name,
                class
                    .helpers
                    .iter()
                    .filter(|helper| helper.name == name)
                    .count(),
            )
        }) {
            if count > 1 {
                if name == "size" {
                    result.invalid_size = true;
                }
                result
                    .errors
                    .push(format!("Multiple {name}() helpers are ambiguous"));
                continue;
            }
            let Some(helper) = class.helpers.iter().find(|helper| helper.name == name) else {
                continue;
            };
            match name {
                "size" => match size(helper) {
                    Ok(bounds) => result.local_bounds = Some(bounds),
                    Err(error) => {
                        result.invalid_size = true;
                        result.errors.push(error);
                    }
                },
                "color" => match color(helper) {
                    Ok(color) => result.color = Some(color),
                    Err(error) => result.errors.push(error),
                },
                _ => unreachable!(),
            }
        }
        result.unsupported = class
            .helpers
            .iter()
            .filter(|helper| {
                !["size", "color"].contains(&helper.name.as_str())
                    && !spheres::supported(class, helper)
                    && !lines::supported(class, helper)
            })
            .map(|helper| helper.name.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        result
    }

    pub fn world_bounds(&self, entity: &Entity) -> Option<(DVec3, DVec3)> {
        let [mins, maxs] = self.local_bounds?;
        let origin = entity.origin().filter(|origin| origin.is_finite())?;
        let bounds = (origin + mins, origin + maxs);
        (bounds.0.is_finite() && bounds.1.is_finite()).then_some(bounds)
    }

    pub fn pick(&self, entity: &Entity, ray: &Ray) -> Option<Hit> {
        if entity.hidden || !entity.solids.is_empty() || !ray.origin.is_finite() {
            return None;
        }
        let direction = ray.direction.try_normalize()?;
        let (mins, maxs) = self.world_bounds(entity)?;
        let (mut enter, mut exit) = (f64::NEG_INFINITY, f64::INFINITY);
        for axis in 0..3 {
            if direction[axis].abs() < 1e-12 {
                if ray.origin[axis] < mins[axis] || ray.origin[axis] > maxs[axis] {
                    return None;
                }
            } else {
                let a = (mins[axis] - ray.origin[axis]) / direction[axis];
                let b = (maxs[axis] - ray.origin[axis]) / direction[axis];
                enter = enter.max(a.min(b));
                exit = exit.min(a.max(b));
            }
        }
        (enter >= 0.0 && enter <= exit).then_some(Hit {
            object: ObjectId::Entity(entity.id),
            solid: None,
            side: None,
            distance: enter,
            point: ray.origin + direction * enter,
        })
    }
}

pub fn specialized_light(entity: &Entity) -> bool {
    matches!(
        entity
            .classname()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "light" | "light_spot" | "light_environment" | "light_dynamic"
    )
}

fn values(helper: &Helper) -> Result<Vec<i32>, String> {
    helper
        .arguments
        .iter()
        .map(|argument| argument.parse::<i32>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| format!("{}() requires native integer arguments", helper.name))
}

fn size(helper: &Helper) -> Result<[DVec3; 2], String> {
    if ![3, 6].contains(&helper.arguments.len()) {
        return Err("size() requires three dimensions or six minimum/maximum coordinates".into());
    }
    let values = values(helper)?;
    let vector = |values: &[i32]| {
        DVec3::new(
            f64::from(values[0]),
            f64::from(values[1]),
            f64::from(values[2]),
        )
    };
    let bounds = match values.len() {
        3 => {
            let half = vector(&values) * 0.5;
            [-half, half]
        }
        6 => [vector(&values[..3]), vector(&values[3..])],
        _ => {
            return Err(
                "size() requires three dimensions or six minimum/maximum coordinates".into(),
            );
        }
    };
    if !bounds[0].cmplt(bounds[1]).all() {
        return Err("size() needs positive dimensions and ordered bounds".into());
    }
    if bounds
        .iter()
        .any(|point| point.abs().max_element() > MAX_HELPER_EXTENT)
    {
        return Err(format!(
            "size() coordinates must stay within ±{MAX_HELPER_EXTENT} editor units"
        ));
    }
    Ok(bounds)
}

fn color(helper: &Helper) -> Result<[u8; 3], String> {
    if helper.arguments.len() != 3 {
        return Err("color() requires three integer channels".into());
    }
    let values = values(helper)?;
    let channels = values
        .into_iter()
        .map(u8::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "color() channels must be between 0 and 255")?;
    channels
        .try_into()
        .map_err(|_| "color() requires three integer channels".into())
}

#[cfg(test)]
mod tests;
