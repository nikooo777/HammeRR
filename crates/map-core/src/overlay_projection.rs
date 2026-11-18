use glam::{DVec2, DVec3};

use crate::{Plane, overlay};

pub const CLIP_EPSILON: f64 = 1e-8;

pub struct Frame {
    origin: DVec3,
    u: DVec3,
    v: DVec3,
    normal: DVec3,
    corners: [DVec2; 4],
    clips: [Plane; 4],
    start: DVec2,
    end: DVec2,
}

impl Frame {
    pub fn new(definition: &overlay::Definition) -> Result<Self, String> {
        definition.validate()?;
        let native = |axis: [f64; 3]| DVec3::from_array(axis).as_vec3().as_dvec3();
        let origin = native(definition.basis_origin);
        let u = native(definition.basis_u);
        let normal = native(definition.basis_normal);
        let cross = normal.cross(u);
        let v = if native(definition.basis_v).dot(cross) < 0.0 {
            -cross
        } else {
            cross
        };
        let corners = definition
            .uv_points
            .map(|p| DVec2::new(f64::from(p[0] as f32), f64::from(p[1] as f32)));
        let turn = (corners[1] - corners[0])
            .perp_dot(corners[2] - corners[1])
            .signum();
        let clips = std::array::from_fn(|index| {
            let a = corners[index];
            let edge = (corners[(index + 1) % 4] - a).normalize();
            let normal = (turn * DVec2::new(edge.y, -edge.x)).extend(0.0);
            Plane {
                normal,
                dist: normal.dot(a.extend(0.0)),
            }
        });
        let options = &definition.options;
        Ok(Self {
            origin,
            u,
            v,
            normal,
            corners,
            clips,
            start: DVec2::new(f64::from(options.u[0]), f64::from(options.v[0])),
            end: DVec2::new(f64::from(options.u[1]), f64::from(options.v[1])),
        })
    }

    pub fn point(&self, point: DVec3) -> DVec3 {
        let delta = point - self.origin;
        let projected =
            delta - self.normal * (delta.dot(self.normal) / self.normal.length_squared());
        DVec3::new(
            projected.dot(self.u) / self.u.length_squared(),
            projected.dot(self.v) / self.v.length_squared(),
            0.0,
        )
    }

    pub fn direction(&self) -> DVec3 {
        self.normal
    }

    pub fn clip(&self, mut polygon: Vec<DVec3>) -> Vec<DVec3> {
        for clip in self.clips {
            polygon = clip.clip_behind_with_epsilon(&polygon, CLIP_EPSILON);
            if polygon.len() < 3 {
                break;
            }
        }
        polygon
    }

    pub fn covers(&self, winding: &[DVec3]) -> bool {
        let polygon = self.clip(winding.iter().map(|&point| self.point(point)).collect());
        let Some(first) = polygon.first().map(|point| point.truncate()) else {
            return false;
        };
        polygon[1..].windows(2).any(|edge| {
            let a = edge[0].truncate() - first;
            let b = edge[1].truncate() - first;
            a.perp_dot(b).abs() > CLIP_EPSILON
        })
    }

    pub fn texcoord(&self, point: DVec2) -> Result<DVec2, String> {
        let [a, b, c, d] = self.corners;
        let horizontal = d - a;
        let vertical = b - a;
        let diagonal = c - d - b + a;
        let point = point - a;
        let qa = -horizontal.perp_dot(diagonal);
        let qb = point.perp_dot(diagonal) - horizontal.perp_dot(vertical);
        let qc = point.perp_dot(vertical);
        let scale = qa.abs().max(qb.abs()).max(qc.abs());
        let roots = if qa.abs() <= 1e-12 * scale {
            [-qc / qb, f64::NAN]
        } else {
            let discriminant = qb * qb - 4.0 * qa * qc;
            if discriminant < 0.0 {
                return Err("overlay UV quadrilateral has no real inverse".into());
            }
            let q = -0.5 * (qb + discriminant.sqrt().copysign(qb));
            if q == 0.0 {
                [0.0, f64::NAN]
            } else {
                [q / qa, qc / q]
            }
        };
        const RANGE: std::ops::RangeInclusive<f64> = -1e-6..=1.0 + 1e-6;
        for u in roots {
            if !RANGE.contains(&u) {
                continue;
            }
            let edge = vertical + diagonal * u;
            let v = (point - horizontal * u).dot(edge) / edge.length_squared();
            if RANGE.contains(&v) {
                return Ok(self.start
                    + (self.end - self.start) * DVec2::new(u, v).clamp(DVec2::ZERO, DVec2::ONE));
            }
        }
        Err("overlay UV inversion leaves the clipped quadrilateral".into())
    }
}
