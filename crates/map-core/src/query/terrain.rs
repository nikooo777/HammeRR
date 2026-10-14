use glam::DVec3;
use source_formats::vmf::{Side, Solid};

use super::Ray;
use crate::{BrushGeometry, DisplacementMesh};

fn meshes(solid: &Solid) -> Vec<(usize, DisplacementMesh)> {
    let Ok(geometry) = BrushGeometry::from_solid(solid) else {
        return Vec::new();
    };
    geometry
        .faces
        .iter()
        .filter_map(|face| {
            let info = solid.sides[face.side].displacement.as_ref()?;
            let mesh = DisplacementMesh::build(face, info).ok()?;
            mesh.positions
                .iter()
                .all(|point| point.is_finite())
                .then_some((face.side, mesh))
        })
        .collect()
}

pub(super) fn points(solid: &Solid) -> Vec<DVec3> {
    meshes(solid)
        .into_iter()
        .flat_map(|(_, mesh)| mesh.positions)
        .collect()
}

fn intersects_box(ray: &Ray, mins: DVec3, maxs: DVec3) -> bool {
    let mut enter: f64 = 0.0;
    let mut exit = f64::INFINITY;
    for axis in 0..3 {
        let direction = ray.direction[axis];
        if direction.abs() < 1e-12 {
            if ray.origin[axis] < mins[axis] || ray.origin[axis] > maxs[axis] {
                return false;
            }
        } else {
            let a = (mins[axis] - ray.origin[axis]) / direction;
            let b = (maxs[axis] - ray.origin[axis]) / direction;
            enter = enter.max(a.min(b));
            exit = exit.min(a.max(b));
            if enter > exit {
                return false;
            }
        }
    }
    true
}

fn triangle(ray: &Ray, a: DVec3, b: DVec3, c: DVec3) -> Option<f64> {
    let ab = b - a;
    let ac = c - a;
    let p = ray.direction.cross(ac);
    let determinant = ab.dot(p);
    if determinant >= -1e-12 {
        return None;
    }
    let inverse = determinant.recip();
    let from_a = ray.origin - a;
    let u = from_a.dot(p) * inverse;
    if !(-1e-9..=1.0 + 1e-9).contains(&u) {
        return None;
    }
    let q = from_a.cross(ab);
    let v = ray.direction.dot(q) * inverse;
    if v < -1e-9 || u + v > 1.0 + 1e-9 {
        return None;
    }
    let distance = ac.dot(q) * inverse;
    (distance.is_finite() && distance >= 0.0).then_some(distance)
}

pub(super) fn pick(
    ray: &Ray,
    solid: &Solid,
    include_face: &impl Fn(&Solid, &Side, DVec3) -> bool,
) -> Option<(f64, usize)> {
    let mut nearest = None::<(f64, usize)>;
    for (side, mesh) in meshes(solid) {
        let (mins, maxs) = mesh.positions.iter().fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(lo, hi), point| (lo.min(*point), hi.max(*point)),
        );
        if !intersects_box(ray, mins, maxs) {
            continue;
        }
        for [a, b, c] in mesh.triangles() {
            let Some(distance) =
                triangle(ray, mesh.positions[a], mesh.positions[b], mesh.positions[c])
            else {
                continue;
            };
            if nearest.is_some_and(|hit| hit.0 <= distance) {
                continue;
            }
            let point = ray.origin + ray.direction * distance;
            if point.is_finite() && include_face(solid, &solid.sides[side], point) {
                nearest = Some((distance, side));
            }
        }
    }
    nearest
}
