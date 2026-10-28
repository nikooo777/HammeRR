use std::collections::BTreeSet;

use glam::{DVec2, DVec3};
use source_formats::vmf::UvAxis;

use crate::surface::{self, FaceId, Projection, SurfaceEdit};
use crate::{Document, EditError, Plane};

#[derive(Debug, Clone, Copy)]
pub enum Basis {
    Face(FaceId),
    View { right: DVec3, up: DVec3 },
}

fn invalid(message: &str) -> EditError {
    EditError::Invalid(message.into())
}

pub fn project(
    document: &Document,
    faces: &[FaceId],
    basis: Basis,
    size: [u32; 2],
    repeats: DVec2,
) -> Result<Vec<(FaceId, SurfaceEdit)>, EditError> {
    if faces.is_empty() {
        return Err(invalid("Select at least one face"));
    }
    if size.contains(&0)
        || !repeats.is_finite()
        || repeats.min_element() <= 0.0
        || repeats.max_element() > 1024.0
    {
        return Err(invalid(
            "Texture dimensions must be positive and tile repeats must be greater than 0 and at most 1024",
        ));
    }
    let faces: BTreeSet<_> = faces.iter().copied().collect();
    let (mut projection, plane) = match basis {
        Basis::Face(reference) => {
            if !faces.contains(&reference) {
                return Err(invalid(
                    "The reference face must belong to the target selection",
                ));
            }
            let side = surface::side(document, reference)
                .ok_or_else(|| invalid("Reference face does not exist"))?;
            let plane = Plane::from_points(side.plane.0)
                .ok_or_else(|| invalid("Reference face has an invalid plane"))?;
            (
                Projection {
                    u: side.u_axis,
                    v: side.v_axis,
                    rotation: side.rotation,
                },
                Some(plane),
            )
        }
        Basis::View { right, up } => {
            if !right.is_finite()
                || !up.is_finite()
                || right.length_squared() < 1e-12
                || up.length_squared() < 1e-12
            {
                return Err(invalid("View axes must be finite and nonzero"));
            }
            let right = right.normalize();
            let down = -up.normalize();
            if right.dot(down).abs() > 1e-6 {
                return Err(invalid("View axes must be perpendicular"));
            }
            (
                Projection {
                    u: UvAxis {
                        axis: right,
                        offset: 0.0,
                        scale: 1.0,
                    },
                    v: UvAxis {
                        axis: down,
                        offset: 0.0,
                        scale: 1.0,
                    },
                    rotation: 0.0,
                },
                None,
            )
        }
    };
    if !projection.u.axis.is_finite()
        || !projection.v.axis.is_finite()
        || !projection.rotation.is_finite()
        || !projection.u.scale.is_finite()
        || !projection.v.scale.is_finite()
        || projection.u.scale.abs() < 1e-8
        || projection.v.scale.abs() < 1e-8
    {
        return Err(invalid(
            "Reference projection is invalid; align that face first",
        ));
    }
    let mut points = Vec::new();
    for face in &faces {
        let side =
            surface::side(document, *face).ok_or_else(|| invalid("Target face does not exist"))?;
        let target = Plane::from_points(side.plane.0)
            .ok_or_else(|| invalid("Target face has an invalid plane"))?;
        if let Some(plane) = plane
            && (plane.normal.dot(target.normal) < 1.0 - 1e-6
                || (plane.dist - target.dist).abs() > 1e-4)
        {
            return Err(invalid(
                "Fit together needs faces on the same plane and facing the same direction; use view projection for other surfaces",
            ));
        }
        let area = projection
            .u
            .axis
            .cross(projection.v.axis)
            .dot(target.normal)
            .abs();
        if area < 1e-8 * projection.u.axis.length() * projection.v.axis.length() || area < 1e-12 {
            return Err(invalid(
                "A target face is edge-on to this projection; change the view or choose different faces",
            ));
        }
        points.extend(surface::face_vertices(document, *face)?);
    }
    for (axis, pixels, repeats) in [
        (&mut projection.u, size[0], repeats.x),
        (&mut projection.v, size[1], repeats.y),
    ] {
        let (lo, hi) = points
            .iter()
            .map(|point| point.dot(axis.axis))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), value| {
                (lo.min(value), hi.max(value))
            });
        if !lo.is_finite() || !hi.is_finite() || hi - lo < 1e-8 {
            return Err(invalid("Texture axis does not span the selected faces"));
        }
        let sign = axis.scale.signum();
        axis.scale = sign * (hi - lo) / (f64::from(pixels) * repeats);
        axis.offset = if sign > 0.0 {
            -lo / axis.scale
        } else {
            -hi / axis.scale
        };
        if !axis.scale.is_finite() || axis.scale.abs() < 1e-8 || !axis.offset.is_finite() {
            return Err(invalid(
                "Tile repeats produce a texture scale outside the supported range",
            ));
        }
    }
    Ok(faces
        .into_iter()
        .map(|face| {
            (
                face,
                SurfaceEdit {
                    projection: Some(projection.clone()),
                    ..Default::default()
                },
            )
        })
        .collect())
}
