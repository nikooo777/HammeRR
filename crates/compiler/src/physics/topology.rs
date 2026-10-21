use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;

use super::{MAX_CORNERS, MAX_PLANES, Shape};
use crate::map::MAX_COMPILE_COORD;

pub(super) const MAX_FACE_INDICES: usize = 6 * MAX_CORNERS - 12;
pub(super) const MAX_REQUEST_INDICES: usize = 6_000_000;
pub(super) const MAX_REQUEST_WORK: usize = 50_000_000;
const SOURCE_CORNER_ROUNDOFF: f64 = 512.0 * f64::EPSILON * 131_072.0;
const SUPPORT_EPSILON: f64 = 0.02;
const PLANAR_ROUNDOFF_ULPS: f64 = 4.0;

#[derive(Default)]
pub(super) struct Budget {
    indices: usize,
    work: usize,
}

impl Budget {
    pub(super) fn charge(&mut self, indices: usize, work: usize) -> Result<(), String> {
        self.indices = self
            .indices
            .checked_add(indices)
            .ok_or("Physics topology index count overflows")?;
        self.work = self
            .work
            .checked_add(work)
            .ok_or("Physics topology work count overflows")?;
        if self.indices > MAX_REQUEST_INDICES || self.work > MAX_REQUEST_WORK {
            return Err("Physics topology exceeds six million face indices or 50 million validation work units".into());
        }
        Ok(())
    }
}

fn error(brush: u32, message: impl std::fmt::Display) -> String {
    format!("Physics brush {brush} topology: {message}")
}

fn source_key(point: DVec3) -> [u64; 3] {
    point
        .to_array()
        .map(|value| if value == 0.0 { 0 } else { value.to_bits() })
}

pub(super) fn build<'a>(
    brush: u32,
    sides: impl Clone + Iterator<Item = ([f32; 4], &'a [DVec3])>,
    budget: &mut Budget,
) -> Result<Shape, String> {
    let mut count = 0_usize;
    let mut indices = 0_usize;
    let mut face_work = 0_usize;
    for (_, winding) in sides.clone() {
        count += 1;
        indices = indices
            .checked_add(winding.len())
            .ok_or_else(|| error(brush, "source winding count overflows"))?;
        if count > MAX_PLANES
            || !(3..=MAX_CORNERS).contains(&winding.len())
            || indices > MAX_FACE_INDICES
        {
            return Err(error(
                brush,
                "source winding counts exceed supported bounds",
            ));
        }
        face_work += winding.len() * winding.len();
    }
    if count < 4 {
        return Err(error(
            brush,
            "at least four nonempty source faces are required",
        ));
    }
    let corner_bound = MAX_CORNERS.min(indices);
    let work =
        corner_bound * corner_bound + 2 * corner_bound * count + face_work + indices * corner_bound;
    budget
        .charge(indices, work)
        .map_err(|message| error(brush, message))?;
    let mut incident_faces: BTreeMap<[u64; 3], [u64; 4]> = BTreeMap::new();
    for (face_index, (_, winding)) in sides.clone().enumerate() {
        for &point in winding {
            if !point.is_finite() || point.abs().max_element() > MAX_COMPILE_COORD {
                return Err(error(
                    brush,
                    "source corners require finite Source coordinates",
                ));
            }
            let faces = incident_faces.entry(source_key(point)).or_default();
            faces[face_index / 64] |= 1 << (face_index % 64);
        }
    }
    let mut source_corners: Vec<DVec3> = Vec::with_capacity(MAX_CORNERS.min(indices));
    let mut corner_faces: Vec<[u64; 4]> = Vec::with_capacity(MAX_CORNERS.min(indices));
    let mut source_indices = BTreeMap::new();
    let mut shape = Shape {
        brush,
        corners: Vec::with_capacity(MAX_CORNERS.min(indices)),
        planes: Vec::with_capacity(count),
        faces: Vec::with_capacity(count),
        materials: Vec::new(),
    };
    for (plane, winding) in sides {
        let mut face = Vec::with_capacity(winding.len());
        for &point in winding.iter().rev() {
            let key = source_key(point);
            let incident = incident_faces[&key];
            let existing = source_indices.get(&key).copied().or_else(|| {
                source_corners
                    .iter()
                    .enumerate()
                    .filter(|&(index, &candidate)| {
                        point == candidate
                            || ((point - candidate).abs().max_element() <= SOURCE_CORNER_ROUNDOFF
                                && incident
                                    .iter()
                                    .zip(corner_faces[index])
                                    .all(|(a, b)| a & b == 0))
                    })
                    .min_by(|(_, a), (_, b)| {
                        point
                            .distance_squared(**a)
                            .total_cmp(&point.distance_squared(**b))
                    })
                    .map(|(index, _)| index)
            });
            let index = if let Some(index) = existing {
                for (faces, incident) in corner_faces[index].iter_mut().zip(incident) {
                    *faces |= incident;
                }
                index
            } else {
                if source_corners.len() == MAX_CORNERS {
                    return Err(error(brush, "source corner count exceeds 256"));
                }
                let stored = point
                    .as_vec3()
                    .to_array()
                    .map(|value| if value == 0.0 { 0.0 } else { value });
                if shape.corners.contains(&stored) {
                    return Err(error(
                        brush,
                        "distinct source corners collapse in Source f32 coordinates",
                    ));
                }
                let index = source_corners.len();
                source_corners.push(point);
                corner_faces.push(incident);
                shape.corners.push(stored);
                index
            };
            source_indices.insert(key, index);
            face.push(index as u16);
        }
        shape.planes.push(plane);
        shape.faces.push(face);
    }
    validate(&shape)?;
    Ok(shape)
}

