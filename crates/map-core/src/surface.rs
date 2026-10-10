use std::collections::BTreeSet;

use glam::DQuat;
use source_formats::vmf::{Side, Solid};

use crate::{BrushGeometry, Document, EditError, ObjectId, Plane, Transaction};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FaceId {
    pub solid: u32,
    pub side: u32,
}

#[derive(Debug, Clone, Default)]
pub struct SurfaceEdit {
    pub material: Option<String>,
    pub shift_u: Option<f64>,
    pub shift_v: Option<f64>,
    pub scale_u: Option<f64>,
    pub scale_v: Option<f64>,
    pub rotation: Option<f64>,
    pub fit: Option<[u32; 2]>,
}

pub fn solid(doc: &Document, id: u32) -> Option<&Solid> {
    doc.vmf().solids().find(|solid| solid.id == id)
}

pub fn side(doc: &Document, face: FaceId) -> Option<&Side> {
    solid(doc, face.solid)?
        .sides
        .iter()
        .find(|side| side.id == face.side)
}

fn invalid(message: &str) -> EditError {
    EditError::Invalid(message.into())
}

fn update(side: &mut Side, edit: &SurfaceEdit, points: &[glam::DVec3]) -> Result<(), EditError> {
    if let Some(material) = &edit.material {
        if material.trim().is_empty() {
            return Err(invalid("material must not be empty"));
        }
        side.material = material.trim().to_ascii_uppercase();
    }
    if let Some(rotation) = edit.rotation {
        let normal = Plane::from_points(side.plane.0)
            .ok_or_else(|| invalid("face has an invalid plane"))?
            .normal;
        let turn = DQuat::from_axis_angle(normal, (rotation - side.rotation).to_radians());
        side.u_axis.axis = turn * side.u_axis.axis;
        side.v_axis.axis = turn * side.v_axis.axis;
        side.rotation = rotation;
    }
    for (axis, shift, scale) in [
        (&mut side.u_axis, edit.shift_u, edit.scale_u),
        (&mut side.v_axis, edit.shift_v, edit.scale_v),
    ] {
        if let Some(shift) = shift {
            axis.offset = shift;
        }
        if let Some(scale) = scale {
            axis.scale = scale;
        }
    }
    if let Some(size) = edit.fit {
        for (axis, pixels) in [(&mut side.u_axis, size[0]), (&mut side.v_axis, size[1])] {
            let (lo, hi) = points
                .iter()
                .map(|point| point.dot(axis.axis))
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), value| {
                    (lo.min(value), hi.max(value))
                });
            if !lo.is_finite() || hi - lo < 1e-8 {
                return Err(invalid("texture axis does not span the face"));
            }
            let sign = axis.scale.signum();
            axis.scale = sign * (hi - lo) / f64::from(pixels);
            axis.offset = if sign > 0.0 {
                -lo / axis.scale
            } else {
                -hi / axis.scale
            };
        }
    }
    if !side.u_axis.axis.is_finite()
        || !side.v_axis.axis.is_finite()
        || !side.u_axis.offset.is_finite()
        || !side.v_axis.offset.is_finite()
        || !side.u_axis.scale.is_finite()
        || !side.v_axis.scale.is_finite()
        || side.u_axis.scale.abs() < 1e-8
        || side.v_axis.scale.abs() < 1e-8
    {
        return Err(invalid(
            "texture projection must have finite axes and nonzero scales",
        ));
    }
    Ok(())
}

