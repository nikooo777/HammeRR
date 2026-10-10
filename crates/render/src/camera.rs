use glam::{Mat4, Vec3};
use source_formats::vmf::{Vmf, parse_vec3};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    #[default]
    Perspective,
    Top,
    Front,
    Side,
}

impl Projection {
    pub fn name(self) -> &'static str {
        match self {
            Self::Perspective => "perspective",
            Self::Top => "top",
            Self::Front => "front",
            Self::Side => "side",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "perspective" => Some(Self::Perspective),
            "top" => Some(Self::Top),
            "front" => Some(Self::Front),
            "side" => Some(Self::Side),
            _ => None,
        }
    }

    pub fn axes(self) -> Option<(usize, usize, usize)> {
        match self {
            Self::Perspective => None,
            Self::Top => Some((0, 1, 2)),
            Self::Front => Some((0, 2, 1)),
            Self::Side => Some((1, 2, 0)),
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Perspective => 0,
            Self::Top => 1,
            Self::Front => 2,
            Self::Side => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub position: Vec3,
    pub yaw_degrees: f32,
    pub pitch_degrees: f32,
    pub fov_y_degrees: f32,
    pub projection: Projection,
    pub ortho_height: f32,
}

impl Camera {
    pub const NEAR: f32 = 4.0;
    pub const ORTHO_DISTANCE: f32 = 65_536.0;
    pub const MIN_ORTHO_HEIGHT: f32 = 1.0;
    pub const MAX_ORTHO_HEIGHT: f32 = 131_072.0;

    pub fn looking_at(position: Vec3, target: Vec3) -> Self {
        let dir = (target - position).normalize_or(Vec3::X);
        Self {
            position,
            yaw_degrees: dir.y.atan2(dir.x).to_degrees(),
            pitch_degrees: dir.z.clamp(-1.0, 1.0).asin().to_degrees(),
            fov_y_degrees: 60.0,
            projection: Projection::Perspective,
            ortho_height: 1024.0,
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
        match self.projection {
            Projection::Top => return Vec3::NEG_Z,
            Projection::Front => return Vec3::Y,
            Projection::Side => return Vec3::NEG_X,
            Projection::Perspective => {}
        }
        let (yaw, pitch) = (
            self.yaw_degrees.to_radians(),
            self.pitch_degrees.to_radians(),
        );
        Vec3::new(
            pitch.cos() * yaw.cos(),
            pitch.cos() * yaw.sin(),
            pitch.sin(),
        )
    }

    pub fn right(&self) -> Vec3 {
        if self.projection == Projection::Top {
            return Vec3::X;
        }
        self.forward().cross(Vec3::Z).normalize_or(Vec3::Y)
    }

    pub fn up(&self) -> Vec3 {
        self.right().cross(self.forward()).normalize()
    }

    pub fn planar(projection: Projection, center: Vec3, height: f32) -> Self {
        let mut camera = Self::looking_at(Vec3::ZERO, Vec3::X);
        camera.projection = projection;
        let direction = camera.forward();
        camera.yaw_degrees = direction.y.atan2(direction.x).to_degrees();
        camera.pitch_degrees = direction.z.asin().to_degrees();
        camera.ortho_height = height.clamp(Self::MIN_ORTHO_HEIGHT, Self::MAX_ORTHO_HEIGHT);
        camera.position = center - camera.forward() * Self::ORTHO_DISTANCE;
        camera
    }

    /// Reverse-Z projection with an infinite far plane: depth 1 at the near
    /// plane, 0 at infinity. Keeps precision across Source's large maps.
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        use glam::camera::rh::{proj::directx, view};
        let view = view::look_at_mat4(self.position, self.position + self.forward(), self.up());
        if self.projection != Projection::Perspective {
            let half = self.ortho_height * 0.5;
            return directx::orthographic(
                -half * aspect,
                half * aspect,
                -half,
                half,
                Self::ORTHO_DISTANCE * 2.0,
                Self::NEAR,
            ) * view;
        }
        // wgpu clip space is Y-up with depth in [0, 1], glam's DirectX convention.
        let proj = directx::perspective_infinite_reverse(
            self.fov_y_degrees.to_radians(),
            aspect,
            Self::NEAR,
        );
        proj * view
    }

    /// Frames a bounding box from `direction` (pointing from the camera
    /// toward the box) so all of it fits in view.
    pub fn framing(mins: Vec3, maxs: Vec3, direction: Vec3, fov_y_degrees: f32) -> Self {
        let center = (mins + maxs) * 0.5;
        let radius = ((maxs - mins).length() * 0.5).max(16.0);
        let distance = radius / (fov_y_degrees.to_radians() * 0.5).sin();
        let mut camera =
            Self::looking_at(center - direction.normalize_or(Vec3::X) * distance, center);
        camera.fov_y_degrees = fov_y_degrees;
        camera
    }

    pub fn frame_bounds(self, mins: Vec3, maxs: Vec3, aspect: f32) -> Self {
        if self.projection == Projection::Perspective {
            return Self::framing(mins, maxs, self.forward(), self.fov_y_degrees);
        }
        let size = maxs - mins;
        let height = size
            .dot(self.up().abs())
            .max(size.dot(self.right().abs()) / aspect);
        Self::planar(
            self.projection,
            (mins + maxs) * 0.5,
            (height * 1.2).max(64.0),
        )
    }

    /// World-space ray through a viewport point given in normalized device
    /// coordinates (`-1..1`, +y up).
    pub fn ray(&self, ndc_x: f32, ndc_y: f32, aspect: f32) -> (Vec3, Vec3) {
        let forward = self.forward();
        let right = self.right();
        let up = self.up();
        if self.projection != Projection::Perspective {
            let half = self.ortho_height * 0.5;
            return (
                self.position + right * (ndc_x * half * aspect) + up * (ndc_y * half),
                forward,
            );
        }
        let half = (self.fov_y_degrees.to_radians() * 0.5).tan();
        let dir = forward + right * (ndc_x * half * aspect) + up * (ndc_y * half);
        (self.position, dir.normalize())
    }

    /// Moves relative to the view: `amount.x` forward, `.y` right, `.z` up.
    pub fn fly(&mut self, amount: Vec3) {
        self.position += self.forward() * amount.x + self.right() * amount.y + Vec3::Z * amount.z;
    }

    pub fn look(&mut self, yaw_delta: f32, pitch_delta: f32) {
        if self.projection != Projection::Perspective {
            return;
        }
        self.yaw_degrees = (self.yaw_degrees + yaw_delta).rem_euclid(360.0);
        self.pitch_degrees = (self.pitch_degrees + pitch_delta).clamp(-89.0, 89.0);
    }

    pub fn plane_point(&self, ndc: (f32, f32), aspect: f32, elevation: f32) -> Option<Vec3> {
        let (_, _, axis) = self.projection.axes()?;
        let (mut point, _) = self.ray(ndc.0, ndc.1, aspect);
        point[axis] = elevation;
        Some(point)
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

    #[test]
    fn rays_unproject_through_the_matching_screen_point() {
        let cam = Camera::looking_at(Vec3::new(10.0, -20.0, 30.0), Vec3::new(200.0, 50.0, 0.0));
        let aspect = 16.0 / 9.0;
        let vp = cam.view_projection(aspect);
        let (origin, dir) = cam.ray(0.3, -0.4, aspect);
        let projected = vp.project_point3(origin + dir * 500.0);
        assert!((projected.x - 0.3).abs() < 1e-4 && (projected.y + 0.4).abs() < 1e-4);
    }

    #[test]
    fn planar_views_have_matching_projection_rays_and_axis_orientation() {
        for projection in [Projection::Top, Projection::Front, Projection::Side] {
            let camera = Camera::planar(projection, Vec3::new(-120.0, 80.0, 32.0), 512.0);
            let aspect = 1.7;
            let matrix = camera.view_projection(aspect);
            let (origin, direction) = camera.ray(0.3, -0.4, aspect);
            let point = matrix.project_point3(origin + direction * Camera::ORTHO_DISTANCE);
            assert!((point.x - 0.3).abs() < 1e-4);
            assert!((point.y + 0.4).abs() < 1e-4);
            assert!(point.z > 0.0 && point.z < 1.0);
            let (_, other_direction) = camera.ray(-0.7, 0.6, aspect);
            assert_eq!(direction, other_direction);
            let (horizontal, vertical, normal) = projection.axes().unwrap();
            assert_eq!(camera.right()[horizontal], 1.0);
            assert_eq!(camera.up()[vertical], 1.0);
            let plane = camera.plane_point((0.3, -0.4), aspect, -64.0).unwrap();
            assert_eq!(plane[normal], -64.0);
        }
    }

    #[test]
    fn planar_framing_fits_wide_and_tall_bounds() {
        for projection in [Projection::Top, Projection::Front, Projection::Side] {
            for aspect in [0.5, 2.0] {
                let camera = Camera::planar(projection, Vec3::ZERO, 100.0).frame_bounds(
                    Vec3::splat(-256.0),
                    Vec3::new(1024.0, 512.0, 128.0),
                    aspect,
                );
                let matrix = camera.view_projection(aspect);
                for x in [-256.0, 1024.0] {
                    for y in [-256.0, 512.0] {
                        for z in [-256.0, 128.0] {
                            let point = matrix.project_point3(Vec3::new(x, y, z));
                            assert!(point.x.abs() <= 1.0 && point.y.abs() <= 1.0);
                        }
                    }
                }
            }
        }
    }
}
