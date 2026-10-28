use glam::{DMat3, DVec3, EulerRot};

use super::{
    Component, EPSILON, MAX_COORDINATE, Replacement, invalid, plan_affine, solid, topology,
};
use crate::{Document, EditError};

#[derive(Debug, Clone, PartialEq)]
pub enum ComponentTransform {
    Scale(DVec3),
    Rotate(DVec3),
}

pub struct TransformPlan {
    pub replacements: Vec<Replacement>,
    pub pivot: DVec3,
}

pub fn component_center(document: &Document, components: &[Component]) -> Result<DVec3, EditError> {
    if components.is_empty() || components.len() > super::MAX_COMPONENTS {
        return Err(invalid("Select between 1 and 512 faces, vertices or edges"));
    }
    let ids = components
        .iter()
        .map(|component| component.solid)
        .collect::<std::collections::BTreeSet<_>>();
    if ids.len() > super::MAX_SOLIDS {
        return Err(invalid(
            "Component editing supports at most 16 brushes at a time",
        ));
    }
    let mut mins = DVec3::splat(f64::INFINITY);
    let mut maxs = DVec3::splat(f64::NEG_INFINITY);
    for id in ids {
        let shape = topology(solid(document, id)?.1)?;
        for component in components.iter().filter(|component| component.solid == id) {
            for index in shape.resolve(&component.sides)? {
                let point = shape.vertices[index].position;
                mins = mins.min(point);
                maxs = maxs.max(point);
            }
        }
    }
    Ok((mins + maxs) * 0.5)
}

pub fn plan_transform(
    document: &Document,
    components: &[Component],
    transform: &ComponentTransform,
    pivot: Option<DVec3>,
    texture_lock: bool,
) -> Result<TransformPlan, EditError> {
    let center = component_center(document, components)?;
    let pivot = pivot.unwrap_or(center);
    if !pivot.is_finite() || pivot.abs().max_element() > MAX_COORDINATE {
        return Err(invalid(
            "Pivot needs finite world coordinates within ±65536 units",
        ));
    }
    let linear = match transform {
        ComponentTransform::Scale(factors) => {
            if !factors.is_finite() || factors.min_element() <= 0.0 || *factors == DVec3::ONE {
                return Err(invalid(
                    "Scale needs positive finite X/Y/Z factors and a nonidentity change",
                ));
            }
            DMat3::from_diagonal(*factors)
        }
        ComponentTransform::Rotate(degrees) => {
            if !degrees.is_finite() {
                return Err(invalid("Rotation needs finite X/Y/Z angles in degrees"));
            }
            let normalized = degrees.map(|angle| angle.rem_euclid(360.0));
            if normalized == DVec3::ZERO {
                return Err(invalid(
                    "Rotation must change at least one angle modulo 360 degrees",
                ));
            }
            let radians = normalized.map(f64::to_radians);
            DMat3::from_euler(EulerRot::ZYX, radians.z, radians.y, radians.x)
        }
    };
    let replacements = plan_affine(
        document,
        components,
        linear,
        pivot - linear * pivot,
        texture_lock,
    )?;
    let mut changed = false;
    for replacement in &replacements {
        let before = topology(solid(document, replacement.solid.id)?.1)?;
        let after = topology(&replacement.solid)?;
        changed |= before.vertices.iter().any(|vertex| {
            let other = after
                .vertices
                .iter()
                .find(|other| other.sides == vertex.sides)
                .unwrap();
            vertex.position.distance(other.position) > EPSILON
        });
    }
    if !changed {
        return Err(invalid(
            "The transform leaves every selected corner in place",
        ));
    }
    Ok(TransformPlan {
        replacements,
        pivot,
    })
}
