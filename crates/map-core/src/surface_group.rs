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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ObjectId, edit, texel_uv};

    fn fixture() -> (Document, Vec<FaceId>) {
        let mut document = Document::empty();
        let mut faces = Vec::new();
        for x in [0.0, 128.0] {
            let ObjectId::Solid(solid) = document
                .edit("block", |tx| {
                    edit::create_block(
                        tx,
                        DVec3::new(x, 0.0, 0.0),
                        DVec3::new(x + 128.0, 64.0, 16.0),
                        "brick/test",
                    )
                })
                .unwrap()
            else {
                panic!()
            };
            let side = document
                .solid(solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
                .unwrap()
                .id;
            faces.push(FaceId { solid, side });
        }
        (document, faces)
    }

    fn apply(document: &mut Document, edits: Vec<(FaceId, SurfaceEdit)>) {
        document
            .edit("group projection", |tx| {
                for (face, patch) in &edits {
                    surface::edit_faces(tx, &[*face], patch)?;
                }
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn group_fit_spans_combined_bounds_keeps_seams_and_preserves_mirroring() {
        let (mut document, faces) = fixture();
        document
            .edit("mirror", |tx| {
                surface::edit_faces(
                    tx,
                    &faces[..1],
                    &SurfaceEdit {
                        scale_u: Some(-0.25),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let before = document.vmf().clone();
        let edits = project(
            &document,
            &faces,
            Basis::Face(faces[0]),
            [256, 128],
            DVec2::new(2.0, 1.0),
        )
        .unwrap();
        apply(&mut document, edits);
        let a = surface::side(&document, faces[0]).unwrap();
        let b = surface::side(&document, faces[1]).unwrap();
        assert!(a.u_axis.scale < 0.0);
        assert_eq!(a.u_axis, b.u_axis);
        assert_eq!(a.v_axis, b.v_axis);
        for y in [0.0, 32.0, 64.0] {
            let point = DVec3::new(128.0, y, 16.0);
            assert!((texel_uv(a, point) - texel_uv(b, point)).length() < 1e-8);
        }
        let points: Vec<_> = faces
            .iter()
            .flat_map(|face| surface::face_vertices(&document, *face).unwrap())
            .map(|point| texel_uv(a, point))
            .collect();
        let lo = points
            .iter()
            .copied()
            .fold(DVec2::splat(f64::INFINITY), DVec2::min);
        let hi = points
            .iter()
            .copied()
            .fold(DVec2::splat(f64::NEG_INFINITY), DVec2::max);
        assert!(lo.length() < 1e-8);
        assert!((hi - DVec2::new(512.0, 128.0)).length() < 1e-8);
        document.undo().unwrap();
        assert_eq!(document.vmf(), &before);
    }

    #[test]
    fn view_projection_uses_view_axes_and_rejects_edge_on_or_invalid_batches() {
        let (mut document, faces) = fixture();
        let right = DVec3::new(1.0, 1.0, 0.0).normalize();
        let up = DVec3::new(-1.0, 1.0, 0.0).normalize();
        let edits = project(
            &document,
            &faces,
            Basis::View { right, up },
            [256, 128],
            DVec2::ONE,
        )
        .unwrap();
        apply(&mut document, edits);
        let side = surface::side(&document, faces[1]).unwrap();
        assert!((side.u_axis.axis - right).length() < 1e-8);
        assert!((side.v_axis.axis + up).length() < 1e-8);
        let before = document.vmf().clone();
        assert!(
            project(
                &document,
                &faces,
                Basis::View {
                    right: DVec3::X,
                    up: DVec3::Z
                },
                [256, 128],
                DVec2::ONE
            )
            .is_err()
        );
        for repeats in [
            DVec2::ZERO,
            DVec2::splat(f64::NAN),
            DVec2::splat(1025.0),
            DVec2::splat(f64::MIN_POSITIVE / 128.0),
        ] {
            assert!(
                project(
                    &document,
                    &faces,
                    Basis::Face(faces[0]),
                    [256, 128],
                    repeats
                )
                .is_err()
            );
        }
        let other = FaceId {
            solid: faces[0].solid,
            side: document
                .solid(faces[0].solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| Plane::from_points(side.plane.0).unwrap().normal == DVec3::Y)
                .unwrap()
                .id,
        };
        assert!(
            project(
                &document,
                &[faces[0], other],
                Basis::Face(faces[0]),
                [256, 128],
                DVec2::ONE
            )
            .is_err()
        );
        assert_eq!(document.vmf(), &before);
    }
}
