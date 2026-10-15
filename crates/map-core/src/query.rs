//! Read-only queries: picking and bounds.

mod terrain;
pub use terrain::intersects_box as ray_intersects_bounds;

use glam::DVec3;
use source_formats::vmf::{Entity, Solid};

use crate::document::{Document, ObjectId};
use crate::{BrushGeometry, Plane};

/// Half the edge length of the box that represents a point entity.
pub const POINT_ENTITY_HALF_SIZE: f64 = 8.0;
pub const LIGHT_ENTITY_HALF_SIZE: f64 = 18.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: DVec3,
    pub direction: DVec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub object: ObjectId,
    /// The brush hit, if any (none for point entities).
    pub solid: Option<u32>,
    /// The side id the ray entered through.
    pub side: Option<u32>,
    pub distance: f64,
    pub point: DVec3,
}

pub fn ray_triangle(ray: &Ray, positions: [DVec3; 3]) -> Option<f64> {
    let [a, b, c] = positions;
    terrain::triangle(ray, a, b, c)
}

pub fn pick_displacement_triangle(
    ray: &Ray,
    mesh: &crate::DisplacementMesh,
) -> Option<(usize, f64)> {
    if !ray.origin.is_finite()
        || !ray.direction.is_finite()
        || ![5, 9, 17].contains(&mesh.size)
        || mesh.positions.len() != mesh.size * mesh.size
    {
        return None;
    }
    let indices = crate::displacement::triangles::indices((mesh.size - 1).ilog2()).ok()?;
    let mut nearest: Option<(usize, f64)> = None;
    for (id, [a, b, c]) in indices.into_iter().enumerate() {
        let distance =
            terrain::triangle(ray, mesh.positions[a], mesh.positions[b], mesh.positions[c]);
        if let Some(distance) = distance
            && nearest.is_none_or(|(_, old)| distance < old - 1e-9)
        {
            nearest = Some((id, distance));
        }
    }
    nearest
}

/// Entry distance and side index of a ray into a convex brush, ignoring
/// brushes that contain the ray origin.
fn ray_solid(ray: &Ray, solid: &Solid) -> Option<(f64, usize)> {
    let (mut enter, mut exit, mut enter_side) = (f64::NEG_INFINITY, f64::INFINITY, None);
    for (i, side) in solid.sides.iter().enumerate() {
        let plane = Plane::from_points(side.plane.0)?;
        let denom = plane.normal.dot(ray.direction);
        let dist = plane.distance_to(ray.origin);
        if denom.abs() < 1e-12 {
            if dist > 0.0 {
                return None;
            }
            continue;
        }
        let t = -dist / denom;
        if denom < 0.0 {
            if t > enter {
                enter = t;
                enter_side = Some(i);
            }
        } else {
            exit = exit.min(t);
        }
    }
    (enter >= 0.0 && enter <= exit).then_some((enter, enter_side?))
}

fn ray_box(ray: &Ray, mins: DVec3, maxs: DVec3) -> Option<f64> {
    let inv = ray.direction.recip();
    let t0 = (mins - ray.origin) * inv;
    let t1 = (maxs - ray.origin) * inv;
    let enter = t0.min(t1).max_element();
    let exit = t0.max(t1).min_element();
    (enter >= 0.0 && enter <= exit).then_some(enter)
}

pub fn point_entity_bounds(origin: DVec3) -> (DVec3, DVec3) {
    let h = DVec3::splat(POINT_ENTITY_HALF_SIZE);
    (origin - h, origin + h)
}

/// The nearest visible object along the ray. `include_solid` filters brushes,
/// e.g. to skip tool brushes while they're hidden.
pub fn pick(doc: &Document, ray: &Ray, include_solid: impl Fn(&Solid) -> bool) -> Option<Hit> {
    pick_with_entity_filter(doc, ray, include_solid, |_| true)
}

pub fn pick_with_entity_filter(
    doc: &Document,
    ray: &Ray,
    include_solid: impl Fn(&Solid) -> bool,
    include_entity: impl Fn(&Entity) -> bool,
) -> Option<Hit> {
    pick_with_points(
        doc,
        ray,
        include_solid,
        |_, _, _| true,
        true,
        include_entity,
    )
}

pub fn pick_face(doc: &Document, ray: &Ray, include_solid: impl Fn(&Solid) -> bool) -> Option<Hit> {
    pick_with_points(doc, ray, include_solid, |_, _, _| true, false, |_| true)
}

