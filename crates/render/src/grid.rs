use glam::Vec3;

use crate::{Camera, Projection, ViewOptions, scene::LineVertex};

const MIN_CELL_PIXELS: f32 = 12.0;
const MINOR_COLOR: [u8; 4] = [10, 12, 16, 255];
const MAJOR_COLOR: [u8; 4] = [22, 27, 36, 255];
const AXIS_COLORS: [[u8; 4]; 3] = [[153, 69, 77, 255], [68, 135, 100, 255], [76, 114, 176, 255]];
const PREVIEW_COLOR: [u8; 4] = [100, 210, 255, 255];

fn line(vertices: &mut Vec<LineVertex>, a: Vec3, b: Vec3, color: [u8; 4]) {
    vertices.extend([a, b].map(|point| LineVertex {
        position: point.to_array(),
        color,
    }));
}

pub(crate) fn overlay_lines(
    camera: &Camera,
    size: (u32, u32),
    options: &ViewOptions,
) -> (Vec<LineVertex>, usize) {
    let mut vertices = Vec::new();
    if options.grid_visible {
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        let (horizontal, vertical, normal) = camera.projection.axes().unwrap_or((0, 1, 2));
        let mut center = camera.position;
        let height = if camera.projection == Projection::Perspective {
            center += camera.forward() * 384.0;
            options.grid_size * 64.0
        } else {
            camera.ortho_height
        };
        let minimum =
            (height / size.1.max(1) as f32 * MIN_CELL_PIXELS).max(height * aspect.max(1.0) / 256.0);
        let mut spacing = options.grid_size.max(0.125);
        while spacing < minimum {
            spacing *= 2.0;
        }
        let extent = [height * aspect * 0.5 + spacing, height * 0.5 + spacing];
        let axes = [horizontal, vertical];
        for index in 0..2 {
            let axis = axes[index];
            let other = axes[1 - index];
            let start = ((center[axis] - extent[index]) / spacing).floor() as i32;
            let end = ((center[axis] + extent[index]) / spacing).ceil() as i32;
            for tick in start..=end {
                let mut a = Vec3::ZERO;
                a[axis] = tick as f32 * spacing;
                a[other] = center[other] - extent[1 - index];
                a[normal] = options.grid_elevation;
                let mut b = a;
                b[other] = center[other] + extent[1 - index];
                let color = if tick == 0 {
                    AXIS_COLORS[other]
                } else if tick % 8 == 0 {
                    MAJOR_COLOR
                } else {
                    MINOR_COLOR
                };
                line(&mut vertices, a, b, color);
            }
        }
    }
    let grid_count = vertices.len();
    if let Some([mins, maxs]) = options.block_preview {
        for axis in 0..3 {
            let a = (axis + 1) % 3;
            let b = (axis + 2) % 3;
            for side_a in [mins[a], maxs[a]] {
                for side_b in [mins[b], maxs[b]] {
                    let mut start = mins;
                    start[a] = side_a;
                    start[b] = side_b;
                    let mut end = start;
                    end[axis] = maxs[axis];
                    line(&mut vertices, start, end, PREVIEW_COLOR);
                }
            }
        }
    }
    if let Some(guide) = options.clip_preview {
        let plane = map_core::Plane {
            normal: guide.normal.as_dvec3(),
            dist: f64::from(guide.distance),
        };
        let mut points = plane.base_winding();
        let mins = guide.bounds[0].as_dvec3() - glam::DVec3::splat(16.0);
        let maxs = guide.bounds[1].as_dvec3() + glam::DVec3::splat(16.0);
        for axis in 0..3 {
            let mut normal = glam::DVec3::ZERO;
            normal[axis] = 1.0;
            points = map_core::Plane {
                normal,
                dist: maxs[axis],
            }
            .clip_behind(&points);
            points = map_core::Plane {
                normal: -normal,
                dist: -mins[axis],
            }
            .clip_behind(&points);
        }
        for (a, b) in points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .take(points.len())
        {
            line(&mut vertices, a.as_vec3(), b.as_vec3(), PREVIEW_COLOR);
        }
        if !points.is_empty() {
            let center = (points.iter().sum::<glam::DVec3>() / points.len() as f64).as_vec3();
            let tip = center + guide.normal * 48.0;
            let wing = guide
                .normal
                .cross(camera.forward())
                .try_normalize()
                .unwrap_or(camera.right())
                * 8.0;
            line(&mut vertices, center, tip, PREVIEW_COLOR);
            for side in [-wing, wing] {
                line(
                    &mut vertices,
                    tip,
                    tip - guide.normal * 12.0 + side,
                    PREVIEW_COLOR,
                );
            }
        }
    }
    if let Some(guide) = options.transform_handles {
        let logical_size = (
            (size.0 as f32 / options.handle_pixel_ratio)
                .round()
                .max(1.0) as u32,
            (size.1 as f32 / options.handle_pixel_ratio)
                .round()
                .max(1.0) as u32,
        );
        for shape in crate::handles::shapes(camera, logical_size, &guide) {
            for [a, b] in shape.segments {
                line(&mut vertices, a, b, shape.color);
            }
        }
    }
    if let Some(guide) = options.area_preview.filter(|guide| guide.camera == *camera) {
        let [start, end] = guide.corners;
        let corners = [
            start,
            Vec3::new(end.x, start.y, 0.0).truncate(),
            end,
            Vec3::new(start.x, end.y, 0.0).truncate(),
        ];
        let depth = if camera.projection == Projection::Perspective {
            Camera::NEAR * 2.0
        } else {
            Camera::ORTHO_DISTANCE
        };
        let points = corners.map(|point| {
            let (origin, direction) = camera.ray(point.x, point.y, guide.aspect);
            origin + direction * depth / direction.dot(camera.forward())
        });
        for index in 0..4 {
            line(
                &mut vertices,
                points[index],
                points[(index + 1) % 4],
                if guide.contained {
                    [100, 219, 155, 255]
                } else {
                    PREVIEW_COLOR
                },
            );
        }
    }
    if let Some(guide) = options
        .paint_preview
        .filter(|guide| guide.camera == *camera)
    {
        let depth = if camera.projection == Projection::Perspective {
            Camera::NEAR * 2.0
        } else {
            Camera::ORTHO_DISTANCE
        };
        let point = |angle: f32| {
            let pixel = guide.center + glam::Vec2::new(angle.cos(), angle.sin()) * guide.radius;
            let (origin, direction) = camera.ray(
                2.0 * pixel.x / guide.size.0 as f32 - 1.0,
                1.0 - 2.0 * pixel.y / guide.size.1 as f32,
                guide.size.0 as f32 / guide.size.1 as f32,
            );
            origin + direction * depth / direction.dot(camera.forward())
        };
        for segment in 0..48 {
            line(
                &mut vertices,
                point(segment as f32 * std::f32::consts::TAU / 48.0),
                point((segment + 1) as f32 * std::f32::consts::TAU / 48.0),
                [255, 214, 110, 255],
            );
        }
    }
    (vertices, grid_count)
}
