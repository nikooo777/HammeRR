use glam::{Vec2, Vec3};

use crate::{Camera, Projection};

pub const HANDLE_PIXELS: f32 = 88.0;
const PICK_RADIUS: f32 = 9.0;
const RING_SEGMENTS: usize = 64;
pub const AXIS_COLORS: [[u8; 4]; 3] = [
    [239, 103, 117, 255],
    [100, 219, 155, 255],
    [111, 166, 255, 255],
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Move,
    Rotate,
    Scale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    X,
    Y,
    Z,
    XY,
    XZ,
    YZ,
    View,
    Uniform,
}

impl Handle {
    pub fn name(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
            Self::Z => "z",
            Self::XY => "xy",
            Self::XZ => "xz",
            Self::YZ => "yz",
            Self::View => "view",
            Self::Uniform => "uniform",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "x" => Some(Self::X),
            "y" => Some(Self::Y),
            "z" => Some(Self::Z),
            "xy" => Some(Self::XY),
            "xz" => Some(Self::XZ),
            "yz" => Some(Self::YZ),
            "view" => Some(Self::View),
            "uniform" => Some(Self::Uniform),
            _ => None,
        }
    }

    pub fn axis(self) -> Option<usize> {
        match self {
            Self::X => Some(0),
            Self::Y => Some(1),
            Self::Z => Some(2),
            _ => None,
        }
    }

    pub fn plane(self) -> Option<(usize, usize)> {
        match self {
            Self::XY => Some((0, 1)),
            Self::XZ => Some((0, 2)),
            Self::YZ => Some((1, 2)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guide {
    pub origin: Vec3,
    pub axes: [Vec3; 3],
    pub mode: Mode,
    pub active: Option<Handle>,
}

pub struct Shape {
    pub handle: Handle,
    pub color: [u8; 4],
    pub segments: Vec<[Vec3; 2]>,
}

pub fn project(camera: &Camera, point: Vec3, size: (u32, u32)) -> Option<Vec2> {
    if size.0 == 0 || size.1 == 0 || (point - camera.position).dot(camera.forward()) < Camera::NEAR
    {
        return None;
    }
    let clip = camera.view_projection(size.0 as f32 / size.1 as f32) * point.extend(1.0);
    if !clip.is_finite() || clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some(Vec2::new(
        (ndc.x + 1.0) * size.0 as f32 * 0.5,
        (1.0 - ndc.y) * size.1 as f32 * 0.5,
    ))
}

pub fn length(camera: &Camera, origin: Vec3, height: u32) -> f32 {
    let span = if camera.projection == Projection::Perspective {
        2.0 * (origin - camera.position).dot(camera.forward())
            * (camera.fov_y_degrees.to_radians() * 0.5).tan()
    } else {
        camera.ortho_height
    };
    span / height.max(1) as f32 * HANDLE_PIXELS
}

fn rectangle(center: Vec3, u: Vec3, v: Vec3) -> Vec<[Vec3; 2]> {
    let points = [
        center - u - v,
        center + u - v,
        center + u + v,
        center - u + v,
    ];
    (0..4)
        .map(|index| [points[index], points[(index + 1) % 4]])
        .collect()
}

pub fn shapes(camera: &Camera, size: (u32, u32), guide: &Guide) -> Vec<Shape> {
    if project(camera, guide.origin, size).is_none() {
        return Vec::new();
    }
    let span = length(camera, guide.origin, size.1);
    let mut result = Vec::new();
    for (index, handle) in [Handle::X, Handle::Y, Handle::Z].into_iter().enumerate() {
        let axis = guide.axes[index];
        let alignment = axis.dot(camera.forward()).abs();
        let mut segments = Vec::new();
        if guide.mode == Mode::Rotate {
            if alignment < 0.15 {
                continue;
            }
            let u = guide.axes[(index + 1) % 3] * span;
            let v = guide.axes[(index + 2) % 3] * span;
            let point = |tick: usize| {
                let angle = tick as f32 * std::f32::consts::TAU / RING_SEGMENTS as f32;
                guide.origin + u * angle.cos() + v * angle.sin()
            };
            segments.extend((0..RING_SEGMENTS).map(|tick| [point(tick), point(tick + 1)]));
        } else {
            if alignment > 0.98 {
                continue;
            }
            let tip = guide.origin + axis * span;
            segments.push([guide.origin + axis * span * 0.15, tip]);
            let wing = axis.cross(camera.forward()).normalize_or(camera.right()) * span * 0.065;
            if guide.mode == Mode::Move {
                for sign in [-1.0, 1.0] {
                    segments.push([tip, tip - axis * span * 0.18 + wing * sign]);
                }
            } else {
                let vertical = camera.up() * span * 0.055;
                segments.extend(rectangle(tip, camera.right() * span * 0.055, vertical));
            }
        }
        result.push(Shape {
            handle,
            color: if guide.active == Some(handle) {
                [255, 225, 113, 255]
            } else {
                AXIS_COLORS[index]
            },
            segments,
        });
    }
    if guide.mode == Mode::Move {
        for (handle, a, b) in [(Handle::XY, 0, 1), (Handle::XZ, 0, 2), (Handle::YZ, 1, 2)] {
            if guide.axes[a]
                .cross(guide.axes[b])
                .dot(camera.forward())
                .abs()
                < 0.15
            {
                continue;
            }
            let u = guide.axes[a] * span;
            let v = guide.axes[b] * span;
            result.push(Shape {
                handle,
                color: [170, 195, 222, 255],
                segments: rectangle(guide.origin + (u + v) * 0.3, u * 0.09, v * 0.09),
            });
        }
    }
    if guide.mode != Mode::Rotate {
        result.push(Shape {
            handle: if guide.mode == Mode::Move {
                Handle::View
            } else {
                Handle::Uniform
            },
            color: [232, 236, 245, 255],
            segments: rectangle(
                guide.origin,
                camera.right() * span * 0.07,
                camera.up() * span * 0.07,
            ),
        });
    }
    result
}

pub fn pick(camera: &Camera, size: (u32, u32), guide: &Guide, point: Vec2) -> Option<Handle> {
    shapes(camera, size, guide)
        .into_iter()
        .filter_map(|shape| {
            let distance = shape
                .segments
                .iter()
                .filter_map(|[a, b]| {
                    let a = project(camera, *a, size)?;
                    let b = project(camera, *b, size)?;
                    let delta = b - a;
                    let t =
                        ((point - a).dot(delta) / delta.length_squared().max(1e-8)).clamp(0.0, 1.0);
                    Some((point - a - delta * t).length())
                })
                .min_by(f32::total_cmp)?;
            (distance <= PICK_RADIUS).then_some((shape.handle, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(handle, _)| handle)
}
