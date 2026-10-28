use std::io;

use bytemuck::Pod;

use super::{
    Bsp, LEAF_V0_SIZE, Leaf, Model, Node, Plane, Record, areas::bounded_data, invalid, lump,
};

pub const MAX_POINT_LEAF_RECORDS: usize = 65_536;
pub const POINT_LEAF_CLASSIFICATION: &str = "The requested point narrows to f32. Stored f32 plane coefficients use separate scalar f32 operations in this order: ((nx*x + ny*y) + nz*z) - dist. Every product, partial sum and final distance must remain finite. Negative distance selects child1; zero, including signed zero, selects child0. No epsilon, fused operation or engine bit-parity claim.";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointLeaf {
    pub ordinal: usize,
    pub effective_position: [f32; 3],
    pub nodes_visited: usize,
}

fn record<T: Pod>(bytes: &[u8], index: usize) -> io::Result<T> {
    let start = index
        .checked_mul(size_of::<T>())
        .ok_or_else(|| invalid("Point-leaf record offset overflow"))?;
    let end = start
        .checked_add(size_of::<T>())
        .ok_or_else(|| invalid("Point-leaf record range overflow"))?;
    bytes
        .get(start..end)
        .map(bytemuck::pod_read_unaligned)
        .ok_or_else(|| invalid("Point-leaf record index is outside its native table"))
}

impl Bsp {
    pub fn world_leaf_at_controlled(
        &self,
        position: [f64; 3],
        checkpoint: &mut impl FnMut() -> io::Result<()>,
    ) -> io::Result<PointLeaf> {
        checkpoint()?;
        let effective_position = position.map(|value| value as f32);
        if position.iter().any(|value| !value.is_finite())
            || effective_position.iter().any(|value| !value.is_finite())
        {
            return Err(invalid(
                "World position must narrow to finite native f32 coordinates",
            ));
        }
        let models =
            bounded_data::<Model>(self, Model::LUMP, MAX_POINT_LEAF_RECORDS, Model::VERSION)?;
        let nodes = bounded_data::<Node>(self, Node::LUMP, MAX_POINT_LEAF_RECORDS, Node::VERSION)?;
        let planes =
            bounded_data::<Plane>(self, Plane::LUMP, MAX_POINT_LEAF_RECORDS, Plane::VERSION)?;
        let stride = match self.lumps[lump::LEAFS].version {
            0 => LEAF_V0_SIZE,
            Leaf::VERSION => size_of::<Leaf>(),
            _ => return Err(invalid("Unsupported point-leaf leaf version")),
        };
        bounded_data::<u8>(
            self,
            Leaf::LUMP,
            MAX_POINT_LEAF_RECORDS * stride,
            self.lumps[Leaf::LUMP].version,
        )?;
        let leaves = self.checked(Leaf::LUMP, stride)?;
        let mut reference = record::<Model>(models, 0)?.head_node;
        let count = nodes.len() / size_of::<Node>();
        for nodes_visited in 0..=count {
            checkpoint()?;
            if reference < 0 {
                let ordinal = usize::try_from(-1_i64 - i64::from(reference))
                    .map_err(|_| invalid("World leaf ordinal overflow"))?;
                if ordinal >= leaves.len() / stride {
                    return Err(invalid("World leaf reference is outside its native table"));
                }
                return Ok(PointLeaf {
                    ordinal,
                    effective_position,
                    nodes_visited,
                });
            }
            if nodes_visited == count {
                return Err(invalid(
                    "World point-leaf traversal has a cycle or exceeds its native node count",
                ));
            }
            let node = record::<Node>(nodes, reference as usize)?;
            let plane_index = usize::try_from(node.plane)
                .map_err(|_| invalid("World point-leaf plane index is negative"))?;
            let plane = record::<Plane>(planes, plane_index)?;
            if plane.normal.iter().any(|value| !value.is_finite())
                || !plane.dist.is_finite()
                || plane.normal.iter().all(|&value| value == 0.0)
            {
                return Err(invalid("World point-leaf plane must be finite and nonzero"));
            }
            let mut distance = 0.0_f32;
            for (axis, (normal, coordinate)) in
                plane.normal.into_iter().zip(effective_position).enumerate()
            {
                let product = normal * coordinate;
                if !product.is_finite() {
                    return Err(invalid("World point-leaf multiplication overflow"));
                }
                distance = if axis == 0 {
                    product
                } else {
                    distance + product
                };
                if !distance.is_finite() {
                    return Err(invalid("World point-leaf sum overflow"));
                }
            }
            distance -= plane.dist;
            if !distance.is_finite() {
                return Err(invalid("World point-leaf distance overflow"));
            }
            reference = node.children[usize::from(distance < 0.0)];
        }
        Err(invalid("World point-leaf traversal did not terminate"))
    }
}
