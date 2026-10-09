use glam::{Mat4, Vec3};
use source_formats::vmf::{Vmf, parse_vec3};

/// A Z-up fly camera. Yaw 0 looks down +X; positive pitch looks up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub position: Vec3,
    pub yaw_degrees: f32,
    pub pitch_degrees: f32,
    pub fov_y_degrees: f32,
}

impl Camera {
    pub const NEAR: f32 = 4.0;

    pub fn looking_at(position: Vec3, target: Vec3) -> Self {
        let dir = (target - position).normalize_or(Vec3::X);
        Self {
            position,
            yaw_degrees: dir.y.atan2(dir.x).to_degrees(),
            pitch_degrees: dir.z.clamp(-1.0, 1.0).asin().to_degrees(),
            fov_y_degrees: 60.0,
        }
    }

    /// The active camera Hammer saved in the map, if any.
    pub fn from_vmf(vmf: &Vmf) -> Option<Self> {
        let cameras = vmf.cameras.as_ref()?.children()?;
        let active: usize = cameras
            .iter()
            .find(|n| n.key.eq_ignore_ascii_case("activecamera"))
            .and_then(|n| n.as_str()?.parse().ok())?;
        let camera = cameras
            .iter()
            .filter(|n| n.key.eq_ignore_ascii_case("camera"))
            .nth(active)?
            .children()?;
        let vec = |key: &str| {
            let v = parse_vec3(camera.iter().find(|n| n.key == key)?.as_str()?)?;
            Some(v.as_vec3())
        };
        Some(Self::looking_at(vec("position")?, vec("look")?))
    }

    /// Looks at a bounding box from above one corner, far enough to see it all.
    pub fn overview(mins: Vec3, maxs: Vec3) -> Self {
        let center = (mins + maxs) * 0.5;
        let radius = (maxs - mins).length() * 0.5;
        let offset = Vec3::new(-1.0, -1.0, 0.8).normalize() * radius * 1.4;
        Self::looking_at(center + offset, center)
    }

    pub fn forward(&self) -> Vec3 {
        let (yaw, pitch) = (self.yaw_degrees.to_radians(), self.pitch_degrees.to_radians());
        Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin())
    }

    pub fn right(&self) -> Vec3 {
        self.forward().cross(Vec3::Z).normalize_or(Vec3::Y)
    }

    /// Reverse-Z projection with an infinite far plane: depth 1 at the near
    /// plane, 0 at infinity. Keeps precision across Source's large maps.
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        use glam::camera::rh::{proj::directx, view};
        let view = view::look_at_mat4(self.position, self.position + self.forward(), Vec3::Z);
        // wgpu clip space is Y-up with depth in [0, 1], glam's DirectX convention.
        let proj =
            directx::perspective_infinite_reverse(self.fov_y_degrees.to_radians(), aspect, Self::NEAR);
        proj * view
    }

    /// Moves relative to the view: `amount.x` forward, `.y` right, `.z` up.
    pub fn fly(&mut self, amount: Vec3) {
        self.position += self.forward() * amount.x + self.right() * amount.y + Vec3::Z * amount.z;
    }

    pub fn look(&mut self, yaw_delta: f32, pitch_delta: f32) {
        self.yaw_degrees = (self.yaw_degrees + yaw_delta).rem_euclid(360.0);
        self.pitch_degrees = (self.pitch_degrees + pitch_delta).clamp(-89.0, 89.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looking_at_points_the_camera_at_the_target() {
        let cam = Camera::looking_at(Vec3::ZERO, Vec3::new(0.0, 10.0, 10.0));
        assert!((cam.forward() - Vec3::new(0.0, 1.0, 1.0).normalize()).length() < 1e-5);
        assert!((cam.right() - Vec3::X).length() < 1e-5);
    }

    #[test]
    fn projection_puts_targets_in_front_with_reverse_depth() {
        let cam = Camera::looking_at(Vec3::ZERO, Vec3::X);
        let vp = cam.view_projection(1.0);
        let near = vp.project_point3(Vec3::new(Camera::NEAR, 0.0, 0.0));
        let far = vp.project_point3(Vec3::new(10_000.0, 0.0, 0.0));
        assert!((near.z - 1.0).abs() < 1e-5);
        assert!(far.z > 0.0 && far.z < near.z);
        let up = vp.project_point3(Vec3::new(100.0, 0.0, 10.0));
        assert!(up.y > 0.0, "+Z is up on screen");
        let left = vp.project_point3(Vec3::new(100.0, 10.0, 0.0));
        assert!(left.x < 0.0, "+Y is to the left when looking down +X");
    }
}
