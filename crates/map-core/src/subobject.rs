use std::collections::{BTreeMap, BTreeSet};

use glam::{DMat3, DVec3};
use source_formats::vmf::Solid;

use crate::{BrushGeometry, Document, EditError, ObjectId, Plane, Transaction};

mod transform;
pub use transform::{ComponentTransform, TransformPlan, component_center, plan_transform};

mod weld;
pub use weld::{
    WeldChange, WeldPlan, WeldTarget, plan_weld, plan_weld_target, plan_weld_with_textures,
};
mod split;
pub use split::{
    MAX_RISE as MAX_SPLIT_RISE, MIN_RISE as MIN_SPLIT_RISE, SplitChange, SplitFace, SplitPlan,
    split, split_with_textures,
};
mod crease;
pub use crease::{
    Crease, CreaseChange, CreasePair, CreasePlan, MAX_BOUNDARY_SOURCE_CORNERS, crease,
    crease_pairs, crease_with_roof_textures,
};
mod partition;
pub(crate) use partition::checked_partition;
pub use partition::{
    MAX_PARTITION_LOOP_VERTICES, MAX_PARTITION_VERTICES, Partition, PartitionCut, PartitionLoop,
    PartitionVertex, VertexSource, partition_pairs, plan_partitions, plan_partitions_at,
    polygon_partition_pairs,
};
mod extrusion;
pub use extrusion::{
    ExtrusionFace, ExtrusionFragment, ExtrusionPiece, ExtrusionPlan, ExtrusionSpec,
    ExtrusionSurface, MAX_EXTRUSION_FRAGMENTS, MAX_EXTRUSION_PIECES, MAX_EXTRUSION_PLANES,
    MAX_EXTRUSION_SOURCE_VERTICES, extrude_partitions, extrude_partitions_with_materials,
    extrude_partitions_with_surfaces, plan_partition_extrusion,
    plan_partition_extrusion_with_materials, plan_partition_extrusion_with_surfaces,
};
mod partition_materials;
pub use partition_materials::{PartitionMaterial, crease_partitions_with_materials};
mod partition_textures;
pub use partition_textures::{PartitionTexture, PieceTexture, crease_partitions_with_surfaces};

