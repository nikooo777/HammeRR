use std::io;

use super::DisplacementInfo;
use crate::bsp::invalid;

const CHILDREN: [[i32; 2]; 4] = [[1, 1], [-1, 1], [-1, -1], [1, -1]];
const SIDES: [[i32; 2]; 4] = [[1, 0], [0, 1], [-1, 0], [0, -1]];
const WINDING: [([i32; 2], Option<usize>); 9] = [
    ([1, -1], Some(3)),
    ([0, -1], None),
    ([-1, -1], Some(2)),
    ([-1, 0], None),
    ([-1, 1], Some(1)),
    ([0, 1], None),
    ([1, 1], Some(0)),
    ([1, 0], None),
    ([1, -1], Some(3)),
];

impl DisplacementInfo {
    pub fn allowed_triangle_indices(self) -> io::Result<Vec<[usize; 3]>> {
        let size = self.grid_size()?;
        let center = (size / 2) as i32;
        let mut triangles = Vec::with_capacity(self.triangle_count()?);
        if !self.vertex_allowed(size * (size / 2) + size / 2) {
            return Err(invalid("displacement LOD root must be allowed"));
        }
        for vertex in [0, size - 1, size * (size - 1), size * size - 1] {
            if !self.vertex_allowed(vertex) {
                return Err(invalid("displacement LOD corners must be allowed"));
            }
        }
        self.tessellate_node([center; 2], size / 2, size, &mut triangles)?;
        Ok(triangles)
    }

    pub fn vertex_allowed(self, index: usize) -> bool {
        if !(2..=4).contains(&self.power) || index >= ((1usize << self.power) + 1).pow(2) {
            return false;
        }
        self.allowed_vertices
            .get(index / 32)
            .is_some_and(|bits| bits & (1 << (index % 32)) != 0)
    }

    fn tessellate_node(
        self,
        center: [i32; 2],
        step: usize,
        size: usize,
        triangles: &mut Vec<[usize; 3]>,
    ) -> io::Result<()> {
        let index = |point: [i32; 2]| point[1] as usize * size + point[0] as usize;
        let point = |offset: [i32; 2], scale: usize| {
            std::array::from_fn(|axis| center[axis] + offset[axis] * scale as i32)
        };
        let mut active_children = [false; 4];
        if step > 1 {
            for (child, offset) in CHILDREN.into_iter().enumerate() {
                let child_center = point(offset, step / 2);
                if self.vertex_allowed(index(child_center)) {
                    if !self.vertex_allowed(index(point(SIDES[child], step)))
                        || !self.vertex_allowed(index(point(SIDES[(child + 1) % 4], step)))
                    {
                        return Err(invalid(
                            "displacement LOD child is missing a required side vertex",
                        ));
                    }
                    active_children[child] = true;
                    self.tessellate_node(child_center, step / 2, size, triangles)?;
                }
            }
        }
        let mut previous = None;
        for (offset, child) in WINDING {
            if child.is_some_and(|child| active_children[child]) {
                previous = None;
                continue;
            }
            let vertex = index(point(offset, step));
            if self.vertex_allowed(vertex) {
                if let Some(previous) = previous {
                    triangles.push([previous, vertex, index(center)]);
                }
                previous = Some(vertex);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrestricted_tessellation_matches_native_triangles_at_every_power() {
        for power in 2..=4 {
            let info = DisplacementInfo {
                power,
                ..Default::default()
            };
            let canonical = |triangles: Vec<[usize; 3]>| {
                triangles
                    .into_iter()
                    .map(|mut triangle| {
                        let start = (0..3).min_by_key(|&i| triangle[i]).unwrap();
                        triangle.rotate_left(start);
                        triangle
                    })
                    .collect::<std::collections::BTreeSet<_>>()
            };
            assert_eq!(
                canonical(info.allowed_triangle_indices().unwrap()),
                canonical(info.triangle_indices().unwrap())
            );
        }
    }

    #[test]
    fn invalid_roots_corners_and_powers_are_rejected() {
        for index in [0, 4, 20, 24, 12] {
            let mut info = DisplacementInfo::default();
            info.allowed_vertices[index / 32] &= !(1 << (index % 32));
            assert!(info.allowed_triangle_indices().is_err());
        }
        assert!(
            DisplacementInfo {
                power: 5,
                ..Default::default()
            }
            .allowed_triangle_indices()
            .is_err()
        );
        let mut missing_dependency = DisplacementInfo::default();
        missing_dependency.allowed_vertices[0] &= !(1 << 14);
        assert!(missing_dependency.allowed_triangle_indices().is_err());
        assert!(!DisplacementInfo::default().vertex_allowed(25));
    }
}