pub fn pick_face_filtered(
    doc: &Document,
    ray: &Ray,
    include_solid: impl Fn(&Solid) -> bool,
    include_face: impl Fn(&Solid, &source_formats::vmf::Side, DVec3) -> bool,
) -> Option<Hit> {
    pick_with_points(doc, ray, include_solid, include_face, false, |_| true)
}

fn pick_with_points(
    doc: &Document,
    ray: &Ray,
    include_solid: impl Fn(&Solid) -> bool,
    include_face: impl Fn(&Solid, &source_formats::vmf::Side, DVec3) -> bool,
    include_points: bool,
    include_entity: impl Fn(&Entity) -> bool,
) -> Option<Hit> {
    if !ray.origin.is_finite()
        || !ray.direction.is_finite()
        || ray.direction.length_squared() < 1e-24
    {
        return None;
    }
    let ray = Ray {
        origin: ray.origin,
        direction: ray.direction.normalize(),
    };
    let test_solid = |object, solid: &Solid| {
        if solid.hidden || !include_solid(solid) {
            return None;
        }
        let displaced = solid.sides.iter().any(|side| side.displacement.is_some());
        let (t, side) = if displaced {
            terrain::pick(&ray, solid, &include_face)?
        } else {
            ray_solid(&ray, solid)?
        };
        if !displaced && !include_face(solid, &solid.sides[side], ray.origin + ray.direction * t) {
            return None;
        }
        Some(Hit {
            object,
            solid: Some(solid.id),
            side: Some(solid.sides[side].id),
            distance: t,
            point: ray.origin + ray.direction * t,
        })
    };
    let test_point_entity = |entity: &Entity| {
        if !include_points || !entity.solids.is_empty() || !include_entity(entity) {
            return None;
        }
        let (mins, maxs) = entity_bounds(entity)?;
        let t = ray_box(&ray, mins, maxs)?;
        Some(Hit {
            object: ObjectId::Entity(entity.id),
            solid: None,
            side: None,
            distance: t,
            point: ray.origin + ray.direction * t,
        })
    };

    let world = doc
        .vmf()
        .world
        .solids
        .iter()
        .filter_map(|s| test_solid(ObjectId::Solid(s.id), s));
    let entities = doc
        .vmf()
        .entities
        .iter()
        .filter(|e| !e.hidden)
        .flat_map(|e| {
            let id = ObjectId::Entity(e.id);
            e.solids
                .iter()
                .filter_map(move |s| test_solid(id, s))
                .chain(test_point_entity(e))
        });
    world
        .chain(entities)
        .min_by(|a, b| a.distance.total_cmp(&b.distance))
}

pub fn face_bounds(solid: &Solid, side_id: u32) -> Option<(DVec3, DVec3)> {
    let geometry = BrushGeometry::from_solid(solid).ok()?;
    let index = solid.sides.iter().position(|side| side.id == side_id)?;
    let face = geometry.faces.iter().find(|face| face.side == index)?;
    let points = if let Some(info) = &solid.sides[index].displacement {
        crate::DisplacementMesh::build(face, info).ok()?.positions
    } else {
        face.vertices.clone()
    };
    if points.iter().any(|point| !point.is_finite()) {
        return None;
    }
    union(points.into_iter().map(|point| (point, point)))
}

pub fn solid_bounds(solid: &Solid) -> Option<(DVec3, DVec3)> {
    if solid.sides.iter().any(|side| side.displacement.is_some()) {
        return union(
            terrain::points(solid)
                .into_iter()
                .map(|point| (point, point)),
        );
    }
    let brush = BrushGeometry::from_solid(solid).ok()?;
    Some((brush.mins, brush.maxs))
}

pub fn entity_bounds(entity: &Entity) -> Option<(DVec3, DVec3)> {
    if entity.solids.is_empty() {
        if matches!(
            entity
                .classname()
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str(),
            "light" | "light_spot" | "light_dynamic" | "light_environment"
        ) {
            let origin = entity.origin()?;
            let half = DVec3::splat(LIGHT_ENTITY_HALF_SIZE);
            return Some((origin - half, origin + half));
        }
        return entity.origin().map(point_entity_bounds);
    }
    union(entity.solids.iter().filter_map(solid_bounds))
}

pub fn bounds(doc: &Document, id: ObjectId) -> Option<(DVec3, DVec3)> {
    match id {
        ObjectId::Solid(id) => solid_bounds(doc.solid(id)?),
        ObjectId::Entity(id) => entity_bounds(doc.entity(id)?),
    }
}

