//! Convex polygons on planes, wound clockwise when seen from the front.

use glam::DVec3;

/// Points this close to a plane count as lying on it.
pub const ON_EPSILON: f64 = 0.01;
/// Polygons with fewer than three edges longer than this are slivers.
const EDGE_LENGTH: f64 = 0.2;
/// Half the size of the starting polygon on a plane; covers the ±16384
/// map limit with room to spare.
const BASE_EXTENT: f64 = 65_536.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Front,
    Back,
    On,
    Cross,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Winding(pub Vec<DVec3>);

impl Winding {
    /// A large square on the plane.
    pub fn base(normal: DVec3, dist: f64) -> Self {
        let n = normal;
        let up = if n.z.abs() >= n.x.abs() && n.z.abs() >= n.y.abs() {
            DVec3::X
        } else {
            DVec3::Z
        };
        let up = (up - n * up.dot(n)).normalize() * BASE_EXTENT;
        let right = up.cross(n);
        let origin = n * dist;
        Self(vec![
            origin - right + up,
            origin + right + up,
            origin + right - up,
            origin - right - up,
        ])
    }

    pub fn classify(&self, normal: DVec3, dist: f64) -> Side {
        let (mut front, mut back) = (false, false);
        for &p in &self.0 {
            let d = normal.dot(p) - dist;
            front |= d > ON_EPSILON;
            back |= d < -ON_EPSILON;
        }
        match (front, back) {
            (true, true) => Side::Cross,
            (true, false) => Side::Front,
            (false, true) => Side::Back,
            (false, false) => Side::On,
        }
    }

    /// The parts in front of and behind the plane. A polygon lying on the
    /// plane goes to the side its own normal faces.
    pub fn split(&self, normal: DVec3, dist: f64) -> (Option<Self>, Option<Self>) {
        let dists: Vec<f64> = self.0.iter().map(|&p| normal.dot(p) - dist).collect();
        let front = dists.iter().any(|&d| d > ON_EPSILON);
        let back = dists.iter().any(|&d| d < -ON_EPSILON);
        match (front, back) {
            (false, false) => {
                return if self.normal().dot(normal) > 0.0 {
                    (Some(self.clone()), None)
                } else {
                    (None, Some(self.clone()))
                };
            }
            (true, false) => return (Some(self.clone()), None),
            (false, true) => return (None, Some(self.clone())),
            (true, true) => {}
        }

        let mut f = Vec::with_capacity(self.0.len() + 2);
        let mut b = Vec::with_capacity(self.0.len() + 2);
        for i in 0..self.0.len() {
            let j = (i + 1) % self.0.len();
            let (p, q) = (self.0[i], self.0[j]);
            let (dp, dq) = (dists[i], dists[j]);
            if dp >= -ON_EPSILON {
                f.push(p);
            }
            if dp <= ON_EPSILON {
                b.push(p);
            }
            let crosses =
                (dp > ON_EPSILON && dq < -ON_EPSILON) || (dp < -ON_EPSILON && dq > ON_EPSILON);
            if crosses {
                let mid = intersect(p, q, dp, dq, normal, dist);
                f.push(mid);
                b.push(mid);
            }
        }
        (Self::checked(f), Self::checked(b))
    }

    /// The part in front of the plane.
    pub fn clip_front(&self, normal: DVec3, dist: f64) -> Option<Self> {
        match self.classify(normal, dist) {
            Side::Front | Side::On => Some(self.clone()),
            Side::Back => None,
            Side::Cross => self.split(normal, dist).0,
        }
    }

    /// The part behind the plane.
    pub fn clip_back(&self, normal: DVec3, dist: f64) -> Option<Self> {
        self.clip_front(-normal, -dist)
    }

    fn checked(points: Vec<DVec3>) -> Option<Self> {
        let mut points = points;
        points.dedup_by(|a, b| a.distance_squared(*b) < 1e-6);
        while points.len() > 1 && points[0].distance_squared(points[points.len() - 1]) < 1e-6 {
            points.pop();
        }
        (points.len() >= 3).then_some(Self(points))
    }

    /// Unit normal from the winding order.
    pub fn normal(&self) -> DVec3 {
        let mut sum = DVec3::ZERO;
        let p0 = self.0[0];
        for i in 1..self.0.len() - 1 {
            sum += (self.0[i + 1] - p0).cross(self.0[i] - p0);
        }
        sum.normalize_or_zero()
    }

    pub fn area(&self) -> f64 {
        let p0 = self.0[0];
        (1..self.0.len() - 1)
            .map(|i| (self.0[i] - p0).cross(self.0[i + 1] - p0).length() * 0.5)
            .sum()
    }

    pub fn center(&self) -> DVec3 {
        self.0.iter().sum::<DVec3>() / self.0.len() as f64
    }