pub const MAX_SIDES: usize = 64;
pub const MAX_SOLIDS: usize = 16;
pub const MAX_COMPONENTS: usize = 512;
const EPSILON: f64 = 0.000_01;
const MIN_EDGE: f64 = 0.002;
const MAX_COORDINATE: f64 = 65_536.0;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Component {
    pub solid: u32,
    pub sides: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct Vertex {
    pub sides: Vec<u32>,
    pub position: DVec3,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub sides: Vec<u32>,
    pub vertices: [usize; 2],
}

#[derive(Debug, Clone)]
pub struct Face {
    pub side: u32,
    pub vertices: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct Topology {
    pub solid: u32,
    pub vertices: Vec<Vertex>,
    pub edges: Vec<Edge>,
    pub faces: Vec<Face>,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

pub fn solid(document: &Document, id: u32) -> Result<(ObjectId, &Solid), EditError> {
    let mut matches = document.vmf().solids().filter(|solid| solid.id == id);
    let solid = matches
        .next()
        .ok_or(EditError::NotFound(ObjectId::Solid(id)))?;
    if matches.next().is_some() {
        return Err(invalid(format!("Brush {id} has an ambiguous duplicate ID")));
    }
    let owner = document.owner_of_solid(id).unwrap();
    if let ObjectId::Entity(id) = owner
        && document
            .vmf()
            .entities
            .iter()
            .filter(|entity| entity.id == id)
            .count()
            != 1
    {
        return Err(invalid(format!(
            "Entity {id} has an ambiguous duplicate ID"
        )));
    }
    Ok((owner, solid))
}

pub fn topology(solid: &Solid) -> Result<Topology, EditError> {
    let fail = |message: &str| invalid(format!("Brush {}: {message}", solid.id));
    if !(4..=MAX_SIDES).contains(&solid.sides.len()) {
        return Err(fail("component editing requires 4–64 sides"));
    }
    let ids = solid
        .sides
        .iter()
        .map(|side| side.id)
        .collect::<BTreeSet<_>>();
    if ids.len() != solid.sides.len() {
        return Err(fail("duplicate face IDs make components ambiguous"));
    }
    if solid
        .sides
        .iter()
        .any(|side| side.plane.0.iter().any(|point| !point.is_finite()))
    {
        return Err(fail("plane points must be finite"));
    }
    let geometry = BrushGeometry::from_solid(solid).map_err(|error| fail(&error.to_string()))?;
    if geometry.faces.len() != solid.sides.len() {
        return Err(fail(
            "redundant or missing faces; repair the brush before editing components",
        ));
    }
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut faces = Vec::new();
    let mut adjacency: BTreeMap<[usize; 2], Vec<u32>> = BTreeMap::new();
    for face in &geometry.faces {
        let side = solid.sides[face.side].id;
        let mut ring = Vec::new();
        for point in &face.vertices {
            if !point.is_finite() || point.abs().max_element() > MAX_COORDINATE {
                return Err(fail("geometry is unbounded or outside ±65536 units"));
            }
            let index = vertices
                .iter()
                .position(|vertex| vertex.position.distance(*point) < EPSILON)
                .unwrap_or_else(|| {
                    vertices.push(Vertex {
                        sides: Vec::new(),
                        position: *point,
                    });
                    vertices.len() - 1
                });
            vertices[index].sides.push(side);
            ring.push(index);
        }
        for index in 0..ring.len() {
            let mut pair = [ring[index], ring[(index + 1) % ring.len()]];
            if vertices[pair[0]]
                .position
                .distance(vertices[pair[1]].position)
                <= MIN_EDGE
            {
                return Err(fail("an edge is collapsed or shorter than 0.002 units"));
            }
            pair.sort_unstable();
            adjacency.entry(pair).or_default().push(side);
        }
        faces.push(Face {
            side,
            vertices: ring,
        });
    }
    let mut keys = BTreeSet::new();
    for vertex in &mut vertices {
        vertex.sides.sort_unstable();
        vertex.sides.dedup();
        if vertex.sides.len() < 3 || !keys.insert(vertex.sides.clone()) {
            return Err(fail("open or ambiguous vertex topology"));
        }
    }
    let mut edges = Vec::new();
    let mut edge_keys = BTreeSet::new();
    for (vertices, mut sides) in adjacency {
        sides.sort_unstable();
        sides.dedup();
        if sides.len() != 2 || !edge_keys.insert(sides.clone()) {
            return Err(fail("open or ambiguous edge topology"));
        }
        edges.push(Edge { sides, vertices });
    }
    if vertices.len() + faces.len() != edges.len() + 2 {
        return Err(fail("brush is not a closed convex polyhedron"));
    }
    let center = geometry.center();
    let volume: f64 = geometry
        .faces
        .iter()
        .map(|face| {
            let a = face.vertices[0] - center;
            face.vertices[1..]
                .windows(2)
                .map(|pair| a.dot((pair[1] - center).cross(pair[0] - center)) / 6.0)
                .sum::<f64>()
        })
        .sum();
    if !volume.is_finite() || volume <= 1e-6 {
        return Err(fail("brush has no positive enclosed volume"));
    }
    edges.sort_by(|a, b| a.sides.cmp(&b.sides));
    faces.sort_by_key(|face| face.side);
    Ok(Topology {
        solid: solid.id,
        vertices,
        edges,
        faces,
    })
}

impl Topology {
    pub fn resolve(&self, sides: &[u32]) -> Result<Vec<usize>, EditError> {
        let mut sides = sides.to_vec();
        sides.sort_unstable();
        if sides.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(invalid("Component face IDs must be unique"));
        }
        let result = if sides.len() == 1 {
            self.faces
                .iter()
                .find(|face| face.side == sides[0])
                .map(|face| face.vertices.clone())
        } else if sides.len() == 2 {
            self.edges
                .iter()
                .find(|edge| edge.sides == sides)
                .map(|edge| edge.vertices.to_vec())
        } else {
            self.vertices
                .iter()
                .position(|vertex| vertex.sides == sides)
                .map(|index| vec![index])
        };
        result.ok_or_else(|| {
            invalid(format!(
                "Brush {} has no face, vertex or edge incident to faces {sides:?}",
                self.solid
            ))
        })
    }
}

#[derive(Clone, PartialEq)]
pub struct Replacement {
    owner: ObjectId,
    solid: Solid,
}

pub fn plan_move(
    document: &Document,
    components: &[Component],
    delta: DVec3,
) -> Result<Vec<Replacement>, EditError> {
    plan_move_with_texture_lock(document, components, delta, false)
}

pub fn plan_move_with_texture_lock(
    document: &Document,
    components: &[Component],
    delta: DVec3,
    texture_lock: bool,
) -> Result<Vec<Replacement>, EditError> {
    if !delta.is_finite() || delta.abs().max_element() > MAX_COORDINATE || delta == DVec3::ZERO {
        return Err(invalid(
            "Move needs a finite nonzero offset within ±65536 units",
        ));
    }
    plan_affine(document, components, DMat3::IDENTITY, delta, texture_lock)
}

fn plan_affine(
    document: &Document,
    components: &[Component],
    linear: DMat3,
    offset: DVec3,
    texture_lock: bool,
) -> Result<Vec<Replacement>, EditError> {
    if components.is_empty() || components.len() > MAX_COMPONENTS {
        return Err(invalid("Select between 1 and 512 faces, vertices or edges"));
    }
    let mut grouped: BTreeMap<u32, Vec<&Component>> = BTreeMap::new();
    for component in components {
        grouped.entry(component.solid).or_default().push(component);
    }
    if grouped.len() > MAX_SOLIDS {
        return Err(invalid(
            "Component editing supports at most 16 brushes at a time",
        ));
    }
    let mut result = Vec::new();
    for (id, components) in grouped {
        let (owner, source) = solid(document, id)?;
        if source.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(invalid(format!(
                "Brush {id} has displacements; component transforms are unsupported"
            )));
        }
        let topology = topology(source)?;
        let mut selected = BTreeSet::new();
        for component in components {
            selected.extend(topology.resolve(&component.sides)?);
        }
        let positions = topology
            .vertices
            .iter()
            .enumerate()
            .map(|(index, vertex)| {
                if selected.contains(&index) {
                    linear * vertex.position + offset
                } else {
                    vertex.position
                }
            })
            .collect::<Vec<_>>();
        if positions
            .iter()
            .any(|point| !point.is_finite() || point.abs().max_element() > MAX_COORDINATE)
        {
            return Err(invalid(format!(
                "Brush {id}: transformed vertices exceed ±65536 units"
            )));
        }
        for edge in &topology.edges {
            if positions[edge.vertices[0]].distance(positions[edge.vertices[1]]) <= MIN_EDGE {
                return Err(invalid(format!(
                    "Brush {id}: transform collapses edge {:?}",
                    edge.sides
                )));
            }
        }
        let mut replacement = source.clone();
        for face in &topology.faces {
            let ring = face
                .vertices
                .iter()
                .map(|index| positions[*index])
                .collect::<Vec<_>>();
            let original = source
                .sides
                .iter()
                .find(|side| side.id == face.side)
                .unwrap();
            let moved = face
                .vertices
                .iter()
                .filter(|index| selected.contains(index))
                .count();
            let triple = if moved == 0 {
                original.plane.0
            } else if moved == face.vertices.len() {
                original.plane.0.map(|point| linear * point + offset)
            } else {
                let area = |points: &[DVec3; 3]| {
                    (points[0] - points[1])
                        .cross(points[2] - points[1])
                        .length_squared()
                };
                ring[1..]
                    .windows(2)
                    .map(|pair| [ring[0], pair[0], pair[1]])
                    .max_by(|a, b| area(a).total_cmp(&area(b)))
                    .filter(|points| area(points) > EPSILON * EPSILON)
                    .ok_or_else(|| {
                        invalid(format!(
                            "Brush {id}, face {}: transform collapses the face",
                            face.side
                        ))
                    })?
            };
            let plane = Plane::from_points(triple).ok_or_else(|| {
                invalid(format!(
                    "Brush {id}, face {}: transform collapses the plane",
                    face.side
                ))
            })?;
            if ring
                .iter()
                .any(|point| plane.distance_to(*point).abs() > EPSILON)
            {
                return Err(invalid(format!(
                    "Brush {id}, face {}: transform makes the face nonplanar; move the whole edge or face together",
                    face.side
                )));
            }
            if positions
                .iter()
                .any(|point| plane.distance_to(*point) > EPSILON)
            {
                return Err(invalid(format!(
                    "Brush {id}, face {}: transform makes the brush concave or inverted",
                    face.side
                )));
            }
            if face.vertices.iter().any(|index| selected.contains(index)) {
                let target = replacement
                    .sides
                    .iter_mut()
                    .find(|side| side.id == face.side)
                    .unwrap();
                if texture_lock {
                    let before = face
                        .vertices
                        .iter()
                        .map(|index| topology.vertices[*index].position)
                        .collect::<Vec<_>>();
                    let projection = crate::texture_deform::attach(target, &before, &ring)
                        .map_err(|error| {
                            invalid(format!("Brush {id}, face {}: {error}", face.side))
                        })?;
                    target.u_axis = projection.u;
                    target.v_axis = projection.v;
                }
                target.plane.0 = triple;
            }
        }
        let rebuilt = self::topology(&replacement)?;
        if rebuilt.vertices.len() != topology.vertices.len()
            || rebuilt.edges.len() != topology.edges.len()
        {
            return Err(invalid(format!(
                "Brush {id}: transform changes topology; use split or weld explicitly"
            )));
        }
        for (index, vertex) in topology.vertices.iter().enumerate() {
            if !rebuilt.vertices.iter().any(|after| {
                after.sides == vertex.sides
                    && after.position.distance(positions[index]) < EPSILON * 10.0
            }) {
                return Err(invalid(format!(
                    "Brush {id}: transform changes vertex identity or enclosed geometry"
                )));
            }
        }
        result.push(Replacement {
            owner,
            solid: replacement,
        });
    }
    Ok(result)
}

pub fn apply(tx: &mut Transaction, replacements: &[Replacement]) -> Result<usize, EditError> {
    for replacement in replacements {
        let target = match replacement.owner {
            ObjectId::Solid(id) => tx.solid_mut(id)?,
            ObjectId::Entity(id) => tx
                .entity_mut(id)?
                .solids
                .iter_mut()
                .find(|solid| solid.id == replacement.solid.id)
                .ok_or(EditError::NotFound(ObjectId::Solid(replacement.solid.id)))?,
        };
        *target = replacement.solid.clone();
    }
    Ok(replacements.len())
}