pub fn union(boxes: impl IntoIterator<Item = (DVec3, DVec3)>) -> Option<(DVec3, DVec3)> {
    boxes
        .into_iter()
        .reduce(|(a0, a1), (b0, b1)| (a0.min(b0), a1.max(b1)))
}

pub fn bounds_of(doc: &Document, ids: &[ObjectId]) -> Option<(DVec3, DVec3)> {
    union(ids.iter().filter_map(|&id| bounds(doc, id)))
}

pub fn local_basis(doc: &Document, id: ObjectId) -> Option<glam::DMat3> {
    let solid = match id {
        ObjectId::Solid(id) => doc.solid(id)?,
        ObjectId::Entity(id) => {
            let entity = doc.entity(id)?;
            if entity.solids.is_empty() {
                let mut angles = entity
                    .properties
                    .get("angles")
                    .and_then(source_formats::vmf::parse_vec3)
                    .unwrap_or(DVec3::ZERO);
                if entity.properties.get("angles").is_none()
                    && let Some(angle) = entity
                        .properties
                        .get("angle")
                        .and_then(|value| value.parse::<f64>().ok())
                {
                    angles = match angle {
                        -1.0 => DVec3::new(-90.0, 0.0, 0.0),
                        -2.0 => DVec3::new(90.0, 0.0, 0.0),
                        _ => DVec3::new(0.0, angle, 0.0),
                    };
                }
                if matches!(entity.classname(), Some("light_spot" | "light_environment"))
                    && let Some(pitch) = entity
                        .properties
                        .get("pitch")
                        .and_then(|value| value.parse::<f64>().ok())
                {
                    angles.x = -pitch;
                }
                if !angles.is_finite() {
                    return None;
                }
                let radians = angles.map(f64::to_radians);
                return Some(glam::DMat3::from_quat(glam::DQuat::from_euler(
                    glam::EulerRot::ZYX,
                    radians.y,
                    radians.x,
                    radians.z,
                )));
            }
            &entity.solids[0]
        }
    };
    let points = solid.sides.first()?.plane.0;
    let z = Plane::from_points(points)?.normal;
    let edge = points[1] - points[0];
    let x = (edge - z * edge.dot(z)).try_normalize()?;
    let y = z.cross(x).try_normalize()?;
    Some(glam::DMat3::from_cols(x, y, z))
}

