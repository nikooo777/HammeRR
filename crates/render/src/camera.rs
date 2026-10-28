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
    pub fn framing(
        mins: Vec3,
        maxs: Vec3,
        direction: Vec3,
        fov_y_degrees: f32,
        aspect: f32,
    ) -> Self {
        let center = (mins + maxs) * 0.5;
        let radius = ((maxs - mins).length() * 0.5).max(16.0);
        let vertical = fov_y_degrees.to_radians() * 0.5;
        let horizontal = (vertical.tan() * aspect).atan();
        let distance = (radius / vertical.min(horizontal).sin()).max(radius + Self::NEAR);
        let mut camera =
            Self::looking_at(center - direction.normalize_or(Vec3::X) * distance, center);
        camera.fov_y_degrees = fov_y_degrees;
        camera
    }

    pub fn frame_bounds(self, mins: Vec3, maxs: Vec3, aspect: f32) -> Self {
        if self.projection == Projection::Perspective {
            return Self {
                position: Self::framing(mins, maxs, self.forward(), self.fov_y_degrees, aspect)
                    .position,
                ..self
            };
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

    pub fn selection_region(
        &self,
        corners: [glam::DVec2; 2],
        aspect: f64,
    ) -> map_core::region::Region {
        let lo = corners[0].min(corners[1]);
        let hi = corners[0].max(corners[1]);
        let forward = self.forward().as_dvec3();
        let right = self.right().as_dvec3();
        let up = self.up().as_dvec3();
        let origin = self.position.as_dvec3();
        let half = if self.projection == Projection::Perspective {
            (f64::from(self.fov_y_degrees).to_radians() * 0.5).tan()
        } else {
            f64::from(self.ortho_height) * 0.5
        };
        let point = |x: f64, y: f64, depth: f64| {
            let span = if self.projection == Projection::Perspective {
                half * depth
            } else {
                half
            };
            origin + forward * depth + right * (x * span * aspect) + up * (y * span)
        };
        let near = f64::from(Self::NEAR);
        let far = f64::from(Self::ORTHO_DISTANCE) * 4.0;
        let front = [(lo.x, lo.y), (hi.x, lo.y), (hi.x, hi.y), (lo.x, hi.y)]
            .map(|(x, y)| point(x, y, near));
        let back =
            [(lo.x, lo.y), (hi.x, lo.y), (hi.x, hi.y), (lo.x, hi.y)].map(|(x, y)| point(x, y, far));
        let seed = point((lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5, (near + far) * 0.5);
        let mut planes: Vec<_> = [[front[0], front[1], front[2]], [back[0], back[1], back[2]]]
            .into_iter()
            .chain((0..4).map(|index| [front[index], front[(index + 1) % 4], back[index]]))
            .filter_map(map_core::Plane::from_points)
            .collect();
        for plane in &mut planes {
            if plane.distance_to(seed) > 0.0 {
                plane.normal = -plane.normal;
                plane.dist = -plane.dist;
            }
        }
        map_core::region::Region { planes, seed }
    }
}
