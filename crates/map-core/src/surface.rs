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
    pub u_axis: Option<[Option<f64>; 3]>,
    pub v_axis: Option<[Option<f64>; 3]>,
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

pub fn texture_tangents(
    normal: DVec3,
    s: DVec3,
    left_handed: bool,
) -> Result<(DVec3, DVec3, DVec3), String> {
    let normal = normal
        .try_normalize()
        .ok_or("Normal-map lighting needs a finite nonzero sample normal")?;
    let bitangent = normal.cross(s).try_normalize().ok_or("Normal-map lighting tangent is parallel to its sample normal; adjust face mapping or smoothing")?;
    let tangent = bitangent.cross(normal).normalize();
    let bitangent = if left_handed { -bitangent } else { bitangent };
    Ok((tangent, bitangent, normal))
}

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
    for (axis, components) in [
        (&mut side.u_axis.axis, edit.u_axis),
        (&mut side.v_axis.axis, edit.v_axis),
    ] {
        if let Some(components) = components {
            for (index, value) in components.into_iter().enumerate() {
                if let Some(value) = value {
                    axis[index] = value;
                }
            }
        }
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
        source_formats::keyvalues::validate_string(material)
            .map_err(|error| invalid(&error.to_string()))?;
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
    let projection = placed_projection(
        &Projection {
            u: side.u_axis,
            v: side.v_axis,
            rotation: side.rotation,
        },
        points,
        edit.fit,
        [edit.justify_u, edit.justify_v],
    )?;
    side.u_axis = projection.u;
    side.v_axis = projection.v;
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
    let direct = edit.u_axis.is_some() || edit.v_axis.is_some();
    let [u, v] = [side.u_axis, side.v_axis].map(|axis| {
        if direct {
            let vector = axis.axis / axis.scale;
            vector / vector.abs().max_element()
        } else {
            axis.axis
        }
    });
    let area = u.cross(v).dot(normal).abs();
    if !area.is_finite() || area < 1e-8 * u.length() * v.length() || (!direct && area < 1e-12) {
        return Err(invalid(
            "texture projection collapses on this face; use face alignment or edge wrapping",
        ));
    }
    if direct
        && points
            .iter()
            .any(|point| !crate::texel_uv(side, *point).is_finite())
    {
        return Err(invalid(
            "texture vectors produce non-finite texel coordinates on this face",
        ));
    }
    Ok(())
}

pub fn placed_projection(
    projection: &Projection,
    points: &[DVec3],
    fit: Option<[u32; 2]>,
    justify: [Option<(Justify, u32)>; 2],
) -> Result<Projection, EditError> {
    if fit.is_none() && justify.iter().all(Option::is_none) {
        return Ok(projection.clone());
    }
    if points.len() < 3 || points.iter().any(|point| !point.is_finite()) {
        return Err(invalid("Texture placement needs a finite polygon loop"));
    }
    if fit.is_some_and(|size| size.contains(&0))
        || justify.iter().flatten().any(|(_, pixels)| *pixels == 0)
    {
        return Err(invalid("texture dimensions must be positive"));
    }
    let mut result = projection.clone();
    for (index, axis) in [&mut result.u, &mut result.v].into_iter().enumerate() {
        if fit.is_none() && justify[index].is_none() {
            continue;
        }
        let (lo, hi) = points
            .iter()
            .map(|point| point.dot(axis.axis))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), value| {
                (lo.min(value), hi.max(value))
            });
        if !lo.is_finite() || !hi.is_finite() || hi - lo < 1e-8 {
            return Err(invalid("texture axis does not span the face"));
        }
        if let Some(size) = fit {
            let sign = axis.scale.signum();
            axis.scale = sign * (hi - lo) / f64::from(size[index]);
            axis.offset = if sign > 0.0 {
                -lo / axis.scale
            } else {
                -hi / axis.scale
            };
        }
        if !axis.scale.is_finite() || axis.scale.abs() < 1e-8 || !axis.offset.is_finite() {
            return Err(invalid(
                "texture projection must have finite axes and nonzero scales",
            ));
        }
        if let Some((placement, pixels)) = justify[index] {
            let a = lo / axis.scale + axis.offset;
            let b = hi / axis.scale + axis.offset;
            let (lo, hi) = (a.min(b), a.max(b));
            axis.offset += match placement {
                Justify::Start => -lo,
                Justify::Center => (f64::from(pixels) - lo - hi) * 0.5,
                Justify::End => f64::from(pixels) - hi,
            };
        }
    }
    Ok(result)
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
    if edit.u_axis.is_some() || edit.v_axis.is_some() {
        if edit.alignment.is_some() || edit.projection.is_some() || edit.rotation.is_some() {
            return Err(invalid(
                "Direct texture vectors cannot be combined with alignment, copied projection or rotation",
            ));
        }
        if [edit.u_axis, edit.v_axis]
            .into_iter()
            .flatten()
            .flatten()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(invalid("texture vector components must be finite"));
        }
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
                crate::displacement::Operation::Sewn(info)
                | crate::displacement::Operation::Subdivided(info) => (**info).clone(),
                crate::displacement::Operation::Masked {
                    operation,
                    vertices,
                } => crate::displacement::brush::masked(&geometry, info, operation, vertices)?,
                crate::displacement::Operation::Collision(settings) => {
                    crate::displacement::collision::apply(info, *settings)
                }
                crate::displacement::Operation::Smoothing(angle) => {
                    crate::displacement::smoothing::edit(info, *angle)?
                }
                crate::displacement::Operation::TriangleTags {
                    triangles,
                    settings,
                } => {
                    let mesh = crate::DisplacementMesh::build(&geometry, info)
                        .map_err(|error| invalid(&error.to_string()))?;
                    crate::displacement::triangles::edit(info, &mesh, triangles, *settings)?
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
                crate::displacement::Operation::Stroke { spec, points } => {
                    crate::displacement::stroke::apply(&geometry, info, *spec, points, None)?
                }
                crate::displacement::Operation::VertexMove { vertices, offset } => {
                    crate::displacement::brush::move_vertices(&geometry, info, vertices, *offset)?
                }
                crate::displacement::Operation::VertexSet { vertices } => {
                    crate::displacement::brush::set_vertices(&geometry, info, vertices)?
                }
            });
        }
        changed += usize::from(*side != before);
    }
    Ok(changed)
}