pub fn bounds_in_basis(
    doc: &Document,
    ids: &[ObjectId],
    basis: glam::DMat3,
) -> Option<(DVec3, DVec3)> {
    let inverse = basis.transpose();
    let mut points = Vec::new();
    for id in ids {
        let solids: Vec<_> = match *id {
            ObjectId::Solid(id) => vec![doc.solid(id)?],
            ObjectId::Entity(id) => {
                let entity = doc.entity(id)?;
                if entity.solids.is_empty() {
                    let (mins, maxs) = entity_bounds(entity)?;
                    points.extend((0..8).map(|index| {
                        DVec3::new(
                            if index & 1 == 0 { mins.x } else { maxs.x },
                            if index & 2 == 0 { mins.y } else { maxs.y },
                            if index & 4 == 0 { mins.z } else { maxs.z },
                        )
                    }));
                }
                entity.solids.iter().collect()
            }
        };
        for solid in solids {
            if solid.sides.iter().any(|side| side.displacement.is_some()) {
                points.extend(terrain::points(solid));
            } else {
                let shape = BrushGeometry::from_solid(solid).ok()?;
                points.extend(shape.faces.into_iter().flat_map(|face| face.vertices));
            }
        }
    }
    union(points.into_iter().map(|point| {
        let point = inverse * point;
        (point, point)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{create_block, create_entity};
    use crate::surface::FaceId;

    #[test]
    fn local_bounds_use_brush_vertices_after_oblique_rotation() {
        let mut doc = Document::empty();
        let id = doc
            .edit("fixture", |tx| {
                create_block(tx, DVec3::ZERO, DVec3::new(64.0, 32.0, 16.0), "test")
            })
            .unwrap();
        doc.edit("rotate", |tx| {
            crate::edit::transform(
                tx,
                &[id],
                crate::edit::Transform {
                    rotation: DVec3::new(0.0, 0.0, 45.0),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        let basis = local_basis(&doc, id).unwrap();
        let (mins, maxs) = bounds_in_basis(&doc, &[id], basis).unwrap();
        assert!((maxs - mins).abs_diff_eq(DVec3::new(64.0, 32.0, 16.0), 1e-8));
        let (mins, maxs) = bounds_of(&doc, &[id]).unwrap();
        assert!((maxs - mins).x > 64.0);
    }

    #[test]
    fn picks_the_nearest_object_and_entry_face() {
        let mut doc = Document::empty();
        let (near, far, ent) = doc
            .edit("setup", |tx| {
                let near = create_block(
                    tx,
                    DVec3::new(100.0, -32.0, -32.0),
                    DVec3::new(164.0, 32.0, 32.0),
                    "a",
                )?;
                let far = create_block(
                    tx,
                    DVec3::new(300.0, -32.0, -32.0),
                    DVec3::new(364.0, 32.0, 32.0),
                    "a",
                )?;
                let ent = create_entity(
                    tx,
                    "info_target",
                    Some(DVec3::new(50.0, 0.0, 100.0)),
                    &[],
                    vec![],
                )?;
                Ok((near, far, ent))
            })
            .unwrap();
        let along_x = Ray {
            origin: DVec3::ZERO,
            direction: DVec3::X,
        };
        let hit = pick(&doc, &along_x, |_| true).unwrap();
        assert_eq!(hit.object, near);
        assert!((hit.distance - 100.0).abs() < 1e-9);
        let side = doc.solid(match near {
            ObjectId::Solid(id) => id,
            _ => unreachable!(),
        });
        let entered = side
            .unwrap()
            .sides
            .iter()
            .find(|s| Some(s.id) == hit.side)
            .unwrap();
        let normal = Plane::from_points(entered.plane.0).unwrap().normal;
        assert_eq!(normal, DVec3::NEG_X);

        let skip_near = pick(&doc, &along_x, |s| ObjectId::Solid(s.id) != near).unwrap();
        assert_eq!(skip_near.object, far);
        let skip_entry = pick_face_filtered(
            &doc,
            &along_x,
            |_| true,
            |_, side, point| side.id != entered.id && point.x >= 0.0,
        )
        .unwrap();
        assert_eq!(skip_entry.object, far);
        assert!(pick_face_filtered(&doc, &along_x, |_| true, |_, _, _| false).is_none());

        let up = Ray {
            origin: DVec3::new(50.0, 0.0, 0.0),
            direction: DVec3::Z,
        };
        assert_eq!(pick(&doc, &up, |_| true).unwrap().object, ent);
        let blocker = doc
            .edit("helper", |tx| {
                create_entity(tx, "light", Some(DVec3::new(50.0, 0.0, 0.0)), &[], vec![])
            })
            .unwrap();
        assert_eq!(pick(&doc, &along_x, |_| true).unwrap().object, blocker);
        assert_eq!(pick_face(&doc, &along_x, |_| true).unwrap().object, near);
    }

    #[test]
    fn brushes_containing_the_camera_are_ignored() {
        let mut doc = Document::empty();
        doc.edit("block", |tx| {
            create_block(tx, DVec3::splat(-64.0), DVec3::splat(64.0), "a")
        })
        .unwrap();
        let ray = Ray {
            origin: DVec3::ZERO,
            direction: DVec3::X,
        };
        assert_eq!(pick(&doc, &ray, |_| true), None);
    }

    #[test]
    fn light_helpers_can_be_picked_outside_the_old_cube() {
        let mut doc = Document::empty();
        let id = doc
            .edit("light", |tx| {
                create_entity(tx, "light_spot", Some(DVec3::ZERO), &[], vec![])
            })
            .unwrap();
        let ray = Ray {
            origin: DVec3::new(15.0, -100.0, 0.0),
            direction: DVec3::Y,
        };
        assert_eq!(pick(&doc, &ray, |_| true).unwrap().object, id);
        assert_eq!(
            bounds(&doc, id),
            Some((
                DVec3::splat(-LIGHT_ENTITY_HALF_SIZE),
                DVec3::splat(LIGHT_ENTITY_HALF_SIZE)
            ))
        );
    }
    fn raised_terrain() -> (Document, FaceId) {
        use crate::surface::FaceId;
        let mut doc = Document::empty();
        let ObjectId::Solid(id) = doc
            .edit("parent", |tx| {
                create_block(
                    tx,
                    DVec3::new(-64.0, -64.0, 0.0),
                    DVec3::new(64.0, 64.0, 16.0),
                    "brick/test",
                )
            })
            .unwrap()
        else {
            unreachable!()
        };
        let face = FaceId {
            solid: id,
            side: doc
                .solid(id)
                .unwrap()
                .sides
                .iter()
                .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id,
        };
        doc.edit("terrain", |tx| {
            crate::displacement::edit::create(tx, &[face], 2, 0)
        })
        .unwrap();
        doc.edit("raise", |tx| {
            crate::surface::edit_faces(
                tx,
                &[face],
                &crate::surface::SurfaceEdit {
                    terrain: Some(crate::displacement::Operation::Sculpt {
                        brush: crate::displacement::brush::Brush {
                            center: DVec3::new(0.0, 0.0, 16.0),
                            radius: 32.0,
                            axis: crate::displacement::brush::Axis::WorldZ,
                        },
                        operation: crate::displacement::brush::Sculpt::Raise(64.0),
                    }),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        (doc, face)
    }

    #[test]
    fn displaced_peaks_drive_surface_hits_and_world_local_bounds() {
        let (doc, face) = raised_terrain();
        let ray = Ray {
            origin: DVec3::new(0.0, 0.0, 200.0),
            direction: DVec3::NEG_Z,
        };
        let hit = pick_face(&doc, &ray, |_| true).unwrap();
        assert_eq!(hit.point, DVec3::new(0.0, 0.0, 80.0));
        assert_eq!(hit.side, Some(face.side));
        let id = ObjectId::Solid(face.solid);
        assert_eq!(
            bounds(&doc, id),
            Some((DVec3::new(-64.0, -64.0, 16.0), DVec3::new(64.0, 64.0, 80.0)))
        );
        assert_eq!(
            bounds_in_basis(&doc, &[id], glam::DMat3::IDENTITY),
            bounds(&doc, id)
        );
        assert!(pick_face_filtered(&doc, &ray, |_| true, |_, _, _| false).is_none());
        assert!(pick_face(&doc, &ray, |_| false).is_none());
    }

    #[test]
    fn terrain_offsets_entity_owners_and_hidden_scopes_keep_rendered_identity() {
        let (mut doc, face) = raised_terrain();
        doc.edit("offset", |tx| {
            let side = tx
                .solid_mut(face.solid)?
                .sides
                .iter_mut()
                .find(|side| side.id == face.side)
                .unwrap();
            for row in side
                .displacement
                .as_mut()
                .unwrap()
                .offsets
                .as_mut()
                .unwrap()
            {
                for point in row {
                    *point += DVec3::X * 200.0;
                }
            }
            crate::edit::tie_to_entity(tx, &[face.solid], "func_brush", &[])
        })
        .unwrap();
        let owner = doc.owner_of_solid(face.solid).unwrap();
        let ray = Ray {
            origin: DVec3::new(200.0, 0.0, 200.0),
            direction: DVec3::NEG_Z,
        };
        let hit = pick_face(&doc, &ray, |_| true).unwrap();
        assert_eq!(hit.object, owner);
        assert_eq!(hit.point, DVec3::new(200.0, 0.0, 80.0));
        assert_eq!(bounds(&doc, owner).unwrap().0.x, 136.0);
        let miss = Ray {
            origin: DVec3::new(0.0, 0.0, 200.0),
            direction: DVec3::NEG_Z,
        };
        assert!(pick_face(&doc, &miss, |_| true).is_none());
        doc.edit("hidden", |tx| {
            let ObjectId::Entity(entity) = owner else {
                unreachable!()
            };
            tx.entity_mut(entity)?
                .solids
                .iter_mut()
                .find(|solid| solid.id == face.solid)
                .unwrap()
                .hidden = true;
            Ok(())
        })
        .unwrap();
        assert!(pick_face(&doc, &ray, |_| true).is_none());
    }

    #[test]
    fn malformed_terrain_does_not_fall_back_to_invisible_parent_faces() {
        let (mut doc, face) = raised_terrain();
        doc.edit("malformed", |tx| {
            tx.solid_mut(face.solid)?
                .sides
                .iter_mut()
                .find(|side| side.id == face.side)
                .unwrap()
                .displacement
                .as_mut()
                .unwrap()
                .power = 5;
            Ok(())
        })
        .unwrap();
        let ray = Ray {
            origin: DVec3::new(0.0, 0.0, 200.0),
            direction: DVec3::NEG_Z,
        };
        assert!(pick(&doc, &ray, |_| true).is_none());
        assert!(bounds(&doc, ObjectId::Solid(face.solid)).is_none());
    }
}
