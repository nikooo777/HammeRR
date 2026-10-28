use glam::DVec3;

pub(super) const DISTANCE_WORK: usize = 4;

#[derive(Debug)]
pub(super) struct Triangle {
    corners: [DVec3; 3],
    normal: DVec3,
}

impl Triangle {
    pub(super) fn new(corners: [DVec3; 3]) -> Result<Self, String> {
        if corners.iter().any(|point| !point.is_finite()) {
            return Err("Native physics emitted a non-finite triangle".into());
        }
        let area = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
        let length = area.length();
        if !length.is_finite() || length == 0.0 {
            return Err("Native physics emitted a degenerate triangle".into());
        }
        Ok(Self {
            corners,
            normal: area / length,
        })
    }

    pub(super) fn normal(&self) -> DVec3 {
        self.normal
    }

    pub(super) fn distance_squared(&self, point: DVec3) -> Result<f64, String> {
        if !point.is_finite() {
            return Err("Physics hull distance requires a finite source corner".into());
        }
        let offset = self.normal.dot(point - self.corners[0]);
        let projected = point - self.normal * offset;
        let mut inside = true;
        let mut nearest = f64::INFINITY;
        for index in 0..self.corners.len() {
            let start = self.corners[index];
            let direction = self.corners[(index + 1) % self.corners.len()] - start;
            inside &= direction.cross(projected - start).dot(self.normal) >= 0.0;
            let fraction = (point - start).dot(direction) / direction.length_squared();
            let closest = start + direction * fraction.clamp(0.0, 1.0);
            nearest = nearest.min(point.distance_squared(closest));
        }
        if inside {
            nearest = offset * offset;
        }
        if !nearest.is_finite() || nearest < 0.0 {
            return Err("Native physics hull distance is not finite".into());
        }
        Ok(nearest)
    }
}
