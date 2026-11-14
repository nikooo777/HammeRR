use glam::{DMat3, DVec3};
use source_formats::vmf::{Side, Solid, Vmf, is_precise_vertices_key};

use super::BrushGeometry;
use crate::EditError;

fn invalid(solid: &Solid, message: impl std::fmt::Display) -> EditError {
    EditError::Invalid(format!("Brush {} precise vertices: {message}", solid.id))
}

pub fn has_precise_vertices(solid: &Solid) -> bool {
    solid.sides.iter().any(|side| {
        side.extra
            .iter()
            .any(|node| is_precise_vertices_key(&node.key))
            || side
                .extra_properties
                .iter()
                .any(|(key, _)| is_precise_vertices_key(key))
    })
}

pub fn validate_precise(solid: &Solid) -> Result<(), EditError> {
    solid
        .validate_precise_vertices()
        .map_err(|error| invalid(solid, error))
}

pub fn validate_precise_vmf(vmf: &Vmf) -> Result<(), EditError> {
    for solid in vmf.solids() {
        validate_precise(solid)?;
    }
    Ok(())
}

pub(crate) fn ensure_editable(solid: &Solid) -> Result<bool, EditError> {
    if !has_precise_vertices(solid) {
        return Ok(false);
    }
    for side in &solid.sides {
        if side
            .precise_vertices()
            .map_err(|error| invalid(solid, error))?
            .is_some_and(|ring| ring.has_opaque_fields)
        {
            return Err(invalid(
                solid,
                "opaque extension fields cannot be remapped by a geometry edit",
            ));
        }
    }
    BrushGeometry::from_solid(solid).map_err(|error| invalid(solid, error))?;
    Ok(true)
}

pub fn prepare_precise_edit(solid: &mut Solid) -> Result<bool, EditError> {
    if !ensure_editable(solid)? {
        return Ok(false);
    }
    validate_precise(solid)?;
    for side in &mut solid.sides {
        side.clear_precise_vertices()
            .map_err(|error| EditError::Invalid(error.to_string()))?;
    }
    Ok(true)
}

pub fn refresh_precise(solid: &mut Solid, enabled: bool) -> Result<(), EditError> {
    if !enabled {
        return Ok(());
    }
    for side in &mut solid.sides {
        side.clear_precise_vertices()
            .map_err(|error| EditError::Invalid(error.to_string()))?;
    }
    let geometry = BrushGeometry::from_solid(solid).map_err(|error| invalid(solid, error))?;
    for face in geometry.faces {
        solid.sides[face.side]
            .set_precise_vertices(&face.vertices)
            .map_err(|error| EditError::Invalid(error.to_string()))?;
    }
    validate_precise(solid)
}

/// Maps retained vertex caches after an explicit affine edit of the planes.
pub fn transform_precise(
    solid: &mut Solid,
    linear: DMat3,
    offset: DVec3,
) -> Result<bool, EditError> {
    if !has_precise_vertices(solid) {
        return Ok(false);
    }
    validate_precise(solid)?;
    let determinant = linear.determinant();
    if !linear.is_finite() || !offset.is_finite() || !determinant.is_finite() || determinant == 0.0
    {
        return Err(invalid(
            solid,
            "affine mapping must be finite and invertible",
        ));
    }
    if linear == DMat3::IDENTITY && offset == DVec3::ZERO {
        return Ok(true);
    }
    for side in &solid.sides {
        if side
            .precise_vertices()
            .map_err(|error| invalid(solid, error))?
            .is_some_and(|ring| ring.has_opaque_fields)
        {
            return Err(invalid(
                solid,
                "opaque extension fields cannot be remapped by a geometry edit",
            ));
        }
    }
    for side in &mut solid.sides {
        if let Some(ring) = side
            .precise_vertices()
            .map_err(|error| EditError::Invalid(error.to_string()))?
        {
            let mut vertices = ring
                .vertices
                .into_iter()
                .map(|point| linear * point + offset)
                .collect::<Vec<_>>();
            if determinant < 0.0 {
                vertices[1..].reverse();
            }
            side.set_precise_vertices(&vertices)
                .map_err(|error| EditError::Invalid(error.to_string()))?;
        }
    }
    validate_precise(solid)?;
    Ok(true)
}

pub(crate) fn inherit_surface(source: &Side, geometry: &Side) -> Result<Side, EditError> {
    let ring = geometry
        .precise_vertices()
        .map_err(|error| EditError::Invalid(error.to_string()))?;
    let mut result = source.clone();
    result
        .clear_precise_vertices()
        .map_err(|error| EditError::Invalid(error.to_string()))?;
    result.plane = geometry.plane;
    if let Some(ring) = ring {
        result
            .set_precise_vertices(&ring.vertices)
            .map_err(|error| EditError::Invalid(error.to_string()))?;
    }
    Ok(result)
}

fn same_planes(a: &Solid, b: &Solid) -> bool {
    a.sides.len() == b.sides.len()
        && a.sides.iter().all(|side| {
            b.sides
                .iter()
                .any(|other| other.id == side.id && other.plane == side.plane)
        })
}

pub(crate) fn reconcile(
    solid: &mut Solid,
    before: Option<&Solid>,
    mapped: Option<&[(u32, source_formats::vmf::Plane)]>,
) -> Result<(), EditError> {
    validate_precise(solid)?;
    let present = has_precise_vertices(solid);
    let Some(before) = before else {
        return Ok(());
    };
    if (!present && !has_precise_vertices(before)) || same_planes(solid, before) {
        return Ok(());
    }
    ensure_editable(before)?;
    if present
        && mapped.is_some_and(|planes| {
            planes.len() == solid.sides.len()
                && planes
                    .iter()
                    .zip(&solid.sides)
                    .all(|(&(id, plane), side)| id == side.id && plane == side.plane)
        })
    {
        return Ok(());
    }
    refresh_precise(solid, true)
}
