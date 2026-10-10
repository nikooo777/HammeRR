//! Read-only queries: picking and bounds.

use glam::DVec3;
use source_formats::vmf::{Entity, Solid};

use crate::document::{Document, ObjectId};
use crate::{BrushGeometry, Plane};

/// Half the edge length of the box that represents a point entity.
pub const POINT_ENTITY_HALF_SIZE: f64 = 8.0;

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
    let ray = Ray {
        origin: ray.origin,
        direction: ray.direction.normalize(),
    };
    let test_solid = |object, solid: &Solid| {
        if solid.hidden || !include_solid(solid) {
            return None;
        }
        let (t, side) = ray_solid(&ray, solid)?;
        Some(Hit {
            object,
            solid: Some(solid.id),
            side: Some(solid.sides[side].id),
            distance: t,
            point: ray.origin + ray.direction * t,
        })
    };
    let test_point_entity = |entity: &Entity| {
        if !entity.solids.is_empty() {
            return None;
        }
        let (mins, maxs) = point_entity_bounds(entity.origin()?);
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

pub fn solid_bounds(solid: &Solid) -> Option<(DVec3, DVec3)> {
    let brush = BrushGeometry::from_solid(solid).ok()?;
    Some((brush.mins, brush.maxs))
}

pub fn entity_bounds(entity: &Entity) -> Option<(DVec3, DVec3)> {
    if entity.solids.is_empty() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{create_block, create_entity};

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

        let up = Ray {
            origin: DVec3::new(50.0, 0.0, 0.0),
            direction: DVec3::Z,
        };
        assert_eq!(pick(&doc, &up, |_| true).unwrap().object, ent);
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
}