    pub fn bounds(&self) -> (DVec3, DVec3) {
        self.0
            .iter()
            .fold((DVec3::MAX, DVec3::MIN), |(lo, hi), &p| {
                (lo.min(p), hi.max(p))
            })
    }

    pub fn reversed(&self) -> Self {
        let mut points = self.0.clone();
        points.reverse();
        Self(points)
    }

    pub fn is_tiny(&self) -> bool {
        let long_edges = (0..self.0.len())
            .filter(|&i| self.0[i].distance(self.0[(i + 1) % self.0.len()]) > EDGE_LENGTH)
            .count();
        long_edges < 3
    }

    /// Joins two coplanar polygons sharing an edge if the result is convex.
    pub fn merge(&self, other: &Self, normal: DVec3) -> Option<Self> {
        let (a, b) = (&self.0, &other.0);
        for i in 0..a.len() {
            let (p1, p2) = (a[i], a[(i + 1) % a.len()]);
            let Some(j) = (0..b.len()).find(|&j| {
                b[j].distance_squared(p2) < 1e-6 && b[(j + 1) % b.len()].distance_squared(p1) < 1e-6
            }) else {
                continue;
            };
            // Walk self from p2 around to p1, then other from p1 around to p2.
            let mut points = Vec::with_capacity(a.len() + b.len() - 2);
            for k in 0..a.len() - 1 {
                points.push(a[(i + 1 + k) % a.len()]);
            }
            for k in 0..b.len() - 1 {
                points.push(b[(j + 1 + k) % b.len()]);
            }
            return convex_without_collinear(points, normal);
        }
        None
    }
}

/// Removes collinear points and rejects reflex corners.
fn convex_without_collinear(points: Vec<DVec3>, normal: DVec3) -> Option<Winding> {
    let n = points.len();
    let mut kept = Vec::with_capacity(n);
    for i in 0..n {
        let prev = points[(i + n - 1) % n];
        let cur = points[i];
        let next = points[(i + 1) % n];
        let turn = (cur - prev).cross(next - cur).dot(normal);
        let edge = (next - prev).length();
        // Clockwise from the front turns negative around the normal.
        if turn > 1e-3 * edge {
            return None;
        }
        if turn.abs() <= 1e-3 * edge {
            let along = (cur - prev)
                .normalize_or_zero()
                .dot((next - cur).normalize_or_zero());
            if along > 0.0 {
                continue;
            }
        }
        kept.push(cur);
    }
    Winding::checked(kept)
}

fn intersect(p: DVec3, q: DVec3, dp: f64, dq: f64, normal: DVec3, dist: f64) -> DVec3 {
    let mut mid = p + (q - p) * (dp / (dp - dq));
    for axis in 0..3 {
        if normal[axis] == 1.0 {
            mid[axis] = dist;
        } else if normal[axis] == -1.0 {
            mid[axis] = -dist;
        }
    }
    mid
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(z: f64, size: f64) -> Winding {
        // Clockwise seen from above (+Z).
        Winding(vec![
            DVec3::new(0.0, size, z),
            DVec3::new(size, size, z),
            DVec3::new(size, 0.0, z),
            DVec3::new(0.0, 0.0, z),
        ])
    }

    #[test]
    fn base_winding_faces_its_plane() {
        for normal in [
            DVec3::X,
            DVec3::NEG_Z,
            DVec3::new(1.0, 2.0, -3.0).normalize(),
        ] {
            let w = Winding::base(normal, 5.0);
            assert!((w.normal() - normal).length() < 1e-9);
            assert_eq!(w.classify(normal, 5.0), Side::On);
        }
    }

    #[test]
    fn split_and_clip() {
        let w = square(0.0, 64.0);
        assert!((w.normal() - DVec3::Z).length() < 1e-12);
        let (front, back) = w.split(DVec3::X, 16.0);
        let (front, back) = (front.unwrap(), back.unwrap());
        assert!((front.area() - 48.0 * 64.0).abs() < 1e-9);
        assert!((back.area() - 16.0 * 64.0).abs() < 1e-9);
        assert!(w.clip_front(DVec3::X, 100.0).is_none());
        let (on_front, on_back) = w.split(DVec3::Z, 0.0);
        assert!(on_front.is_some() && on_back.is_none());
    }

    #[test]
    fn merges_neighbours_into_one_convex_polygon() {
        let w = square(0.0, 64.0);
        let (front, back) = w.split(DVec3::X, 16.0);
        let merged = front.unwrap().merge(&back.unwrap(), DVec3::Z).unwrap();
        assert_eq!(merged.0.len(), 4);
        assert!((merged.area() - 64.0 * 64.0).abs() < 1e-9);
        assert!((merged.normal() - DVec3::Z).length() < 1e-12);
    }
}