pub(super) fn counts(shape: &Shape) -> Result<(usize, usize), String> {
    if !(4..=MAX_CORNERS).contains(&shape.corners.len())
        || !(4..=MAX_PLANES).contains(&shape.planes.len())
        || shape.faces.len() != shape.planes.len()
    {
        return Err(error(
            shape.brush,
            "4–256 corners and matching planes and ordered faces are required",
        ));
    }
    let mut indices = 0_usize;
    let mut work =
        shape.corners.len() * shape.corners.len() + 2 * shape.corners.len() * shape.planes.len();
    for face in &shape.faces {
        if !(3..=shape.corners.len()).contains(&face.len()) {
            return Err(error(
                shape.brush,
                "a face needs 3–256 unique corner indices",
            ));
        }
        indices = indices
            .checked_add(face.len())
            .ok_or_else(|| error(shape.brush, "face index count overflows"))?;
        if indices > MAX_FACE_INDICES {
            return Err(error(
                shape.brush,
                "face index count exceeds supported bounds",
            ));
        }
        work += face.len() * face.len();
    }
    Ok((indices, work))
}

fn point(value: [f32; 3]) -> DVec3 {
    DVec3::from_array(value.map(f64::from))
}

fn roundoff(value: [f32; 3], normal: DVec3) -> f64 {
    let radius = value.map(|value| {
        f64::from(value.next_up() - value).max(f64::from(value - value.next_down())) * 0.5
    });
    normal.abs().dot(DVec3::from_array(radius))
}

