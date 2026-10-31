use glam::DVec3;

use crate::{SceneMesh, scene::LineVertex};

const BOUNDS_COLOR: [u8; 4] = [80, 210, 240, 255];
const CENTER_COLOR: [u8; 4] = [235, 235, 245, 255];
const AXIS_COLORS: [[u8; 4]; 3] = [
    [240, 100, 100, 255],
    [100, 220, 130, 255],
    [110, 160, 255, 255],
];

pub fn append_selection_bounds(mesh: &mut SceneMesh, low: DVec3, high: DVec3) {
    mesh.add_bounds_lines(low, high, BOUNDS_COLOR);
    let center = (low + high) * 0.5;
    let tick = ((high - low).max_element() * 0.025).clamp(2.0, 16.0);
    for (axis, color) in DVec3::AXES.into_iter().zip(AXIS_COLORS) {
        let endpoint = low + axis * (high - low).dot(axis);
        for point in [low, endpoint] {
            mesh.lines.push(LineVertex {
                position: point.as_vec3().to_array(),
                color,
            });
        }
        for point in [center - axis * tick, center + axis * tick] {
            mesh.lines.push(LineVertex {
                position: point.as_vec3().to_array(),
                color: CENTER_COLOR,
            });
        }
    }
}
