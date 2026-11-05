use std::collections::VecDeque;

use glam::Vec3;
use map_core::rope_collision::{
    MAX_BRUSHES, MAX_STATIONARY_REFERENCES, MAX_STATIONARY_TREE_NODES, StationaryNode,
    StationaryTopology, TerrainPatch,
};

use super::Map;
use crate::{
    Control,
    tree::{Limits, Tree},
};

const TREE_LIMITS: Limits = Limits {
    nodes: MAX_STATIONARY_TREE_NODES,
    pending: MAX_BRUSHES,
    points: 800_000,
    work: 16_000_000,
};
const MAX_MEMBERSHIP_WORK: usize = 16_000_000;

fn children_for_bounds(plane: map_core::Plane, patch: &TerrainPatch) -> [bool; 2] {
    let mins = patch.mins.as_vec3();
    let maxs = patch.maxs.as_vec3();
    let normal = plane.normal.as_vec3();
    let distance = plane.dist as f32;
    for (axis, direction) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().enumerate() {
        if normal == direction {
            return if mins[axis] >= distance {
                [true, false]
            } else if maxs[axis] <= distance {
                [false, true]
            } else {
                [true, true]
            };
        }
    }
    let positive = Vec3::new(
        if normal.x < 0.0 { mins.x } else { maxs.x },
        if normal.y < 0.0 { mins.y } else { maxs.y },
        if normal.z < 0.0 { mins.z } else { maxs.z },
    );
    let negative = Vec3::new(
        if normal.x < 0.0 { maxs.x } else { mins.x },
        if normal.y < 0.0 { maxs.y } else { mins.y },
        if normal.z < 0.0 { maxs.z } else { mins.z },
    );
    [
        normal.dot(positive) >= distance,
        normal.dot(negative) < distance,
    ]
}

impl Map<'_> {
    pub fn prepare_rope_collision_topology(
        &mut self,
        patches: &[TerrainPatch],
        collision_mask: i32,
        control: &Control,
    ) -> Result<StationaryTopology, String> {
        control.checkpoint()?;
        if patches.len() != self.terrains.len()
            || patches
                .iter()
                .zip(&self.terrains)
                .any(|(patch, terrain)| patch.solid != terrain.solid || patch.side != terrain.side)
        {
            return Err("Rope stationary topology needs native terrain order".into());
        }
        let structural = self.entities[0]
            .brushes
            .iter()
            .copied()
            .filter(|&index| self.brushes[index].structural())
            .collect::<Vec<_>>();
        let tree = Tree::build_limited_with_hints(
            &self.brushes,
            &structural,
            &mut self.planes,
            &self.hints,
            control,
            TREE_LIMITS,
        )?;
        let mut nodes = tree
            .nodes
            .iter()
            .map(|node| StationaryNode {
                split: node.split.map(|(index, children)| {
                    let plane = self.planes.get(index);
                    (
                        map_core::Plane {
                            normal: plane.normal.as_vec3().as_dvec3(),
                            dist: f64::from(plane.dist as f32),
                        },
                        children,
                    )
                }),
                solid: node.solid(),
                ..StationaryNode::default()
            })
            .collect::<Vec<_>>();
        let brushes = self
            .brushes
            .iter()
            .enumerate()
            .filter(|(_, brush)| brush.entity == 0 && brush.contents & collision_mask != 0)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let memberships = tree.brush_leaf_memberships_limited(
            &self.brushes,
            &brushes,
            &self.planes,
            control,
            TREE_LIMITS,
        )?;
        let mut references = 0usize;
        for (brush, leaves) in memberships.into_iter().enumerate() {
            control.checkpoint()?;
            for leaf in leaves {
                references = references.saturating_add(1);
                if references > MAX_STATIONARY_REFERENCES {
                    return Err("Rope stationary BSP leaf references exceed their limit".into());
                }
                nodes[leaf].brushes.push(brush);
            }
        }
        let mut work = 0usize;
        for (patch_index, patch) in patches.iter().enumerate() {
            let mut pending = VecDeque::from([0]);
            while let Some(index) = pending.pop_front() {
                control.checkpoint()?;
                work = work.saturating_add(1);
                if work > MAX_MEMBERSHIP_WORK {
                    return Err("Rope terrain BSP membership exceeded its work limit".into());
                }
                if let Some((plane, children)) = nodes[index].split {
                    let include = children_for_bounds(plane, patch);
                    for (child, include) in children.into_iter().zip(include) {
                        if include {
                            pending.push_back(child);
                        }
                    }
                    if pending.len() > MAX_STATIONARY_TREE_NODES {
                        return Err("Rope terrain BSP membership exceeded its pending limit".into());
                    }
                } else {
                    references = references.saturating_add(1);
                    if references > MAX_STATIONARY_REFERENCES {
                        return Err("Rope stationary BSP leaf references exceed their limit".into());
                    }
                    nodes[index].patches.push(patch_index);
                }
            }
        }
        control.checkpoint()?;
        Ok(StationaryTopology { nodes })
    }
}
