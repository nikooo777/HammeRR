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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceHit {
    pub hit: Hit,
    pub normal: DVec3,
    pub terrain_vertex: Option<TerrainVertex>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainVertex {
    pub face: crate::surface::FaceId,
    pub triangle: usize,
    pub index: usize,
    pub grid_size: usize,
    pub position: DVec3,
}

pub fn face_corner(doc: &Document, hit: &Hit) -> Option<(crate::subobject::Component, DVec3)> {
    let (owner, solid) = crate::subobject::solid(doc, hit.solid?).ok()?;
    if owner != hit.object || !hit.point.is_finite() {
        return None;
    }
    let side = hit.side?;
    let source = solid.sides.iter().find(|face| face.id == side)?;
    if source.displacement.is_some() {
        return None;
    }
    let topology = crate::subobject::topology(solid).ok()?;
    let face = topology.faces.iter().find(|face| face.side == side)?;
    let vertex = face
        .vertices
        .iter()
        .map(|&index| &topology.vertices[index])
        .min_by(|a, b| {
            a.position
                .distance_squared(hit.point)
                .total_cmp(&b.position.distance_squared(hit.point))
                .then_with(|| a.sides.cmp(&b.sides))
        })?;
    Some((
        crate::subobject::Component {
            solid: solid.id,
            sides: vertex.sides.clone(),
        },
        vertex.position,
    ))
}

pub fn pick_surface(
    doc: &Document,
    ray: &Ray,
    include_solid: impl Fn(&Solid) -> bool,
) -> Option<SurfaceHit> {
    let hit = pick_face(doc, ray, include_solid)?;
    let (owner, solid) = crate::subobject::solid(doc, hit.solid?).ok()?;
    if owner != hit.object {
        return None;
    }
    let geometry = BrushGeometry::from_solid(solid).ok()?;
    let side = hit.side?;
    if solid.sides.iter().filter(|face| face.id == side).count() != 1 {
        return None;
    }
    let face = geometry
        .faces
        .iter()
        .find(|face| solid.sides[face.side].id == side)?;
    let mut normal = face.plane.normal;
    let mut terrain_vertex = None;
    if solid.sides[face.side].displacement.is_some() {
        let surfaces = crate::displacement::surface::prepare(doc.vmf());
        let mesh = surfaces.get(&crate::surface::FaceId {
            solid: solid.id,
            side,
        })?;
        let ray = Ray {
            origin: ray.origin,
            direction: ray.direction.try_normalize()?,
        };
        let (triangle, _) = pick_displacement_triangle(&ray, mesh)?;
        let [a, b, c] =
            crate::displacement::triangles::indices((mesh.size - 1).ilog2()).ok()?[triangle];
        normal = (mesh.positions[c] - mesh.positions[a])
            .cross(mesh.positions[b] - mesh.positions[a])
            .try_normalize()?;
        let index = [a, b, c].into_iter().min_by(|&a, &b| {
            mesh.positions[a]
                .distance_squared(hit.point)
                .total_cmp(&mesh.positions[b].distance_squared(hit.point))
                .then_with(|| a.cmp(&b))
        })?;
        terrain_vertex = Some(TerrainVertex {
            face: crate::surface::FaceId {
                solid: solid.id,
                side,
            },
            triangle,
            index,
            grid_size: mesh.size,
            position: mesh.positions[index],
        });
    }
    if normal.dot(ray.direction) > 0.0 {
        normal = -normal;
    }
    Some(SurfaceHit {
        hit,
        normal,
        terrain_vertex,
    })
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
    let mut nearest: Option<Hit> = None;
    visit_hits(
        doc,
        ray,
        include_solid,
        include_face,
        include_points,
        include_entity,
        |hit| {
            if nearest.is_none_or(|old| hit.distance.total_cmp(&old.distance).is_lt()) {
                nearest = Some(hit);
            }
        },
    );
    nearest
}

pub fn visit_object_hits(
    doc: &Document,
    ray: &Ray,
    include_solid: impl Fn(&Solid) -> bool,
    include_point: impl Fn(&Entity) -> bool,
    visit: impl FnMut(Hit),
) {
    visit_hits(
        doc,
        ray,
        include_solid,
        |_, _, _| true,
        true,
        include_point,
        visit,
    );
}

fn visit_hits(
    doc: &Document,
    ray: &Ray,
    include_solid: impl Fn(&Solid) -> bool,
    include_face: impl Fn(&Solid, &source_formats::vmf::Side, DVec3) -> bool,
    include_points: bool,
    include_entity: impl Fn(&Entity) -> bool,
    visit: impl FnMut(Hit),
) {
    if !ray.origin.is_finite()
        || !ray.direction.is_finite()
        || ray.direction.length_squared() < 1e-24
    {
        return;
    }
    let ray = Ray {
        origin: ray.origin,
        direction: ray.direction.normalize(),
    };
    let surfaces = crate::displacement::surface::prepare(doc.vmf());
    let test_solid = |object, solid: &Solid| {
        if solid.hidden || !include_solid(solid) {
            return None;
        }
        let displaced = solid.sides.iter().any(|side| side.displacement.is_some());
        let (t, side) = if displaced {
            terrain::pick(&ray, solid, &include_face, &surfaces)?
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
    world.chain(entities).for_each(visit);
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

pub fn point_entity_angles(entity: &Entity) -> Option<DVec3> {
    let mut angles = match entity.properties.get("angles") {
        Some(value) => source_formats::vmf::parse_vec3(value)?,
        None => DVec3::ZERO,
    };
    if entity.properties.get("angles").is_none()
        && let Some(value) = entity.properties.get("angle")
    {
        let angle = value.trim().parse::<f64>().ok()?;
        angles = match angle {
            -1.0 => DVec3::new(-90.0, 0.0, 0.0),
            -2.0 => DVec3::new(90.0, 0.0, 0.0),
            _ => DVec3::new(0.0, angle, 0.0),
        };
    }
    if matches!(entity.classname(), Some("light_spot" | "light_environment"))
        && let Some(value) = entity.properties.get("pitch")
    {
        let pitch = value.trim().parse::<f64>().ok()?;
        angles.x = -pitch;
    }
    angles.is_finite().then_some(angles)
}

pub fn local_basis(doc: &Document, id: ObjectId) -> Option<glam::DMat3> {
    let solid = match id {
        ObjectId::Solid(id) => doc.solid(id)?,
        ObjectId::Entity(id) => {
            let entity = doc.entity(id)?;
            if entity.solids.is_empty() {
                let radians = point_entity_angles(entity)?.map(f64::to_radians);
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
