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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_outline_keeps_pixel_radius_in_every_projection() {
        for projection in [
            Projection::Top,
            Projection::Front,
            Projection::Side,
            Projection::Perspective,
        ] {
            let mut camera = Camera::looking_at(Vec3::new(-300.0, -400.0, 250.0), Vec3::ZERO);
            if projection != Projection::Perspective {
                camera = Camera::planar(projection, Vec3::ZERO, 512.0);
            }
            let size = (800, 600);
            let center = glam::Vec2::new(340.0, 290.0);
            let options = ViewOptions {
                grid_visible: false,
                paint_preview: Some(crate::PaintGuide {
                    center,
                    radius: 24.0,
                    size,
                    camera,
                }),
                ..Default::default()
            };
            let (lines, count) = overlay_lines(&camera, size, &options);
            assert_eq!(count, 0);
            assert_eq!(lines.len(), 96);
            for vertex in &lines {
                let pixel =
                    crate::handles::project(&camera, Vec3::from_array(vertex.position), size)
                        .unwrap();
                assert!((pixel.distance(center) - 24.0).abs() < 0.02);
            }
            assert_eq!(lines, overlay_lines(&camera, (1600, 1200), &options).0);
            camera.position.x += 1.0;
            assert!(overlay_lines(&camera, size, &options).0.is_empty());
        }
    }

    #[test]
    fn overlays_keep_logical_handle_size_and_original_selection_aspect() {
        let camera = Camera::planar(Projection::Top, Vec3::ZERO, 512.0);
        let options = ViewOptions {
            grid_visible: false,
            transform_handles: Some(crate::handles::Guide {
                origin: Vec3::ZERO,
                axes: [Vec3::X, Vec3::Y, Vec3::Z],
                mode: crate::handles::Mode::Move,
                active: None,
            }),
            ..Default::default()
        };
        let (logical, _) = overlay_lines(&camera, (1200, 800), &options);
        let (physical, _) = overlay_lines(
            &camera,
            (2400, 1600),
            &ViewOptions {
                handle_pixel_ratio: 2.0,
                ..options
            },
        );
        assert_eq!(logical, physical);
        let options = ViewOptions {
            grid_visible: false,
            area_preview: Some(crate::AreaGuide {
                corners: [glam::Vec2::splat(-0.5), glam::Vec2::splat(0.5)],
                camera,
                aspect: 2.0,
                contained: true,
            }),
            ..Default::default()
        };
        let (lines, count) = overlay_lines(&camera, (1000, 1000), &options);
        assert_eq!(count, 0);
        assert_eq!(lines.len(), 8);
        let left = Vec3::from_array(lines[0].position);
        assert!((left.x + 256.0).abs() < 1e-4);
        assert!((left.y + 128.0).abs() < 1e-4);
        let other = Camera::planar(Projection::Front, Vec3::ZERO, 512.0);
        assert!(overlay_lines(&other, (1000, 1000), &options).0.is_empty());
    }

    #[test]
    fn grid_stays_bounded_when_fractional_spacing_is_zoomed_out() {
        let camera = Camera::planar(Projection::Top, Vec3::ZERO, Camera::MAX_ORTHO_HEIGHT);
        let options = ViewOptions {
            grid_size: 0.125,
            ..ViewOptions::default()
        };
        let (lines, count) = overlay_lines(&camera, (1920, 1080), &options);
        assert!(count > 0 && count < 1000);
        assert!(
            lines
                .iter()
                .all(|line| line.position[2] == options.grid_elevation)
        );
    }

    #[test]
    fn hidden_grid_keeps_brush_preview_visible() {
        let options = ViewOptions {
            grid_visible: false,
            block_preview: Some([Vec3::ZERO, Vec3::new(32.0, 64.0, 128.0)]),
            ..ViewOptions::default()
        };
        let (lines, grid_count) = overlay_lines(
            &Camera::planar(Projection::Front, Vec3::ZERO, 512.0),
            (800, 600),
            &options,
        );
        assert_eq!(grid_count, 0);
        assert_eq!(lines.len(), 24);
    }

    #[test]
    fn cut_plane_and_arrow_remain_visible_without_the_grid() {
        for (projection, normal) in [
            (Projection::Top, Vec3::X),
            (Projection::Front, Vec3::Z),
            (Projection::Side, Vec3::Y),
        ] {
            let options = ViewOptions {
                grid_visible: false,
                clip_preview: Some(crate::ClipGuide {
                    normal,
                    distance: 0.0,
                    bounds: [Vec3::splat(-32.0), Vec3::splat(32.0)],
                }),
                ..Default::default()
            };
            let camera = Camera::planar(projection, Vec3::ZERO, 512.0);
            let (lines, grid_count) = overlay_lines(&camera, (800, 600), &options);
            assert_eq!(grid_count, 0);
            assert_eq!(lines.len(), 14);
            assert!(
                lines
                    .iter()
                    .all(|line| Vec3::from_array(line.position).is_finite())
            );
            let wings = Vec3::from_array(lines[11].position) - Vec3::from_array(lines[13].position);
            assert!((wings.length() - 16.0).abs() < 1e-5);
            assert!(wings.dot(camera.forward()).abs() < 1e-5);
        }
    }
}
