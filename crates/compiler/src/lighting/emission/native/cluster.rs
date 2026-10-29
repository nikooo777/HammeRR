use glam::DVec3;
use source_formats::bsp::{
    Bsp, Leaf, LightCube, MAX_POINT_LEAF_RECORDS, MAX_VISIBILITY_BYTES, Model, Node, Plane, Record,
    contents, lump,
};

use super::super::{ARITHMETIC_OPERATIONS, NativePatch};
use crate::lighting::bake::{Allocation, Budget};

const NODE_WORK: usize = 256;
const PATCH_WORK: usize = 256;

pub(super) struct Clusters {
    world: Model,
    planes: Vec<Plane>,
    nodes: Vec<Node>,
    leaves: Vec<Leaf>,
    clusters: usize,
    pending: Vec<i32>,
    _allocation: Allocation,
}

impl Clusters {
    pub(super) fn new(bsp: &Bsp, budget: &mut Budget<'_>) -> Result<Self, String> {
        let leaf_stride = match bsp.lumps[lump::LEAFS].version {
            0 => size_of::<Leaf>() + size_of::<LightCube>(),
            Leaf::VERSION => size_of::<Leaf>(),
            _ => return Err("Native emission has an unsupported BSP leaf version".into()),
        };
        let tables = [
            (Plane::LUMP, Plane::VERSION, size_of::<Plane>()),
            (Node::LUMP, Node::VERSION, size_of::<Node>()),
            (Model::LUMP, Model::VERSION, size_of::<Model>()),
            (Leaf::LUMP, bsp.lumps[lump::LEAFS].version, leaf_stride),
        ];
        let mut counts = [0; 4];
        for (index, (slot, version, stride)) in tables.into_iter().enumerate() {
            budget.consume(1)?;
            let data = &bsp.lumps[slot];
            if data.version != version
                || !data.data.len().is_multiple_of(stride)
                || data.data.len() / stride > MAX_POINT_LEAF_RECORDS
            {
                return Err("Native emission BSP source-cluster tables exceed their native layout or record limit".into());
            }
            counts[index] = data.data.len() / stride;
        }
        let [planes, nodes, models, leaves] = counts;
        let bytes = planes * size_of::<Plane>()
            + nodes * size_of::<Node>()
            + leaves * size_of::<Leaf>()
            + (nodes + 1) * size_of::<i32>()
            + size_of::<Self>();
        let allocation = budget.reserve_bytes(bytes)?;
        let _decoded = budget.reserve_bytes(
            models * size_of::<Model>()
                + if bsp.lumps[lump::LEAFS].version == 0 {
                    leaves * size_of::<Leaf>()
                } else {
                    0
                },
        )?;
        let visibility = &bsp.lumps[lump::VISIBILITY].data;
        if visibility.len() > MAX_VISIBILITY_BYTES {
            return Err("Native emission visibility exceeds its native byte limit".into());
        }
        budget.consume(bytes.div_ceil(64) + visibility.len().div_ceil(64))?;
        let clusters = bsp
            .visibility()
            .map_err(|error| error.to_string())?
            .filter(|visibility| visibility.clusters() != 0)
            .ok_or("Native emission requires actual BSP visibility clusters")?
            .clusters();
        let world = {
            let models = bsp.records::<Model>().map_err(|error| error.to_string())?;
            *models
                .first()
                .ok_or("Native emission is missing its world BSP model")?
        };
        let mut result = Self {
            world,
            planes: bsp.records::<Plane>().map_err(|error| error.to_string())?,
            nodes: bsp.records::<Node>().map_err(|error| error.to_string())?,
            leaves: bsp.records::<Leaf>().map_err(|error| error.to_string())?,
            clusters,
            pending: Vec::with_capacity(nodes + 1),
            _allocation: allocation,
        };
        result.validate(budget)?;
        Ok(result)
    }

    pub(super) fn world(&self) -> &Model {
        &self.world
    }

    fn reference(&self, reference: i32) -> Result<(), String> {
        let valid = if reference >= 0 {
            (reference as usize) < self.nodes.len()
        } else {
            (-1_i64 - i64::from(reference)) < self.leaves.len() as i64
        };
        if valid {
            Ok(())
        } else {
            Err("Native emission world tree contains an invalid node or leaf reference".into())
        }
    }

    fn validate(&mut self, budget: &mut Budget<'_>) -> Result<(), String> {
        for plane in &self.planes {
            budget.consume(16)?;
            if !plane.dist.is_finite()
                || plane.normal.iter().any(|value| !value.is_finite())
                || plane.normal.iter().all(|&value| value == 0.0)
            {
                return Err("Native emission BSP plane must be finite and nonzero".into());
            }
        }
        for node in &self.nodes {
            budget.consume(8)?;
            if node.plane < 0 || node.plane as usize >= self.planes.len() {
                return Err("Native emission world tree has an invalid plane reference".into());
            }
            for child in node.children {
                self.reference(child)?;
            }
        }
        for leaf in &self.leaves {
            budget.consume(8)?;
            if leaf.cluster < -1
                || (leaf.cluster >= 0
                    && (leaf.cluster as usize >= self.clusters
                        || leaf.contents & contents::SOLID != 0))
            {
                return Err("Native emission BSP leaf has an invalid visibility cluster".into());
            }
        }
        self.reference(self.world.head_node)?;
        let _seen = budget.reserve::<bool>(self.nodes.len())?;
        budget.consume(self.nodes.len())?;
        let mut seen = vec![false; self.nodes.len()];
        self.pending.push(self.world.head_node);
        while let Some(reference) = self.pending.pop() {
            budget.consume(8)?;
            if reference < 0 {
                continue;
            }
            let index = reference as usize;
            if std::mem::replace(&mut seen[index], true) {
                return Err("Native emission world tree has a cycle or repeated node".into());
            }
            self.pending.extend(self.nodes[index].children);
        }
        Ok(())
    }

