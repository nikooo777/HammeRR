//! The map's plane table. Planes are stored in pairs: an even index faces
//! the positive side of its dominant axis, the following odd index is the
//! same plane flipped, so `index ^ 1` is always the opposite plane.

use std::collections::HashMap;

use glam::DVec3;

pub(crate) const NORMAL_EPSILON: f64 = 1e-5;
const DIST_EPSILON: f64 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub normal: DVec3,
    pub dist: f64,
    /// 0-2 for planes facing an axis, 3-5 for planes closest to one.
    pub kind: i32,
}

impl Plane {
    pub fn distance(&self, point: DVec3) -> f64 {
        self.normal.dot(point) - self.dist
    }
}

#[derive(Debug, Default)]
pub struct PlaneSet {
    planes: Vec<Plane>,
    by_dist: HashMap<i64, Vec<usize>>,
    collision_by_dist: HashMap<i64, Vec<usize>>,
}

fn kind(normal: DVec3) -> i32 {
    if let Some(axis) = [DVec3::X, DVec3::Y, DVec3::Z]
        .iter()
        .position(|&axis| normal.abs() == axis)
    {
        return axis as i32;
    }
    let a = normal.abs();
    if a.x >= a.y && a.x >= a.z {
        3
    } else if a.y >= a.z {
        4
    } else {
        5
    }
}

fn snap(mut normal: DVec3, mut dist: f64) -> (DVec3, f64) {
    if let Some(axis) = (0..3).find(|&a| (normal[a].abs() - 1.0).abs() < NORMAL_EPSILON) {
        let sign = normal[axis].signum();
        normal = DVec3::ZERO;
        normal[axis] = sign;
    }
    if (dist - dist.round()).abs() < DIST_EPSILON {
        dist = dist.round();
    }
    (normal, dist)
}

impl PlaneSet {
    pub fn get(&self, index: usize) -> &Plane {
        &self.planes[index]
    }

    pub fn len(&self) -> usize {
        self.planes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.planes.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Plane> {
        self.planes.iter()
    }

    /// Index of the plane, adding it and its twin if new.
    pub fn find(&mut self, normal: DVec3, dist: f64) -> usize {
        let (normal, dist) = snap(normal, dist);
        let key = dist.floor() as i64;
        for k in [key - 1, key, key + 1] {
            for &i in self.by_dist.get(&k).into_iter().flatten() {
                let p = &self.planes[i];
                if (p.normal - normal).abs().max_element() < NORMAL_EPSILON
                    && (p.dist - dist).abs() < DIST_EPSILON
                    && (kind(normal) >= 3 || p.normal == normal)
                {
                    return i;
                }
            }
        }
        self.insert(normal, dist, false)
    }

    pub(crate) fn find_checked<E>(
        &mut self,
        normal: DVec3,
        dist: f64,
        accept: impl Fn(&Plane) -> Result<bool, E>,
    ) -> Result<Option<usize>, E> {
        self.find_checked_in(normal, dist, accept, true)
    }

    pub(crate) fn find_surface_checked<E>(
        &mut self,
        normal: DVec3,
        dist: f64,
        accept: impl Fn(&Plane) -> Result<bool, E>,
    ) -> Result<Option<usize>, E> {
        self.find_checked_in(normal, dist, accept, false)
    }

    fn find_checked_in<E>(
        &mut self,
        normal: DVec3,
        dist: f64,
        accept: impl Fn(&Plane) -> Result<bool, E>,
        collision: bool,
    ) -> Result<Option<usize>, E> {
        let (snapped_normal, snapped_dist) = snap(normal, dist);
        let mut candidate = Plane {
            normal: snapped_normal,
            dist: snapped_dist,
            kind: kind(snapped_normal),
        };
        if !accept(&candidate)? {
            candidate.normal = normal;
            candidate.dist = dist;
            candidate.kind = kind(normal);
            if !accept(&candidate)? {
                return Ok(None);
            }
        }
        let normal = candidate.normal;
        let dist = candidate.dist;
        let key = dist.floor() as i64;
        let hashes = [&self.by_dist, &self.collision_by_dist];
        let hashes = &hashes[..if collision { 2 } else { 1 }];
        for k in [key - 1, key, key + 1] {
            for hash in hashes {
                for &i in hash.get(&k).into_iter().flatten() {
                    let p = &self.planes[i];
                    if (p.normal - normal).abs().max_element() < NORMAL_EPSILON
                        && (p.dist - dist).abs() < DIST_EPSILON
                        && accept(p)?
                    {
                        return Ok(Some(i));
                    }
                }
            }
        }
        Ok(Some(self.insert(normal, dist, collision)))
    }

    fn insert(&mut self, normal: DVec3, dist: f64, collision: bool) -> usize {
        let kind = kind(normal);
        let positive = normal[(kind % 3) as usize] >= 0.0;
        let (even_normal, even_dist) = if positive {
            (normal, dist)
        } else {
            (-normal, -dist)
        };
        let index = self.planes.len();
        let by_dist = if collision {
            &mut self.collision_by_dist
        } else {
            &mut self.by_dist
        };
        for (n, d) in [(even_normal, even_dist), (-even_normal, -even_dist)] {
            by_dist
                .entry(d.floor() as i64)
                .or_default()
                .push(self.planes.len());
            self.planes.push(Plane {
                normal: n,
                dist: d,
                kind,
            });
        }
        if positive { index } else { index + 1 }
    }
}
