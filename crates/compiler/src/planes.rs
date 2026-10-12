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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planes_come_in_pairs() {
        let mut set = PlaneSet::default();
        let down = set.find(DVec3::NEG_Z, -32.0);
        assert_eq!(down, 1);
        assert_eq!(set.get(0).normal, DVec3::Z);
        assert_eq!(set.get(0).dist, 32.0);
        assert_eq!(set.find(DVec3::Z, 32.0005), 0);
        let slope = set.find(DVec3::new(1.0, 0.0, 1.0).normalize(), 10.0);
        assert_eq!(slope, 2);
        assert_eq!(set.get(3).normal, -set.get(2).normal);
        assert_eq!(set.get(slope).kind, 3);
    }

    #[test]
    fn checked_lookup_keeps_distant_support_planes_that_normal_tolerance_would_merge() {
        let mut set = PlaneSet::default();
        let existing_normal = DVec3::new(1.0, 1.0, 0.0).normalize();
        let tangent = DVec3::new(1.0, -1.0, 0.0).normalize();
        let candidate_normal = (existing_normal + tangent * 8e-6).normalize();
        let existing = set.find(existing_normal, 64.0);
        let translation = DVec3::new(-16_000.0, 16_000.0, 0.0);
        let point = translation
            + existing_normal
                * ((64.0 - candidate_normal.dot(translation))
                    / candidate_normal.dot(existing_normal));
        assert!(point.abs().max_element() < crate::map::MAX_COMPILE_COORD);
        assert!(set.get(existing).distance(point) > 0.1);
        assert_eq!(set.find(candidate_normal, 64.0), existing);
        let checked = set
            .find_checked(candidate_normal, 64.0, |plane| {
                Ok::<_, ()>(plane.distance(point).abs() <= 0.01)
            })
            .unwrap()
            .unwrap();
        assert_ne!(checked, existing);
        assert!(set.get(checked).distance(point).abs() < 1e-9);
        assert_eq!(set.get(checked ^ 1).normal, -set.get(checked).normal);
    }

    #[test]
    fn checked_lookup_does_not_allocate_rejected_snapped_axial_planes() {
        let mut set = PlaneSet::default();
        let axial = set.find(DVec3::X, 32.0);
        let len = set.len();
        let rejected = set
            .find_checked(DVec3::X, 40.0, |plane| Ok::<_, ()>(plane.kind >= 3))
            .unwrap();
        assert_eq!(rejected, None);
        assert_eq!(set.len(), len);
        assert_eq!(set.find(DVec3::X, 32.0), axial);
        let cancelled = set.find_checked(DVec3::ONE.normalize(), 32.0, |_| {
            Err::<bool, _>("cancelled")
        });
        assert_eq!(cancelled, Err("cancelled"));
        assert_eq!(set.len(), len);
    }

    #[test]
    fn collision_planes_do_not_change_later_authored_plane_lookup() {
        let collision_normal = DVec3::new(1.0, 1.0, 0.0).normalize();
        let authored_normal = (collision_normal + DVec3::Z * 8e-6).normalize();
        let mut baseline = PlaneSet::default();
        let baseline_authored = baseline
            .find_surface_checked(authored_normal, 64.0, |_| Ok::<_, ()>(true))
            .unwrap()
            .unwrap();
        let mut set = PlaneSet::default();
        let collision = set
            .find_checked(collision_normal, 64.0, |_| Ok::<_, ()>(true))
            .unwrap()
            .unwrap();
        let authored = set
            .find_surface_checked(authored_normal, 64.0, |_| Ok::<_, ()>(true))
            .unwrap()
            .unwrap();
        assert_ne!(authored, collision);
        assert_eq!(set.get(authored), baseline.get(baseline_authored));
        assert_eq!(set.find(authored_normal, 64.0), authored);
    }

    #[test]
    fn checked_lookup_retains_and_reuses_unsnapped_support_distance() {
        let mut set = PlaneSet::default();
        let normal = DVec3::new(1.0, 1.0, 0.0).normalize();
        let boundary = 64.989;
        let accept = |plane: &Plane| Ok::<_, ()>((plane.dist - boundary).abs() <= 0.01);
        let index = set.find_checked(normal, 64.9915, accept).unwrap().unwrap();
        assert_eq!(set.get(index).dist, 64.9915);
        assert_eq!(set.len(), 2);
        assert_eq!(
            set.find_checked(normal, 64.9915, accept).unwrap(),
            Some(index)
        );
        assert_eq!(set.len(), 2);
        let authored = set.find(normal, 64.9915);
        assert_eq!(set.get(authored).dist, 65.0);
        assert_ne!(index, authored);
    }

    #[test]
    fn checked_lookup_preserves_near_axial_normal_when_snapping_removes_a_facet() {
        let mut set = PlaneSet::default();
        let normal = DVec3::new(1.0, 0.001, 0.0).normalize();
        let dist = normal.x * 64.0;
        let accept = |plane: &Plane| Ok::<_, ()>(plane.kind >= 3);
        let index = set.find_checked(normal, dist, accept).unwrap().unwrap();
        assert_eq!(set.get(index).normal, normal);
        assert_eq!(set.get(index).dist, dist);
        assert_eq!(set.get(index).kind, 3);
        assert_eq!(set.find_checked(normal, dist, accept).unwrap(), Some(index));
        assert_eq!(set.len(), 2);
        let axial = set.find(normal, dist);
        assert_eq!(set.get(axial).normal, DVec3::X);
        assert_eq!(set.get(axial).dist, 64.0);
        assert_ne!(index, axial);
    }

    #[test]
    fn unsnapped_near_axial_planes_keep_a_nonaxial_kind() {
        let mut set = PlaneSet::default();
        let normal = DVec3::new(1.0, 1e-9, 0.0);
        let index = set
            .find_surface_checked(normal, 64.0, |plane| Ok::<_, ()>(plane.normal == normal))
            .unwrap()
            .unwrap();
        assert_eq!(set.get(index).normal, normal);
        assert_eq!(set.get(index).kind, 3);
        assert_eq!(set.get(index ^ 1).kind, 3);
        let axial = set.find(DVec3::X, 64.0);
        assert_ne!(axial, index);
        assert_eq!(set.get(axial).normal, DVec3::X);
        assert_eq!(set.get(axial).kind, 0);
    }
}
