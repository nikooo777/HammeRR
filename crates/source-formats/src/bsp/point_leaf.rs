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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Bsp {
        let mut bsp = Bsp::new();
        bsp.set_records(&[
            Model {
                head_node: 0,
                ..Model::default()
            },
            Model {
                head_node: 1,
                ..Model::default()
            },
        ]);
        bsp.set_records(&[Plane {
            normal: [1.0, 0.0, 0.0],
            dist: 1.0,
            kind: 0,
        }]);
        bsp.set_records(&[
            Node {
                plane: 0,
                children: [-2, -1],
                ..Node::default()
            },
            Node {
                plane: 0,
                children: [-3, -3],
                ..Node::default()
            },
        ]);
        bsp.set_records(&[
            Leaf {
                cluster: -1,
                ..Leaf::default()
            },
            Leaf {
                cluster: 0,
                ..Leaf::default()
            },
            Leaf {
                cluster: -1,
                ..Leaf::default()
            },
        ]);
        bsp
    }

    #[test]
    fn world_lookup_uses_model_zero_and_declared_float_ties_in_both_leaf_versions() {
        let mut bsp = fixture();
        for legacy in [false, true] {
            if legacy {
                let leaves = bsp.records::<Leaf>().unwrap();
                bsp.lumps[lump::LEAFS].version = 0;
                bsp.lumps[lump::LEAFS].data = leaves
                    .iter()
                    .flat_map(|leaf| {
                        let mut bytes = bytemuck::bytes_of(leaf).to_vec();
                        bytes.resize(LEAF_V0_SIZE, 0);
                        bytes
                    })
                    .collect();
            }
            for (x, ordinal) in [
                (1.0, 1),
                (1.0 - f64::from(f32::EPSILON), 0),
                (1.0 + f64::from(f32::EPSILON) * 0.25, 1),
                (-1e10, 0),
            ] {
                let hit = bsp
                    .world_leaf_at_controlled([x, 0.0, 0.0], &mut || Ok(()))
                    .unwrap();
                assert_eq!(hit.ordinal, ordinal);
                assert_eq!(hit.effective_position[0], x as f32);
                assert_eq!(hit.nodes_visited, 1);
            }
        }
    }

    #[test]
    fn world_lookup_refuses_bad_routes_arithmetic_and_cancellation() {
        let mut bsp = fixture();
        for child in [0, i32::MAX, i32::MIN] {
            bsp.set_records(&[Node {
                plane: 0,
                children: [child, child],
                ..Node::default()
            }]);
            assert!(
                bsp.world_leaf_at_controlled([2.0, 0.0, 0.0], &mut || Ok(()))
                    .is_err()
            );
        }
        bsp = fixture();
        for plane in [-1, i32::MAX] {
            bsp.set_records(&[Node {
                plane,
                children: [-2, -1],
                ..Node::default()
            }]);
            assert!(
                bsp.world_leaf_at_controlled([2.0, 0.0, 0.0], &mut || Ok(()))
                    .is_err()
            );
        }
        bsp = fixture();
        for plane in [
            Plane {
                normal: [0.0; 3],
                ..Plane::default()
            },
            Plane {
                normal: [f32::NAN, 0.0, 0.0],
                ..Plane::default()
            },
            Plane {
                normal: [1.0, 1.0, -1.0],
                ..Plane::default()
            },
        ] {
            bsp.set_records(&[plane]);
            assert!(
                bsp.world_leaf_at_controlled([f64::from(f32::MAX); 3], &mut || Ok(()))
                    .is_err()
            );
        }
        bsp = fixture();
        assert!(
            bsp.world_leaf_at_controlled([f64::MAX, 0.0, 0.0], &mut || Ok(()))
                .is_err()
        );
        assert!(
            bsp.world_leaf_at_controlled([f64::NAN, 0.0, 0.0], &mut || Ok(()))
                .is_err()
        );
        let mut checks = 0;
        let result = bsp.world_leaf_at_controlled([2.0, 0.0, 0.0], &mut || {
            checks += 1;
            if checks == 3 {
                Err(io::Error::other("cancelled"))
            } else {
                Ok(())
            }
        });
        assert_eq!(result.unwrap_err().to_string(), "cancelled");
        bsp.set_records(&[Model {
            head_node: -1,
            ..Model::default()
        }]);
        assert_eq!(
            bsp.world_leaf_at_controlled([0.0; 3], &mut || Ok(()))
                .unwrap()
                .ordinal,
            0
        );
    }
}
