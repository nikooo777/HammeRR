use super::*;
use glam::DVec2;

mod materialize;
mod mesh;

pub use materialize::{
    FreePartitionFace, FreePartitionFragment, FreePartitionPlan, FreePieceSurface,
    MAX_FREE_PARTITION_FRAGMENTS, materialize_free_partitions, plan_free_partition_materialization,
};

pub const MAX_FREE_PARTITION_VERTICES: usize = 64;
pub const MAX_FREE_PARTITION_EDGES: usize = 32;
pub const MAX_FREE_PARTITION_PIECES: usize = 120;
const ROUNDOFF_FACTOR: f64 = 256.0;

fn roundoff_tolerance(magnitude: f64) -> f64 {
    ROUNDOFF_FACTOR * f64::EPSILON * magnitude.max(1.0)
}

#[derive(Debug, Clone, PartialEq)]
pub struct FreePartitionCut {
    pub solid: u32,
    pub side: u32,
    pub corners: Vec<Vec<u32>>,
    pub vertices: Vec<DVec3>,
    pub edges: Vec<[u32; 2]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FreePartitionVertex {
    pub id: u32,
    pub position: DVec3,
    pub coordinates: [f64; 3],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreePartitionEdge {
    pub vertices: [u32; 2],
    pub authored: bool,
    pub boundary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreePartitionPiece {
    pub id: u32,
    pub vertices: [u32; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct FreePartition {
    pub solid: u32,
    pub side: u32,
    pub owner: ObjectId,
    pub plane: [DVec3; 3],
    pub corners: Vec<Vec<u32>>,
    pub vertices: Vec<FreePartitionVertex>,
    pub edges: Vec<FreePartitionEdge>,
    pub pieces: Vec<FreePartitionPiece>,
    pub endpoint_vertices: Vec<u32>,
    pub max_endpoint_correction: f64,
    pub numeric_tolerance: f64,
    pub cut: FreePartitionCut,
}

impl FreePartition {
    pub fn cut(&self) -> FreePartitionCut {
        self.cut.clone()
    }

    pub fn normal(&self) -> DVec3 {
        Plane::from_points(self.plane).unwrap().normal
    }
}

struct Frame {
    origin: DVec3,
    u: DVec3,
    v: DVec3,
    inverse: DMat3,
}

impl Frame {
    fn new(corners: &[DVec3], normal: DVec3) -> Result<Self, EditError> {
        let a = corners[1] - corners[0];
        let b = corners[2] - corners[0];
        let matrix = DMat3::from_cols(a, b, normal);
        if !matrix.determinant().is_finite() || matrix.determinant().abs() <= MIN_EDGE * MIN_EDGE {
            return Err(invalid(
                "Free-cut source corners do not define a stable face frame",
            ));
        }
        let u = a.normalize();
        Ok(Self {
            origin: corners[0],
            u,
            v: u.cross(normal),
            inverse: matrix.inverse(),
        })
    }

    fn project(&self, point: DVec3) -> DVec2 {
        let offset = point - self.origin;
        DVec2::new(offset.dot(self.u), offset.dot(self.v))
    }

    fn coordinates(&self, point: DVec3) -> [f64; 3] {
        let coordinates = self.inverse * (point - self.origin);
        [
            1.0 - coordinates.x - coordinates.y,
            coordinates.x,
            coordinates.y,
        ]
    }
}

fn source_corners(
    shape: &Topology,
    side: u32,
    captured: &[Vec<u32>],
) -> Result<(Vec<Vec<u32>>, Vec<DVec3>), EditError> {
    let face = shape
        .faces
        .iter()
        .find(|face| face.side == side)
        .ok_or_else(|| invalid("Choose an existing source face for free cuts"))?;
    if !(3..=MAX_BOUNDARY_SOURCE_CORNERS).contains(&face.vertices.len()) {
        return Err(invalid(
            "Free cuts require a convex source face with 3–8 corners",
        ));
    }
    let mut ring = face
        .vertices
        .iter()
        .map(|index| shape.vertices[*index].sides.clone())
        .collect::<Vec<_>>();
    if captured.is_empty() {
        let start = ring
            .iter()
            .enumerate()
            .min_by_key(|(_, value)| *value)
            .unwrap()
            .0;
        ring.rotate_left(start);
    } else {
        if captured.len() != ring.len()
            || captured.iter().collect::<BTreeSet<_>>() != ring.iter().collect()
        {
            return Err(invalid("Free-cut source corner incidence changed"));
        }
        let indices = captured
            .iter()
            .map(|corner| {
                ring.iter()
                    .position(|candidate| candidate == corner)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let count = ring.len();
        if ![1, count - 1].iter().any(|step| {
            (0..count).all(|index| indices[(index + 1) % count] == (indices[index] + step) % count)
        }) {
            return Err(invalid("Free-cut source corner ring changed"));
        }
        ring = captured.to_vec();
    }
    let positions = ring
        .iter()
        .map(|corner| {
            shape
                .vertices
                .iter()
                .find(|vertex| &vertex.sides == corner)
                .unwrap()
                .position
        })
        .collect();
    Ok((ring, positions))
}

pub fn plan_free_partitions(
    document: &Document,
    requests: &[FreePartitionCut],
) -> Result<Vec<FreePartition>, EditError> {
    if requests.is_empty() || requests.len() > MAX_SOLIDS {
        return Err(invalid(
            "Draw free cuts on one face in each of 1–16 brushes",
        ));
    }
    let mut sorted = BTreeMap::new();
    for request in requests {
        if sorted.insert(request.solid, request).is_some() {
            return Err(invalid("Draw free cuts on exactly one face per brush"));
        }
    }
    let mut partitions = Vec::with_capacity(requests.len());
    for (id, request) in sorted {
        let (owner, source) = solid(document, id)?;
        if source.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(invalid(format!(
                "Brush {id}: free cuts on displacements are unsupported"
            )));
        }
        let shape = topology(source)?;
        let (corners, positions) = source_corners(&shape, request.side, &request.corners)?;
        let plane = source
            .sides
            .iter()
            .find(|side| side.id == request.side)
            .unwrap()
            .plane
            .0;
        let normal = Plane::from_points(plane).unwrap().normal;
        let frame = Frame::new(&positions, normal)?;
        partitions.push(mesh::build(
            request, owner, plane, corners, positions, &frame,
        )?);
    }
    Ok(partitions)
}

pub(crate) fn checked_free_partition(
    document: &Document,
    part: &FreePartition,
) -> Result<FreePartition, EditError> {
    let canonical = plan_free_partitions(document, &[part.cut()])?.remove(0);
    if &canonical != part {
        return Err(invalid(
            "Free-cut source geometry, topology or ownership changed",
        ));
    }
    Ok(canonical)
}