pub(super) fn validate(shape: &Shape) -> Result<(), String> {
    let (indices, _) = counts(shape)?;
    let points = shape.corners.iter().copied().map(point).collect::<Vec<_>>();
    if points
        .iter()
        .any(|point| !point.is_finite() || point.abs().max_element() > MAX_COMPILE_COORD)
    {
        return Err(error(
            shape.brush,
            "corners require finite Source coordinates",
        ));
    }
    for (index, point) in points.iter().enumerate() {
        if points[..index].contains(point) {
            return Err(error(shape.brush, "corner coordinates must be unique"));
        }
    }
    let origin = points.iter().sum::<DVec3>() / points.len() as f64;
    let mut edges = BTreeMap::new();
    let mut used = vec![false; points.len()];
    let mut volume = 0.0;
    for (face_index, (face, plane)) in shape.faces.iter().zip(&shape.planes).enumerate() {
        let normal = DVec3::new(
            f64::from(plane[0]),
            f64::from(plane[1]),
            f64::from(plane[2]),
        );
        if !normal.is_finite() || normal.length_squared() == 0.0 || !plane[3].is_finite() {
            return Err(error(
                shape.brush,
                format!("face {face_index} has an invalid plane"),
            ));
        }
        let length = normal.length();
        let normal = normal / length;
        let dist = f64::from(plane[3]) / length;
        let mut seen = BTreeSet::new();
        for &index in face {
            let index = usize::from(index);
            if index >= points.len() || !seen.insert(index) {
                return Err(error(
                    shape.brush,
                    format!("face {face_index} has a repeated or out-of-range corner"),
                ));
            }
            used[index] = true;
            if (normal.dot(points[index]) - dist).abs() > SUPPORT_EPSILON {
                return Err(error(
                    shape.brush,
                    format!("face {face_index} escapes its plane"),
                ));
            }
        }
        if points
            .iter()
            .any(|&point| normal.dot(point) - dist > SUPPORT_EPSILON)
        {
            return Err(error(shape.brush, "corners escape their supporting planes"));
        }
        let first = points[usize::from(face[0])];
        let mut area = DVec3::ZERO;
        for pair in face[1..].windows(2) {
            let a = points[usize::from(pair[0])];
            let b = points[usize::from(pair[1])];
            area += (a - first).cross(b - first);
            volume += (first - origin).dot((a - origin).cross(b - origin)) / 6.0;
        }
        if !area.is_finite() || area.length_squared() == 0.0 || area.dot(normal) <= 0.0 {
            return Err(error(
                shape.brush,
                format!("face {face_index} is degenerate or reversed"),
            ));
        }
        let geometric_normal = area.normalize();
        let epsilon = PLANAR_ROUNDOFF_ULPS
            * face
                .iter()
                .map(|&index| roundoff(shape.corners[usize::from(index)], geometric_normal))
                .fold(0.0_f64, f64::max);
        for &index in face {
            if geometric_normal
                .dot(points[usize::from(index)] - first)
                .abs()
                > epsilon
            {
                return Err(error(
                    shape.brush,
                    format!("face {face_index} is not planar"),
                ));
            }
        }
        if points
            .iter()
            .any(|&point| geometric_normal.dot(point - first) > epsilon)
        {
            return Err(error(
                shape.brush,
                format!("face {face_index} is not a convex boundary"),
            ));
        }
        for (&a, &b) in face.iter().zip(face.iter().cycle().skip(1)) {
            let start = points[usize::from(a)];
            let direction = points[usize::from(b)] - start;
            if face.iter().any(|&index| {
                direction
                    .cross(points[usize::from(index)] - start)
                    .dot(geometric_normal)
                    < -epsilon * direction.length()
            }) {
                return Err(error(
                    shape.brush,
                    format!("face {face_index} is not convex and ordered"),
                ));
            }
            let key = (a.min(b), a.max(b));
            let edge = edges.entry(key).or_insert((0_usize, 0_i32));
            edge.0 += 1;
            edge.1 += if a < b { 1 } else { -1 };
        }
    }
    if used.contains(&false)
        || edges.len() * 2 != indices
        || edges
            .values()
            .any(|&(count, direction)| count != 2 || direction != 0)
        || points.len() + shape.faces.len() != edges.len() + 2
    {
        return Err(error(
            shape.brush,
            "faces must form one closed manifold with opposed edges",
        ));
    }
    let mut neighbors = vec![Vec::new(); points.len()];
    for &(a, b) in edges.keys() {
        neighbors[usize::from(a)].push(usize::from(b));
        neighbors[usize::from(b)].push(usize::from(a));
    }
    let mut connected = vec![false; points.len()];
    let mut pending = vec![0];
    connected[0] = true;
    while let Some(index) = pending.pop() {
        for &neighbor in &neighbors[index] {
            if !connected[neighbor] {
                connected[neighbor] = true;
                pending.push(neighbor);
            }
        }
    }
    if connected.contains(&false) {
        return Err(error(shape.brush, "face topology is disconnected"));
    }
    if !volume.is_finite() || volume <= 0.0 {
        return Err(error(
            shape.brush,
            "finite positive enclosed volume is required",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