    pub(super) fn cluster(
        &mut self,
        patch: &NativePatch<'_>,
        origin: [f32; 3],
        normal: DVec3,
        budget: &mut Budget<'_>,
    ) -> Result<i32, String> {
        budget.consume(PATCH_WORK)?;
        let point = DVec3::from_array(origin.map(f64::from));
        let direction = normal.as_vec3().as_dvec3();
        let equation = patch.boundary.equation;
        let centroid = patch.points.iter().copied().sum::<DVec3>() / 3.0;
        let outward = direction.dot(equation.normal);
        if !point.is_finite()
            || !normal.is_finite()
            || !direction.is_finite()
            || direction.length_squared() == 0.0
            || !equation.normal.is_finite()
            || !equation.dist.is_finite()
            || !outward.is_finite()
            || outward <= 0.0
            || patch.points.iter().any(|point| !point.is_finite())
            || origin != centroid.as_vec3().to_array()
        {
            return Err(
                "Source cluster needs finite native geometry and an outward direction".into(),
            );
        }
        let support = if let Some(index) = patch.world_plane {
            if index != patch.boundary.plane {
                return Err("Source cluster lost its original world plane identity".into());
            }
            let plane = self
                .planes
                .get(index)
                .ok_or("Source cluster is missing its stored world plane")?;
            if plane.normal != equation.normal.as_vec3().to_array()
                || plane.dist != equation.dist as f32
            {
                return Err("Source cluster world plane does not match its final equation".into());
            }
            let normal = DVec3::from_array(plane.normal.map(f64::from));
            let distance = f64::from(plane.dist);
            let residual = patch
                .points
                .iter()
                .map(|point| (normal.dot(*point) - distance).abs())
                .fold(0.0_f64, f64::max);
            let scale = patch
                .points
                .iter()
                .map(|point| (normal * *point).abs().element_sum())
                .fold(0.0_f64, f64::max)
                + distance.abs()
                + 1.0;
            let bound = residual
                + normal.abs().dot((point - centroid).abs())
                + ARITHMETIC_OPERATIONS * f64::EPSILON * scale;
            if !bound.is_finite() {
                return Err("Source cluster support envelope is nonfinite".into());
            }
            Some((index & !1, bound))
        } else {
            None
        };
        self.pending.clear();
        self.pending.push(self.world.head_node);
        while let Some(reference) = self.pending.pop() {
            budget.consume(NODE_WORK)?;
            if reference < 0 {
                let leaf = self.leaves[(-1_i64 - i64::from(reference)) as usize];
                if leaf.contents & contents::SOLID == 0 && leaf.cluster >= 0 {
                    return Ok(i32::from(leaf.cluster));
                }
                continue;
            }
            let node = self.nodes[reference as usize];
            let plane = self.planes[node.plane as usize];
            let normal = DVec3::from_array(plane.normal.map(f64::from));
            let distance = normal.dot(point) - f64::from(plane.dist);
            let source_boundary = support
                .filter(|(index, _)| *index == (node.plane as usize & !1))
                .map(|(_, bound)| bound);
            let side = if let Some(bound) = source_boundary {
                if distance.abs() > bound {
                    return Err(
                        "Source centroid is outside its measured world-plane support".into(),
                    );
                }
                0
            } else {
                sign([
                    normal.x * point.x,
                    normal.y * point.y,
                    normal.z * point.z,
                    -f64::from(plane.dist),
                ])
            };
            let side = if side == 0 {
                sign([
                    normal.x * direction.x,
                    normal.y * direction.y,
                    normal.z * direction.z,
                    0.0,
                ])
            } else {
                side
            };
            match side {
                -1 => self.pending.push(node.children[1]),
                1 => self.pending.push(node.children[0]),
                _ => {
                    self.pending.push(node.children[1]);
                    self.pending.push(node.children[0]);
                }
            }
        }
        Err("Source has no outward-adjacent nonsolid visibility cluster".into())
    }
}

fn sign(terms: [f64; 4]) -> i8 {
    // The stored f32 products are exact in f64; preserve their cancellation and zero ties.
    let mut expansion = [0.0; 4];
    let mut count = 0;
    for mut value in terms {
        let mut written = 0;
        for index in 0..count {
            let component = expansion[index];
            let sum = value + component;
            let virtual_component = sum - value;
            let error = (value - (sum - virtual_component)) + (component - virtual_component);
            if error != 0.0 {
                expansion[written] = error;
                written += 1;
            }
            value = sum;
        }
        expansion[written] = value;
        count = written + 1;
    }
    let value = expansion[..count]
        .iter()
        .rev()
        .copied()
        .find(|&value| value != 0.0)
        .unwrap_or(0.0);
    if value == 0.0 {
        0
    } else if value < 0.0 {
        -1
    } else {
        1
    }
}
