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
        let cross = (a - b).cross(c - b);
        let mut normal = cross.try_normalize()?;
        if let Some(axis) = (0..3).find(|&axis| {
            a[axis] == b[axis]
                && b[axis] == c[axis]
                && cross[axis] != 0.0
                && (0..3).all(|other| other == axis || cross[other] == 0.0)
        }) {
            normal[axis] = cross[axis].signum();
        }
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
