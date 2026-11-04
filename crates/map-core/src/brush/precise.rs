use glam::{DMat3, DVec3};
use source_formats::vmf::{Side, Solid, Vmf, is_precise_vertices_key};

use super::{BrushGeometry, Face};
use crate::{EditError, Plane, subobject};

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

fn triple(vertices: &[DVec3]) -> Option<[DVec3; 3]> {
    vertices
        .get(1..)?
        .windows(2)
        .map(|pair| [vertices[0], pair[0], pair[1]])
        .max_by(|a, b| {
            let area = |points: &[DVec3; 3]| {
                (points[0] - points[1])
                    .cross(points[2] - points[1])
                    .length_squared()
            };
            area(a).total_cmp(&area(b))
        })
        .filter(|points| Plane::from_points(*points).is_some())
}

pub(super) fn geometry(solid: &Solid, canonical: bool) -> Result<Option<BrushGeometry>, EditError> {
    let Some(rings) = solid
        .precise_vertices()
        .map_err(|error| invalid(solid, error))?
    else {
        return Ok(None);
    };
    let mut faces = Vec::with_capacity(rings.len());
    let mut mins = DVec3::MAX;
    let mut maxs = DVec3::MIN;
    for (index, ring) in rings.into_iter().enumerate() {
        let points = triple(&ring.vertices)
            .ok_or_else(|| invalid(solid, "a face has no noncollinear plane triple"))?;
        if canonical && solid.sides[index].plane.0 != points {
            return Err(invalid(
                solid,
                "a stored ring is stale or its plane has not been canonicalized",
            ));
        }
        let plane = Plane::from_points(points).unwrap();
        if ring
            .vertices
            .iter()
            .any(|&point| plane.distance_to(point).abs() > subobject::EPSILON)
        {
            return Err(invalid(solid, "a face ring is nonplanar"));
        }
        if ring.vertices.iter().enumerate().any(|(index, point)| {
            ring.vertices[index + 1..]
                .iter()
                .any(|other| point.distance(*other) < subobject::EPSILON)
        }) {
            return Err(invalid(solid, "a face ring repeats a vertex"));
        }
        for edge in 0..ring.vertices.len() {
            let a = ring.vertices[edge];
            let delta = ring.vertices[(edge + 1) % ring.vertices.len()] - a;
            let length = delta.length();
            if length == 0.0
                || ring.vertices.iter().any(|&point| {
                    delta.cross(plane.normal).dot(point - a) < -subobject::EPSILON * length
                })
            {
                return Err(invalid(
                    solid,
                    "a face ring is nonconvex, self-intersecting or collapsed",
                ));
            }
        }
        for &point in &ring.vertices {
            mins = mins.min(point);
            maxs = maxs.max(point);
        }
        faces.push(Face {
            side: index,
            plane,
            vertices: ring.vertices,
        });
    }
    if faces.iter().any(|face| {
        faces
            .iter()
            .flat_map(|other| &other.vertices)
            .any(|&point| face.plane.distance_to(point) > subobject::EPSILON)
    }) {
        return Err(invalid(
            solid,
            "face rings do not enclose a consistently oriented convex volume",
        ));
    }
    let geometry = BrushGeometry { faces, mins, maxs };
    subobject::validate_geometry(solid, &geometry)?;
    Ok(Some(geometry))
}

pub fn canonicalize_precise(solid: &mut Solid) -> Result<(), EditError> {
    let Some(geometry) = geometry(solid, false)? else {
        return Ok(());
    };
    for face in geometry.faces {
        solid.sides[face.side].plane.0 = triple(&face.vertices).unwrap();
    }
    Ok(())
}

pub fn normalize_precise_vmf(vmf: &mut Vmf) -> Result<(), EditError> {
    for solid in vmf.world.solids.iter_mut().chain(
        vmf.entities
            .iter_mut()
            .flat_map(|entity| &mut entity.solids),
    ) {
        if has_precise_vertices(solid) {
            canonicalize_precise(solid)?;
        }
    }
    Ok(())
}