pub fn edit_faces(
    tx: &mut Transaction,
    faces: &[FaceId],
    edit: &SurfaceEdit,
) -> Result<usize, EditError> {
    if faces.is_empty() {
        return Err(invalid("select at least one face"));
    }
    if [
        edit.shift_u,
        edit.shift_v,
        edit.scale_u,
        edit.scale_v,
        edit.rotation,
    ]
    .into_iter()
    .flatten()
    .any(|value| !value.is_finite())
    {
        return Err(invalid("texture values must be finite"));
    }
    if edit.fit.is_some_and(|size| size.contains(&0)) {
        return Err(invalid("texture dimensions must be positive"));
    }
    let mut changed = 0;
    for face in faces.iter().copied().collect::<BTreeSet<_>>() {
        let owner = tx
            .document()
            .owner_of_solid(face.solid)
            .ok_or(EditError::NotFound(ObjectId::Solid(face.solid)))?;
        let brush =
            solid(tx.document(), face.solid).ok_or_else(|| invalid("brush does not exist"))?;
        let geometry =
            BrushGeometry::from_solid(brush).map_err(|error| invalid(&error.to_string()))?;
        let index = brush
            .sides
            .iter()
            .position(|side| side.id == face.side)
            .ok_or_else(|| invalid("face does not belong to this brush"))?;
        let points = geometry
            .faces
            .iter()
            .find(|face| face.side == index)
            .ok_or_else(|| invalid("face has no visible surface"))?
            .vertices
            .clone();
        let brush = match owner {
            ObjectId::Solid(id) => tx.solid_mut(id)?,
            ObjectId::Entity(id) => tx
                .entity_mut(id)?
                .solids
                .iter_mut()
                .find(|solid| solid.id == face.solid)
                .ok_or_else(|| invalid("brush does not exist"))?,
        };
        let side = &mut brush.sides[index];
        let before = side.clone();
        update(side, edit, &points)?;
        changed += usize::from(*side != before);
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{edit, texel_uv};
    use glam::{DVec2, DVec3};
    use source_formats::keyvalues::WriteOptions;
    use source_formats::vmf::Vmf;

    fn fixture() -> (Document, FaceId) {
        let mut doc = Document::empty();
        let ObjectId::Solid(id) = doc
            .edit("block", |tx| {
                edit::create_block(tx, DVec3::ZERO, DVec3::new(128.0, 64.0, 16.0), "brick/wall")
            })
            .unwrap()
        else {
            panic!()
        };
        let side = doc
            .solid(id)
            .unwrap()
            .sides
            .iter()
            .find(|side| Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
            .unwrap()
            .id;
        (doc, FaceId { solid: id, side })
    }

    #[test]
    fn selected_face_edits_rotate_projection_preserve_neighbors_and_roundtrip() {
        let (mut doc, face) = fixture();
        let before = doc.vmf().clone();
        let original = side(&doc, face).unwrap().clone();
        let patch = SurfaceEdit {
            material: Some("wood/floor".into()),
            shift_u: Some(12.5),
            shift_v: Some(-8.0),
            scale_u: Some(0.5),
            scale_v: Some(-0.25),
            rotation: Some(90.0),
            ..Default::default()
        };
        assert_eq!(
            doc.edit("texture", |tx| edit_faces(tx, &[face, face], &patch))
                .unwrap(),
            1
        );
        let result = side(&doc, face).unwrap();
        let turn = DQuat::from_axis_angle(DVec3::Z, std::f64::consts::FRAC_PI_2);
        assert!(
            result
                .u_axis
                .axis
                .abs_diff_eq(turn * original.u_axis.axis, 1e-8)
        );
        assert_eq!(result.material, "WOOD/FLOOR");
        assert_eq!(texel_uv(result, DVec3::ZERO), DVec2::new(12.5, -8.0));
        for old in &before.world.solids[0].sides {
            if old.id != face.side {
                assert_eq!(
                    solid(&doc, face.solid)
                        .unwrap()
                        .sides
                        .iter()
                        .find(|side| side.id == old.id)
                        .unwrap(),
                    old
                );
            }
        }
        let saved = doc.vmf().write(&WriteOptions::default());
        let reopened = Document::new(Vmf::parse(&saved).unwrap());
        assert_eq!(side(&reopened, face), Some(result));
        let after = doc.vmf().clone();
        doc.undo().unwrap();
        assert_eq!(doc.vmf(), &before);
        doc.redo().unwrap();
        assert_eq!(doc.vmf(), &after);
        let revision = doc.revision();
        assert_eq!(
            doc.edit("same values", |tx| edit_faces(tx, &[face], &patch))
                .unwrap(),
            0
        );
        assert_eq!(doc.revision(), revision);
    }

    #[test]
    fn fit_projects_exactly_one_tile_on_rotated_and_mirrored_faces() {
        let (mut doc, face) = fixture();
        doc.edit("fit", |tx| {
            edit_faces(
                tx,
                &[face],
                &SurfaceEdit {
                    rotation: Some(37.0),
                    scale_v: Some(-1.0),
                    fit: Some([256, 128]),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        let brush = solid(&doc, face.solid).unwrap();
        let index = brush
            .sides
            .iter()
            .position(|side| side.id == face.side)
            .unwrap();
        let geometry = BrushGeometry::from_solid(brush).unwrap();
        let vertices = &geometry
            .faces
            .iter()
            .find(|face| face.side == index)
            .unwrap()
            .vertices;
        let (lo, hi) = vertices
            .iter()
            .map(|point| texel_uv(side(&doc, face).unwrap(), *point))
            .fold(
                (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
                |(lo, hi), uv| (lo.min(uv), hi.max(uv)),
            );
        assert!(lo.abs_diff_eq(DVec2::ZERO, 1e-8));
        assert!(hi.abs_diff_eq(DVec2::new(256.0, 128.0), 1e-8));
    }

    #[test]
    fn invalid_batches_and_parameters_roll_back_geometry_and_history() {
        let (mut doc, face) = fixture();
        let before = doc.vmf().clone();
        let revision = doc.revision();
        for patch in [
            SurfaceEdit {
                scale_u: Some(0.0),
                ..Default::default()
            },
            SurfaceEdit {
                rotation: Some(f64::INFINITY),
                ..Default::default()
            },
            SurfaceEdit {
                fit: Some([0, 128]),
                ..Default::default()
            },
        ] {
            assert!(
                doc.edit("invalid", |tx| edit_faces(tx, &[face], &patch))
                    .is_err()
            );
            assert_eq!(doc.vmf(), &before);
            assert_eq!(doc.revision(), revision);
        }
        let patch = SurfaceEdit {
            material: Some("metal/test".into()),
            ..Default::default()
        };
        assert!(
            doc.edit("bad batch", |tx| edit_faces(
                tx,
                &[
                    face,
                    FaceId {
                        solid: face.solid,
                        side: u32::MAX
                    }
                ],
                &patch
            ))
            .is_err()
        );
        assert_eq!(doc.vmf(), &before);
        assert_eq!(doc.revision(), revision);
    }

    #[test]
    fn face_ids_continue_to_work_inside_brush_entities() {
        let (mut doc, face) = fixture();
        doc.edit("tie", |tx| {
            edit::tie_to_entity(tx, &[face.solid], "func_detail", &[])
        })
        .unwrap();
        let before = doc.vmf().clone();
        doc.edit("texture", |tx| {
            edit_faces(
                tx,
                &[face],
                &SurfaceEdit {
                    shift_u: Some(64.0),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        assert_eq!(side(&doc, face).unwrap().u_axis.offset, 64.0);
        doc.undo().unwrap();
        assert_eq!(doc.vmf(), &before);
    }
}
