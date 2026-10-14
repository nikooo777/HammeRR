use std::collections::BTreeSet;

use glam::{DQuat, DVec2, DVec3};
use source_formats::vmf::{Side, Solid, UvAxis};

use crate::{BrushGeometry, Document, EditError, ObjectId, Plane, Transaction};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FaceId {
    pub solid: u32,
    pub side: u32,
}

#[derive(Debug, Clone, Default)]
pub struct SurfaceEdit {
    pub terrain: Option<crate::displacement::Operation>,
    pub material: Option<String>,
    pub shift_u: Option<f64>,
    pub shift_v: Option<f64>,
    pub scale_u: Option<f64>,
    pub scale_v: Option<f64>,
    pub rotation: Option<f64>,
    pub fit: Option<[u32; 2]>,
    pub projection: Option<Projection>,
    pub alignment: Option<Alignment>,
    pub justify_u: Option<(Justify, u32)>,
    pub justify_v: Option<(Justify, u32)>,
    pub lightmap_scale: Option<u32>,
    pub smoothing_groups: Option<u32>,
    pub smoothing_add: u32,
    pub smoothing_remove: u32,
}

pub const MAX_LIGHTMAP_SCALE: u32 = 1024;
pub const HARD_EDGE_GROUPS: u32 = 0xff00_0000;

pub fn smoothing_mask(groups: &[u32]) -> Result<u32, EditError> {
    groups.iter().try_fold(0, |mask, &group| {
        if !(1..=32).contains(&group) {
            Err(invalid("Smoothing group numbers must be between 1 and 32"))
        } else {
            Ok(mask | (1 << (group - 1)))
        }
    })
}

pub fn smoothing_numbers(mask: u32) -> Vec<u32> {
    (1..=32)
        .filter(|group| mask & (1 << (group - 1)) != 0)
        .collect()
}