pub(crate) fn ensure_editable(solid: &Solid) -> Result<bool, EditError> {
    if !has_precise_vertices(solid) {
        return Ok(false);
    }
    let rings = solid
        .precise_vertices()
        .map_err(|error| invalid(solid, error))?
        .unwrap();
    if rings.iter().any(|ring| ring.has_opaque_fields) {
        return Err(invalid(
            solid,
            "opaque extension fields cannot be remapped by a geometry edit",
        ));
    }
    BrushGeometry::from_solid(solid).map_err(|error| invalid(solid, error))?;
    Ok(true)
}

pub fn prepare_precise_edit(solid: &mut Solid) -> Result<bool, EditError> {
    if !ensure_editable(solid)? {
        return Ok(false);
    }
    canonicalize_precise(solid)?;
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
    subobject::validate_geometry(solid, &geometry)?;
    for face in geometry.faces {
        solid.sides[face.side]
            .set_precise_vertices(&face.vertices)
            .map_err(|error| EditError::Invalid(error.to_string()))?;
    }
    canonicalize_precise(solid)
}

/// Maps the retained rings after an explicit affine edit of the legacy planes.
pub fn transform_precise(
    solid: &mut Solid,
    linear: DMat3,
    offset: DVec3,
) -> Result<bool, EditError> {
    let Some(rings) = solid
        .precise_vertices()
        .map_err(|error| invalid(solid, error))?
    else {
        return Ok(false);
    };
    let determinant = linear.determinant();
    if !linear.is_finite() || !offset.is_finite() || !determinant.is_finite() || determinant == 0.0
    {
        return Err(invalid(
            solid,
            "affine mapping must be finite and invertible",
        ));
    }
    if linear == DMat3::IDENTITY && offset == DVec3::ZERO {
        canonicalize_precise(solid)?;
        return Ok(true);
    }
    if rings.iter().any(|ring| ring.has_opaque_fields) {
        return Err(invalid(
            solid,
            "opaque extension fields cannot be remapped by a geometry edit",
        ));
    }
    let mut transformed = Vec::with_capacity(rings.len());
    for ring in rings {
        let mut vertices = ring
            .vertices
            .into_iter()
            .map(|point| linear * point + offset)
            .collect::<Vec<_>>();
        if determinant < 0.0 {
            vertices[1..].reverse();
        }
        transformed.push(vertices);
    }
    for (side, vertices) in solid.sides.iter_mut().zip(transformed) {
        side.set_precise_vertices(&vertices)
            .map_err(|error| EditError::Invalid(error.to_string()))?;
    }
    canonicalize_precise(solid)?;
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

pub(crate) fn reconcile(solid: &mut Solid, before: Option<&Solid>) -> Result<(), EditError> {
    let present = has_precise_vertices(solid);
    let original = before.is_some_and(has_precise_vertices);
    if !present && !original {
        return Ok(());
    }
    if present && geometry(solid, true).is_ok() {
        return Ok(());
    }
    let Some(before) = before else {
        return canonicalize_precise(solid);
    };
    if present && same_planes(solid, before) {
        return canonicalize_precise(solid);
    }
    if !original {
        return Err(invalid(
            solid,
            "new explicit rings and legacy planes changed ambiguously",
        ));
    }
    for side in &before.sides {
        if side
            .precise_vertices()
            .map_err(|error| invalid(solid, error))?
            .is_some_and(|ring| ring.has_opaque_fields)
        {
            return Err(invalid(
                solid,
                "opaque extension fields cannot be discarded or rebuilt",
            ));
        }
    }
    for side in &solid.sides {
        if let Some(ring) = side
            .precise_vertices()
            .map_err(|error| invalid(solid, error))?
        {
            let previous = before
                .sides
                .iter()
                .find(|previous| previous.id == side.id)
                .and_then(|previous| previous.precise_vertices().ok().flatten())
                .ok_or_else(|| invalid(solid, "changed explicit rings are ambiguous"))?;
            if ring.vertices != previous.vertices
                || ring.has_opaque_fields != previous.has_opaque_fields
            {
                return Err(invalid(
                    solid,
                    "changed explicit rings disagree with their edited planes",
                ));
            }
        }
    }
    refresh_precise(solid, true)
}
