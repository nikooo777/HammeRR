use glam::DVec3;

/// Points within this distance of a plane count as lying on it.
pub const ON_EPSILON: f64 = 0.001;

/// Half the size of the starting polygon for each face. Source maps are
/// limited to ±16384 units, so this comfortably covers any valid brush.
const BASE_WINDING_EXTENT: f64 = 131_072.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub normal: DVec3,
    pub dist: f64,
}

impl Plane {
    /// The plane through three points in VMF order. The normal points out of
    /// the brush; `None` if the points are collinear.
    pub fn from_points([a, b, c]: [DVec3; 3]) -> Option<Self> {
        let normal = (a - b).cross(c - b).try_normalize()?;
        Some(Self {
            normal,
            dist: normal.dot(b),
        })
    }

    pub fn distance_to(&self, point: DVec3) -> f64 {
        self.normal.dot(point) - self.dist
    }

    pub fn approx_eq(&self, other: &Plane) -> bool {
        (self.normal - other.normal).abs().max_element() < 1e-6
            && (self.dist - other.dist).abs() < ON_EPSILON
    }

    /// A large square on this plane, wound clockwise when viewed from the
    /// front (the side the normal points to).
    pub fn base_winding(&self) -> Vec<DVec3> {
        let n = self.normal;
        let up = if n.z.abs() >= n.x.abs() && n.z.abs() >= n.y.abs() {
            DVec3::X
        } else {
            DVec3::Z
        };
        let up = (up - n * up.dot(n)).normalize() * BASE_WINDING_EXTENT;
        let right = up.cross(n);
        let origin = n * self.dist;
        vec![
            origin - right + up,
            origin + right + up,
            origin + right - up,
            origin - right - up,
        ]
    }

    /// Clips a convex polygon to the part behind this plane.
    pub fn clip_behind(&self, points: &[DVec3]) -> Vec<DVec3> {
        let dists: Vec<f64> = points.iter().map(|&p| self.distance_to(p)).collect();
        if dists.iter().all(|&d| d <= ON_EPSILON) {
            return points.to_vec();
        }
        if dists.iter().all(|&d| d >= -ON_EPSILON) {
            return Vec::new();
        }

        let mut out = Vec::with_capacity(points.len() + 1);
        for i in 0..points.len() {
            let j = (i + 1) % points.len();
            let (p, q) = (points[i], points[j]);
            let (dp, dq) = (dists[i], dists[j]);
            if dp <= ON_EPSILON {
                out.push(p);
            }
            let crosses =
                (dp > ON_EPSILON && dq < -ON_EPSILON) || (dp < -ON_EPSILON && dq > ON_EPSILON);
            if crosses {
                let mut mid = p + (q - p) * (dp / (dp - dq));
                // Axis-aligned planes are exact; avoid accumulating error there.
                for axis in 0..3 {
                    if (0..3).any(|other| other != axis && self.normal[other] != 0.0) {
                        continue;
                    }
                    if self.normal[axis] == 1.0 {
                        mid[axis] = self.dist;
                    } else if self.normal[axis] == -1.0 {
                        mid[axis] = -self.dist;
                    }
                }
                out.push(mid);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_winding_is_clockwise_from_the_front() {
        for normal in [
            DVec3::X,
            DVec3::NEG_Y,
            DVec3::Z,
            DVec3::new(1.0, 2.0, 3.0).normalize(),
        ] {
            let plane = Plane { normal, dist: 10.0 };
            let w = plane.base_winding();
            let winding_normal = (w[0] - w[1]).cross(w[2] - w[1]).normalize();
            assert!((winding_normal - normal).length() < 1e-9, "{normal}");
            assert!(w.iter().all(|&p| plane.distance_to(p).abs() < 1e-6));
        }
    }

    #[test]
    fn clipping_keeps_the_back_part() {
        let floor = Plane {
            normal: DVec3::Z,
            dist: 0.0,
        };
        let wall = Plane {
            normal: DVec3::X,
            dist: 16.0,
        };
        let clipped = wall.clip_behind(&floor.base_winding());
        assert_eq!(clipped.len(), 4);
        assert!(clipped.iter().all(|p| p.x <= 16.0));
        assert!(clipped.iter().any(|p| p.x == 16.0));
    }

    #[test]
    fn clipping_preserves_tiny_slopes_when_the_major_normal_rounds_to_one() {
        let plane = Plane {
            normal: DVec3::new(0.0, 1e-8, -1.0),
            dist: 0.051_596,
        };
        let points = [
            DVec3::new(3928.0, 5784.0, 0.0),
            DVec3::new(4396.0, 5784.0, 0.0),
            DVec3::new(4396.0, 5784.0, -1.0),
            DVec3::new(3928.0, 5784.0, -1.0),
        ];
        let clipped = plane.clip_behind(&points);
        assert_eq!(clipped.len(), 4);
        let crossings = clipped
            .iter()
            .filter(|point| point.z < 0.0 && point.z > -0.5)
            .collect::<Vec<_>>();
        assert_eq!(crossings.len(), 2);
        assert!(
            crossings
                .iter()
                .all(|point| plane.distance_to(**point).abs() < 1e-12)
        );
    }

    #[test]
    fn clipping_preserves_minor_slopes_for_every_signed_major_axis() {
        for axis in 0..3 {
            for sign in [-1.0, 1.0] {
                for slope in [-1e-8, 1e-8] {
                    let next = (axis + 1) % 3;
                    let other = (axis + 2) % 3;
                    let mut normal = DVec3::ZERO;
                    normal[axis] = sign;
                    normal[next] = slope;
                    normal[other] = -0.75e-8;
                    let plane = Plane {
                        normal,
                        dist: 0.051_596,
                    };
                    let points = [(-1.0, 3928.0), (-1.0, 4396.0), (1.0, 4396.0), (1.0, 3928.0)]
                        .map(|(along, across)| {
                            let mut point = DVec3::ZERO;
                            point[axis] = along;
                            point[next] = 5784.0;
                            point[other] = across;
                            point
                        });
                    let clipped = plane.clip_behind(&points);
                    assert_eq!(clipped.len(), 4);
                    let crossings = clipped
                        .iter()
                        .filter(|point| point[axis].abs() < 0.5)
                        .collect::<Vec<_>>();
                    assert_eq!(crossings.len(), 2);
                    assert!(
                        crossings
                            .iter()
                            .all(|point| plane.distance_to(**point).abs() < 1e-12)
                    );
                }
            }
        }
    }
}