pub fn lightmap_uv(side: &Side, point: DVec3) -> Option<DVec2> {
    if side.lightmap_scale == 0 {
        return None;
    }
    let normal = Plane::from_points(side.plane.0)?.normal;
    if !side.u_axis.axis.is_finite()
        || !side.v_axis.axis.is_finite()
        || side.u_axis.axis.cross(side.v_axis.axis).dot(normal).abs() < 1e-8
    {
        return None;
    }
    let uv = DVec2::new(
        point.dot(side.u_axis.axis) + side.u_axis.scale * side.u_axis.offset,
        point.dot(side.v_axis.axis) + side.v_axis.scale * side.v_axis.offset,
    ) / f64::from(side.lightmap_scale);
    uv.is_finite().then_some(uv)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Projection {
    pub u: UvAxis,
    pub v: UvAxis,
    pub rotation: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    World,
    Face,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Justify {
    Start,
    Center,
    End,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceSample {
    pub face: FaceId,
    pub material: String,
    pub projection: Projection,
    pub normal: DVec3,
    pub vertices: Vec<DVec3>,
    pub displaced: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaintMode {
    Material,
    Projection,
    Wrap,
}

pub fn face_vertices(doc: &Document, face: FaceId) -> Result<Vec<DVec3>, EditError> {
    let brush = solid(doc, face.solid).ok_or_else(|| invalid("brush does not exist"))?;
    let index = brush
        .sides
        .iter()
        .position(|side| side.id == face.side)
        .ok_or_else(|| invalid("face does not belong to this brush"))?;
    BrushGeometry::from_solid(brush)
        .map_err(|error| invalid(&error.to_string()))?
        .faces
        .into_iter()
        .find(|face| face.side == index)
        .map(|face| face.vertices)
        .ok_or_else(|| invalid("face has no visible surface"))
}

pub fn sample(doc: &Document, face: FaceId) -> Result<SurfaceSample, EditError> {
    let side = side(doc, face).ok_or_else(|| invalid("face does not exist"))?;
    Ok(SurfaceSample {
        face,
        material: side.material.clone(),
        projection: Projection {
            u: side.u_axis,
            v: side.v_axis,
            rotation: side.rotation,
        },
        normal: Plane::from_points(side.plane.0)
            .ok_or_else(|| invalid("face has an invalid plane"))?
            .normal,
        vertices: face_vertices(doc, face)?,
        displaced: side.displacement.is_some(),
    })
}

pub(crate) fn shared_edge(source: &[DVec3], target: &[DVec3]) -> Option<(DVec3, DVec3)> {
    for (a, b) in source
        .iter()
        .zip(source.iter().cycle().skip(1))
        .take(source.len())
    {
        let length = a.distance(*b);
        if length < 1e-6 {
            continue;
        }
        let direction = (*b - *a) / length;
        for (c, d) in target
            .iter()
            .zip(target.iter().cycle().skip(1))
            .take(target.len())
        {
            if (*c - *a).cross(direction).length() > 1e-4
                || (*d - *a).cross(direction).length() > 1e-4
            {
                continue;
            }
            let c = (*c - *a).dot(direction);
            let d = (*d - *a).dot(direction);
            let lo = c.min(d).max(0.0);
            let hi = c.max(d).min(length);
            if hi - lo > 1e-4 {
                return Some((*a + direction * ((lo + hi) * 0.5), direction));
            }
        }
    }
    None
}

pub fn paint_edit(
    doc: &Document,
    target: FaceId,
    sample: &SurfaceSample,
    mode: PaintMode,
) -> Result<SurfaceEdit, EditError> {
    let mut edit = SurfaceEdit {
        material: Some(sample.material.clone()),
        ..Default::default()
    };
    if mode == PaintMode::Material {
        return Ok(edit);
    }
    let mut projection = sample.projection.clone();
    if mode == PaintMode::Wrap {
        let target = self::sample(doc, target)?;
        if sample.displaced || target.displaced {
            return Err(invalid("edge wrapping does not support displacement faces"));
        }
        let (anchor, edge) = shared_edge(&sample.vertices, &target.vertices)
            .ok_or_else(|| invalid("edge wrapping requires faces with a shared edge; use projection for parallel surfaces"))?;
        let angle = edge
            .dot(sample.normal.cross(target.normal))
            .atan2(sample.normal.dot(target.normal));
        let turn = DQuat::from_axis_angle(edge, angle);
        for axis in [&mut projection.u, &mut projection.v] {
            let old = axis.axis;
            axis.axis = turn * old;
            axis.offset += anchor.dot(old - axis.axis) / axis.scale;
        }
    }
    edit.projection = Some(projection);
    Ok(edit)
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

pub(crate) fn update(
    side: &mut Side,
    edit: &SurfaceEdit,
    points: &[glam::DVec3],
) -> Result<(), EditError> {
    if let Some(scale) = edit.lightmap_scale {
        side.lightmap_scale = scale;
    }
    if edit.smoothing_groups.is_some() || edit.smoothing_add != 0 || edit.smoothing_remove != 0 {
        side.smoothing_groups = Some(
            (edit
                .smoothing_groups
                .unwrap_or(side.smoothing_groups.unwrap_or(0))
                | edit.smoothing_add)
                & !edit.smoothing_remove,
        );
    }
    let normal = Plane::from_points(side.plane.0)
        .ok_or_else(|| invalid("face has an invalid plane"))?
        .normal;
    if let Some(projection) = &edit.projection {
        side.u_axis = projection.u;
        side.v_axis = projection.v;
        side.rotation = projection.rotation;
    }
    if let Some(alignment) = edit.alignment {
        let (mut u, mut v) = crate::edit::world_texture_axes(normal);
        if alignment == Alignment::Face {
            u = (u - normal * normal.dot(u)).normalize();
            let tangent = normal.cross(u).normalize();
            v = tangent * tangent.dot(v).signum();
        }
        side.u_axis.axis = u;
        side.v_axis.axis = v;
        side.rotation = 0.0;
    }
    if let Some(material) = &edit.material {
        if material.trim().is_empty() {
            return Err(invalid("material must not be empty"));
        }
        side.material = material.trim().to_ascii_uppercase();
    }
    if let Some(rotation) = edit.rotation {
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
    for (axis, justify) in [
        (&mut side.u_axis, edit.justify_u),
        (&mut side.v_axis, edit.justify_v),
    ] {
        if let Some((placement, pixels)) = justify {
            if pixels == 0 {
                return Err(invalid("texture dimensions must be positive"));
            }
            let (lo, hi) = points
                .iter()
                .map(|point| point.dot(axis.axis) / axis.scale + axis.offset)
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), value| {
                    (lo.min(value), hi.max(value))
                });
            axis.offset += match placement {
                Justify::Start => -lo,
                Justify::Center => (f64::from(pixels) - lo - hi) * 0.5,
                Justify::End => f64::from(pixels) - hi,
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
        || !side.rotation.is_finite()
    {
        return Err(invalid(
            "texture projection must have finite axes and nonzero scales",
        ));
    }
    let area = side.u_axis.axis.cross(side.v_axis.axis).dot(normal).abs();
    if area < 1e-8 * side.u_axis.axis.length() * side.v_axis.axis.length() || area < 1e-12 {
        return Err(invalid(
            "texture projection collapses on this face; use face alignment or edge wrapping",
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
    if edit
        .lightmap_scale
        .is_some_and(|scale| !(1..=MAX_LIGHTMAP_SCALE).contains(&scale))
    {
        return Err(invalid(
            "Lightmap scale must be 1–1024 world units per lighting pixel",
        ));
    }
    if edit.smoothing_add & edit.smoothing_remove != 0 {
        return Err(invalid(
            "A smoothing group cannot be added and removed in the same edit",
        ));
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
        if let Some(operation) = &edit.terrain {
            let geometry = crate::brush::Face {
                side: index,
                plane: Plane::from_points(side.plane.0)
                    .ok_or_else(|| invalid("Terrain base plane is degenerate"))?,
                vertices: points,
            };
            let info = side
                .displacement
                .as_ref()
                .ok_or_else(|| invalid("Every target must contain terrain"))?;
            side.displacement = Some(match operation {
                crate::displacement::Operation::Sewn(info) => (**info).clone(),
                crate::displacement::Operation::Collision(settings) => {
                    crate::displacement::collision::apply(info, *settings)
                }
                crate::displacement::Operation::Smoothing(angle) => {
                    crate::displacement::smoothing::edit(info, *angle)?
                }
                crate::displacement::Operation::Resolution(power) => {
                    crate::displacement::resample::change(&geometry, info, *power)?
                }
                crate::displacement::Operation::Sculpt { brush, operation } => {
                    crate::displacement::brush::sculpt(&geometry, info, *brush, *operation)?
                }
                crate::displacement::Operation::Blend { brush, operation } => {
                    crate::displacement::brush::blend(&geometry, info, *brush, *operation)?
                }
            });
        }
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
    fn lighting_edits_preserve_mixed_memberships_neighbors_and_roundtrip() {
        let (mut doc, face) = fixture();
        let other = FaceId {
            solid: face.solid,
            side: doc
                .solid(face.solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| side.id != face.side)
                .unwrap()
                .id,
        };
        for (face, groups) in [(face, vec![1, 25]), (other, vec![2, 32])] {
            doc.edit("initial groups", |tx| {
                edit_faces(
                    tx,
                    &[face],
                    &SurfaceEdit {
                        smoothing_groups: Some(smoothing_mask(&groups)?),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        }
        let before = doc.vmf().clone();
        let patch = SurfaceEdit {
            lightmap_scale: Some(8),
            smoothing_add: smoothing_mask(&[3, 25]).unwrap(),
            smoothing_remove: smoothing_mask(&[1, 32]).unwrap(),
            ..Default::default()
        };
        assert_eq!(
            doc.edit("lighting", |tx| edit_faces(
                tx,
                &[face, other, face],
                &patch
            ))
            .unwrap(),
            2
        );
        assert_eq!(
            smoothing_numbers(side(&doc, face).unwrap().smoothing_groups.unwrap()),
            vec![3, 25]
        );
        assert_eq!(
            smoothing_numbers(side(&doc, other).unwrap().smoothing_groups.unwrap()),
            vec![2, 3, 25]
        );
        for old in &before.world.solids[0].sides {
            let new = doc
                .solid(face.solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| side.id == old.id)
                .unwrap();
            let mut expected = old.clone();
            if [face.side, other.side].contains(&old.id) {
                expected.lightmap_scale = 8;
                expected.smoothing_groups = Some(
                    (old.smoothing_groups.unwrap() | patch.smoothing_add) & !patch.smoothing_remove,
                );
            }
            assert_eq!(*new, expected);
        }
        let after = doc.vmf().clone();
        assert_eq!(
            Vmf::parse(&doc.vmf().write(&WriteOptions::default())).unwrap(),
            after
        );
        doc.undo().unwrap();
        assert_eq!(doc.vmf(), &before);
        doc.redo().unwrap();
        assert_eq!(doc.vmf(), &after);
        doc.edit("clear groups", |tx| {
            edit_faces(
                tx,
                &[face, other],
                &SurfaceEdit {
                    smoothing_groups: Some(0),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        assert_eq!(side(&doc, face).unwrap().smoothing_groups, Some(0));
        assert_eq!(side(&doc, other).unwrap().smoothing_groups, Some(0));
    }

    #[test]
    fn lighting_validation_rejects_bad_batches_without_history_or_metadata_changes() {
        let (mut doc, face) = fixture();
        assert_eq!(smoothing_mask(&[1, 1, 24, 25, 32]).unwrap(), 0x8180_0001);
        assert_eq!(
            smoothing_mask(&[25, 26, 27, 28, 29, 30, 31, 32]).unwrap(),
            HARD_EDGE_GROUPS
        );
        assert!(smoothing_mask(&[0]).is_err());
        assert!(smoothing_mask(&[33]).is_err());
        let before = doc.vmf().clone();
        let revision = doc.revision();
        for patch in [
            SurfaceEdit {
                lightmap_scale: Some(0),
                ..Default::default()
            },
            SurfaceEdit {
                lightmap_scale: Some(1025),
                ..Default::default()
            },
            SurfaceEdit {
                smoothing_add: 1,
                smoothing_remove: 1,
                ..Default::default()
            },
        ] {
            assert!(
                doc.edit("invalid lighting", |tx| edit_faces(tx, &[face], &patch))
                    .is_err()
            );
            assert_eq!(doc.vmf(), &before);
            assert_eq!(doc.revision(), revision);
        }
        assert!(
            doc.edit("invalid target", |tx| edit_faces(
                tx,
                &[
                    face,
                    FaceId {
                        solid: face.solid,
                        side: u32::MAX
                    }
                ],
                &SurfaceEdit {
                    lightmap_scale: Some(4),
                    ..Default::default()
                }
            ))
            .is_err()
        );
        assert_eq!(doc.vmf(), &before);
        assert_eq!(doc.revision(), revision);
    }

    #[test]
    fn authored_lightmap_coordinates_follow_uv_axes_and_texture_shift() {
        let (doc, face) = fixture();
        let mut side = side(&doc, face).unwrap().clone();
        side.u_axis.axis = DVec3::new(2.0, 0.5, 0.0);
        side.u_axis.scale = -0.25;
        side.u_axis.offset = 12.0;
        side.v_axis.axis = DVec3::new(0.25, -1.0, 0.0);
        side.v_axis.scale = 0.5;
        side.v_axis.offset = 4.0;
        side.lightmap_scale = 8;
        assert_eq!(
            lightmap_uv(&side, DVec3::new(16.0, 24.0, 16.0)),
            Some(DVec2::new(41.0 / 8.0, -18.0 / 8.0))
        );
        side.lightmap_scale = 0;
        assert_eq!(lightmap_uv(&side, DVec3::ZERO), None);
        side.lightmap_scale = 8;
        side.u_axis.axis = DVec3::Z;
        assert_eq!(lightmap_uv(&side, DVec3::ZERO), None);
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

    fn uv_bounds(doc: &Document, face: FaceId) -> (DVec2, DVec2) {
        face_vertices(doc, face)
            .unwrap()
            .into_iter()
            .map(|point| texel_uv(side(doc, face).unwrap(), point))
            .fold(
                (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
                |(lo, hi), uv| (lo.min(uv), hi.max(uv)),
            )
    }

    #[test]
    fn justify_preserves_scale_and_handles_rotated_mirrored_axes() {
        for placement in [Justify::Start, Justify::Center, Justify::End] {
            let (mut doc, face) = fixture();
            doc.edit("justify", |tx| {
                edit_faces(
                    tx,
                    &[face],
                    &SurfaceEdit {
                        rotation: Some(31.0),
                        scale_u: Some(-0.75),
                        scale_v: Some(0.5),
                        shift_u: Some(999.0),
                        shift_v: Some(-999.0),
                        justify_u: Some((placement, 256)),
                        justify_v: Some((placement, 128)),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
            let (lo, hi) = uv_bounds(&doc, face);
            match placement {
                Justify::Start => assert!(lo.abs_diff_eq(DVec2::ZERO, 1e-8)),
                Justify::Center => {
                    assert!(((lo + hi) * 0.5).abs_diff_eq(DVec2::new(128.0, 64.0), 1e-8))
                }
                Justify::End => assert!(hi.abs_diff_eq(DVec2::new(256.0, 128.0), 1e-8)),
            }
            assert_eq!(side(&doc, face).unwrap().u_axis.scale, -0.75);
            assert_eq!(side(&doc, face).unwrap().v_axis.scale, 0.5);
        }
    }

    #[test]
    fn edge_wrap_matches_texels_along_the_entire_seam_and_roundtrips() {
        let (mut doc, face) = fixture();
        doc.edit("source", |tx| {
            edit_faces(
                tx,
                &[face],
                &SurfaceEdit {
                    rotation: Some(23.0),
                    scale_u: Some(-0.4),
                    scale_v: Some(0.7),
                    shift_u: Some(17.0),
                    shift_v: Some(-31.0),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        let source = sample(&doc, face).unwrap();
        let target = FaceId {
            solid: face.solid,
            side: doc
                .solid(face.solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| Plane::from_points(side.plane.0).unwrap().normal == DVec3::NEG_Y)
                .unwrap()
                .id,
        };
        let original = doc.vmf().clone();
        let patch = paint_edit(&doc, target, &source, PaintMode::Wrap).unwrap();
        doc.edit("wrap", |tx| edit_faces(tx, &[target], &patch))
            .unwrap();
        for x in [0.0, 31.0, 64.0, 128.0] {
            let point = DVec3::new(x, 0.0, 16.0);
            assert!(
                texel_uv(side(&doc, face).unwrap(), point)
                    .abs_diff_eq(texel_uv(side(&doc, target).unwrap(), point), 1e-8)
            );
        }
        assert_eq!(side(&doc, target).unwrap().u_axis.scale, -0.4);
        let reopened =
            Document::new(Vmf::parse(&doc.vmf().write(&WriteOptions::default())).unwrap());
        for point in face_vertices(&doc, target).unwrap() {
            assert!(
                texel_uv(side(&doc, target).unwrap(), point)
                    .abs_diff_eq(texel_uv(side(&reopened, target).unwrap(), point), 1e-5)
            );
        }
        doc.undo().unwrap();
        assert_eq!(doc.vmf(), &original);
        doc.redo().unwrap();
        assert_eq!(side(&doc, face).unwrap().u_axis, source.projection.u);
    }

    #[test]
    fn shared_edges_can_overlap_partially_but_corner_contact_is_not_a_seam() {
        let square = [
            DVec3::ZERO,
            DVec3::X * 8.0,
            DVec3::new(8.0, 8.0, 0.0),
            DVec3::Y * 8.0,
        ];
        let partial = [
            DVec3::new(2.0, 0.0, 0.0),
            DVec3::new(5.0, 0.0, 0.0),
            DVec3::new(5.0, -4.0, 0.0),
        ];
        assert!(shared_edge(&square, &partial).is_some());
        let corner = [
            DVec3::new(8.0, 0.0, 0.0),
            DVec3::new(9.0, 0.0, 0.0),
            DVec3::new(9.0, -4.0, 0.0),
        ];
        assert!(shared_edge(&square, &corner).is_none());
    }

    #[test]
    fn invalid_projection_and_wrap_reject_without_modifying_document() {
        let (mut doc, face) = fixture();
        let source = sample(&doc, face).unwrap();
        let wall = FaceId {
            solid: face.solid,
            side: doc
                .solid(face.solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| Plane::from_points(side.plane.0).unwrap().normal == DVec3::NEG_Y)
                .unwrap()
                .id,
        };
        let bottom = FaceId {
            solid: face.solid,
            side: doc
                .solid(face.solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| Plane::from_points(side.plane.0).unwrap().normal == DVec3::NEG_Z)
                .unwrap()
                .id,
        };
        let patch = paint_edit(&doc, wall, &source, PaintMode::Projection).unwrap();
        let original = doc.vmf().clone();
        assert!(
            doc.edit("collapsed", |tx| edit_faces(tx, &[face, wall], &patch))
                .is_err()
        );
        assert_eq!(doc.vmf(), &original);
        assert!(paint_edit(&doc, bottom, &source, PaintMode::Wrap).is_err());
        let mut displaced = source.clone();
        displaced.displaced = true;
        assert!(paint_edit(&doc, wall, &displaced, PaintMode::Wrap).is_err());
        let patch = paint_edit(&doc, wall, &source, PaintMode::Material).unwrap();
        doc.edit("material", |tx| edit_faces(tx, &[wall], &patch))
            .unwrap();
        assert_eq!(doc.vmf(), &original);
    }

    #[test]
    fn face_alignment_is_tangent_while_world_alignment_uses_cardinal_axes() {
        let (mut doc, face) = fixture();
        let turn = DQuat::from_rotation_x(0.4) * DQuat::from_rotation_y(0.2);
        doc.edit("slope", |tx| {
            for side in &mut tx.solid_mut(face.solid)?.sides {
                for point in &mut side.plane.0 {
                    *point = turn * *point;
                }
            }
            Ok(())
        })
        .unwrap();
        doc.edit("face", |tx| {
            edit_faces(
                tx,
                &[face],
                &SurfaceEdit {
                    alignment: Some(Alignment::Face),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        let result = side(&doc, face).unwrap();
        let normal = Plane::from_points(result.plane.0).unwrap().normal;
        assert!(normal.dot(result.u_axis.axis).abs() < 1e-8);
        assert!(normal.dot(result.v_axis.axis).abs() < 1e-8);
        assert!(result.u_axis.axis.dot(result.v_axis.axis).abs() < 1e-8);
        assert!((result.u_axis.axis.length() - 1.0).abs() < 1e-8);
        doc.edit("world", |tx| {
            edit_faces(
                tx,
                &[face],
                &SurfaceEdit {
                    alignment: Some(Alignment::World),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        let result = side(&doc, face).unwrap();
        assert_eq!(
            (result.u_axis.axis, result.v_axis.axis),
            crate::edit::world_texture_axes(normal)
        );
    }
}
