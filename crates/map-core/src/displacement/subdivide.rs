use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use source_formats::vmf::{DispInfo, Vmf};

use super::{DisplacementMesh, edit, neighbors};
use crate::{
    Document, EditError,
    brush::Face,
    primitive::MAX_COORDINATE,
    surface::{self, FaceId},
};

mod budget;
mod cage;

pub use budget::{
    MAX_FACES, MAX_FINAL_QUADS, MAX_SAMPLE_SLOTS, MAX_TOPOLOGY_WORK, MAX_VERTICES,
    REFINEMENT_LEVELS, SourceWork, StageWork, Work,
};

pub struct Plan {
    pub edits: Vec<(FaceId, DispInfo)>,
    pub shared_edges: usize,
    pub vertices: usize,
    pub work: Work,
}

#[derive(Clone, Copy)]
struct Quad {
    vertices: [usize; 4],
    source: usize,
    row: usize,
    column: usize,
}

struct Mesh {
    points: Vec<DVec3>,
    faces: Vec<Quad>,
}

struct Edge {
    ends: [usize; 2],
    faces: Vec<usize>,
}

struct Topology {
    edges: Vec<Edge>,
    face_edges: Vec<[usize; 4]>,
    vertex_edges: Vec<Vec<usize>>,
    vertex_faces: Vec<Vec<usize>>,
}

fn invalid(message: &str) -> EditError {
    EditError::Invalid(message.into())
}

impl Topology {
    fn build(mesh: &Mesh, stage: &StageWork, source_count: usize) -> Result<Self, EditError> {
        budget::check_mesh(mesh.faces.len(), mesh.points.len(), stage)?;
        let mut work = mesh
            .faces
            .len()
            .checked_mul(budget::FIXED_TOPOLOGY_WORK)
            .and_then(|work| work.checked_add(mesh.points.len()))
            .ok_or_else(|| invalid("Subdivision topology work overflow"))?;
        let mut topology = Self {
            edges: Vec::new(),
            face_edges: Vec::with_capacity(mesh.faces.len()),
            vertex_edges: vec![Vec::new(); mesh.points.len()],
            vertex_faces: vec![Vec::new(); mesh.points.len()],
        };
        let mut lookup = BTreeMap::new();
        let mut unique = BTreeSet::new();
        for (face, quad) in mesh.faces.iter().enumerate() {
            let mut sorted = quad.vertices;
            sorted.sort_unstable();
            if sorted.windows(2).any(|pair| pair[0] == pair[1]) || !unique.insert(sorted) {
                return Err(invalid(
                    "Subdivision requires distinct non-overlapping quad cages",
                ));
            }
            for &vertex in &quad.vertices {
                topology.vertex_faces[vertex].push(face);
            }
            let mut face_edges = [0; 4];
            for (side, slot) in face_edges.iter_mut().enumerate() {
                let ends = [quad.vertices[side], quad.vertices[(side + 1) % 4]];
                let key = if ends[0] < ends[1] {
                    ends
                } else {
                    [ends[1], ends[0]]
                };
                let index = *lookup.entry(key).or_insert_with(|| {
                    let index = topology.edges.len();
                    topology.edges.push(Edge {
                        ends,
                        faces: Vec::new(),
                    });
                    for end in ends {
                        topology.vertex_edges[end].push(index);
                    }
                    index
                });
                let edge = &mut topology.edges[index];
                if edge.faces.len() == 2 || (!edge.faces.is_empty() && edge.ends == ends) {
                    return Err(invalid(
                        "Subdivision needs consistently oriented manifold edges",
                    ));
                }
                edge.faces.push(face);
                *slot = index;
            }
            topology.face_edges.push(face_edges);
        }
        for (vertex, faces) in topology.vertex_faces.iter().enumerate() {
            let edges = &topology.vertex_edges[vertex];
            if faces.len() > 4 * source_count || edges.len() > 2 * faces.len() {
                return Err(invalid(
                    "Subdivision exceeds its cage corner-incidence bound",
                ));
            }
            let boundaries = edges
                .iter()
                .filter(|&&edge| topology.edges[edge].faces.len() == 1)
                .count();
            if faces.is_empty() || (boundaries != 0 && boundaries != 2) {
                return Err(invalid("Subdivision needs manifold corner junctions"));
            }
            let mut connected = BTreeSet::from([faces[0]]);
            loop {
                let count = connected.len();
                for &edge in edges {
                    let adjacent = &topology.edges[edge].faces;
                    work = work
                        .checked_add(2 * adjacent.len())
                        .filter(|work| *work <= stage.topology_work_bound)
                        .ok_or_else(|| invalid("Subdivision exceeds its admitted topology work"))?;
                    if adjacent.iter().any(|face| connected.contains(face)) {
                        connected.extend(adjacent);
                    }
                }
                if count == connected.len() {
                    break;
                }
            }
            if connected.len() != faces.len() {
                return Err(invalid(
                    "Corner-only cage contacts need separate subdivision actions",
                ));
            }
        }
        Ok(topology)
    }
}

impl Mesh {
    fn refine(self, topology: &Topology, next: &StageWork) -> Result<Self, EditError> {
        let (point_capacity, face_capacity) = budget::refinement_capacity(
            self.points.len(),
            topology.edges.len(),
            self.faces.len(),
            next,
        )?;
        let centers = self
            .faces
            .iter()
            .map(|face| {
                face.vertices
                    .iter()
                    .map(|&id| self.points[id])
                    .sum::<DVec3>()
                    * 0.25
            })
            .collect::<Vec<_>>();
        let mut points = Vec::with_capacity(point_capacity);
        for (vertex, &point) in self.points.iter().enumerate() {
            let edges = &topology.vertex_edges[vertex];
            if edges
                .iter()
                .any(|&edge| topology.edges[edge].faces.len() == 1)
            {
                points.push(point);
            } else {
                let faces = &topology.vertex_faces[vertex];
                let n = edges.len() as f64;
                let f = faces.iter().map(|&face| centers[face]).sum::<DVec3>() / faces.len() as f64;
                let r = edges
                    .iter()
                    .map(|&edge| {
                        topology.edges[edge]
                            .ends
                            .iter()
                            .map(|&id| self.points[id])
                            .sum::<DVec3>()
                            * 0.5
                    })
                    .sum::<DVec3>()
                    / n;
                points.push((f + 2.0 * r + (n - 3.0) * point) / n);
            }
        }
        let edge_start = points.len();
        for edge in &topology.edges {
            let sum = edge.ends.iter().map(|&id| self.points[id]).sum::<DVec3>();
            points.push(if edge.faces.len() == 1 {
                sum * 0.5
            } else {
                (sum + centers[edge.faces[0]] + centers[edge.faces[1]]) * 0.25
            });
        }
        let center_start = points.len();
        points.extend(centers);
        let mut faces = Vec::with_capacity(face_capacity);
        for (index, face) in self.faces.iter().enumerate() {
            let [a, b, c, d] = face.vertices;
            let [ab, bc, cd, da] = topology.face_edges[index].map(|edge| edge_start + edge);
            let center = center_start + index;
            for (row, column, vertices) in [
                (0, 0, [a, ab, center, da]),
                (1, 0, [ab, b, bc, center]),
                (1, 1, [center, bc, c, cd]),
                (0, 1, [da, center, cd, d]),
            ] {
                let row = face
                    .row
                    .checked_mul(2)
                    .and_then(|value| value.checked_add(row))
                    .ok_or_else(|| invalid("Subdivision row count overflow"))?;
                let column = face
                    .column
                    .checked_mul(2)
                    .and_then(|value| value.checked_add(column))
                    .ok_or_else(|| invalid("Subdivision column count overflow"))?;
                faces.push(Quad {
                    vertices,
                    source: face.source,
                    row,
                    column,
                });
            }
        }
        if points
            .iter()
            .any(|point| !point.is_finite() || point.abs().max_element() > MAX_COORDINATE)
        {
            return Err(invalid(
                "Subdivided terrain positions must fit finite coordinates within ±65536",
            ));
        }
        Ok(Self { points, faces })
    }
}

pub fn plan(document: &Document, faces: &[FaceId]) -> Result<Plan, EditError> {
    plan_inner(document, faces, document.vmf())
}

pub fn plan_with_context(
    document: &Document,
    faces: &[FaceId],
    context: &Vmf,
) -> Result<Plan, EditError> {
    context
        .validate_ids()
        .map_err(|error| EditError::Invalid(error.to_string()))?;
    for face in faces {
        let (owner, original) = crate::subobject::solid(document, face.solid)?;
        let solids = match owner {
            crate::ObjectId::Solid(_) => &context.world.solids,
            crate::ObjectId::Entity(id) => {
                let original = document
                    .entity(id)
                    .ok_or_else(|| invalid("Subdivision lost its original source owner"))?;
                let assembled = context
                    .entities
                    .iter()
                    .find(|entity| entity.id == id)
                    .filter(|entity| {
                        entity.classname() == original.classname()
                            && entity.hidden == original.hidden
                    })
                    .ok_or_else(|| {
                        invalid("Resolved subdivision context changed a source owner")
                    })?;
                &assembled.solids
            }
        };
        let assembled = solids
            .iter()
            .find(|solid| solid.id == face.solid)
            .ok_or_else(|| invalid("Resolved subdivision context lost an original source"))?;
        let mut canonical = original.clone();
        crate::brush::canonicalize_precise(&mut canonical)?;
        if assembled.sides != original.sides && assembled.sides != canonical.sides
            || assembled.hidden != original.hidden
        {
            return Err(invalid(
                "Resolved subdivision context must preserve original source sides and visibility",
            ));
        }
    }
    plan_inner(document, faces, context)
}

fn plan_inner(document: &Document, faces: &[FaceId], context: &Vmf) -> Result<Plan, EditError> {
    let grids = edit::inspect(document, faces)?;
    let mut native_work = 0usize;
    let mut consume = |amount| {
        native_work = native_work
            .checked_add(amount)
            .filter(|work| *work <= MAX_TOPOLOGY_WORK)
            .ok_or_else(|| "Subdivision native topology exceeded its work limit".to_string())?;
        Ok(())
    };
    super::native_topology::check(context, faces, &[], &mut consume)?;
    for grid in &grids {
        let info = surface::side(document, grid.face)
            .unwrap()
            .displacement
            .as_ref()
            .unwrap();
        if !info.extra.is_empty() || grid.mesh.offset_normals_need_remapping(info) {
            return Err(invalid(
                "Opaque terrain blocks or offset-normal caches need remapping before subdivision",
            ));
        }
        grid.mesh.validate_edit_bounds()?;
    }
    let corners = grids
        .iter()
        .map(|grid| {
            let n = grid.mesh.size;
            [0, n * (n - 1), n * n - 1, n - 1].map(|id| grid.mesh.base_positions[id])
        })
        .collect::<Vec<_>>();
    let views = grids
        .iter()
        .zip(&corners)
        .map(|(grid, corners)| neighbors::SurfaceGrid {
            power: grid.power,
            corners: corners.map(|point| point.as_vec3().as_dvec3()),
            positions: &grid.mesh.base_positions,
        })
        .collect::<Vec<_>>();
    let neighbors =
        neighbors::find_spans(&views, &mut |_| Ok(())).map_err(|error| invalid(&error))?;
    if neighbors.ambiguous > 0 || neighbors.mismatched > 0 {
        return Err(invalid(
            "Subdivision needs unambiguous matching full or whole-to-half cage edges",
        ));
    }
    let identities = grids.iter().map(|grid| grid.face).collect::<Vec<_>>();
    let (mut mesh, mut work) = cage::build(&corners, &neighbors.pairs, &identities)?;
    let mut topology = Topology::build(&mesh, &work.stages[0], grids.len())?;
    cage::validate_links(&mesh, &topology, &work.sources, &neighbors.pairs)?;
    for stage in &work.stages[1..] {
        mesh = mesh.refine(&topology, stage)?;
        if stage.level != REFINEMENT_LEVELS {
            topology = Topology::build(&mesh, stage, grids.len())?;
        }
    }
    drop(topology);
    let mut samples = work
        .sources
        .iter()
        .map(|source| vec![None; source.sample_slots])
        .collect::<Vec<_>>();
    for face in &mesh.faces {
        let size = work.sources[face.source].sample_size;
        for (vertex, (row, column)) in face.vertices.into_iter().zip([
            (face.row, face.column),
            (face.row + 1, face.column),
            (face.row + 1, face.column + 1),
            (face.row, face.column + 1),
        ]) {
            let previous = samples[face.source][row * size + column].replace(vertex);
            if previous.is_some_and(|previous| previous != vertex) {
                return Err(invalid("Subdivision produced conflicting grid samples"));
            }
        }
    }
    let mut edits = Vec::with_capacity(grids.len());
    let mut vertices = 0usize;
    for (index, grid) in grids.iter().enumerate() {
        let side = surface::side(document, grid.face).unwrap();
        let info = side.displacement.as_ref().unwrap();
        let size = grid.mesh.size;
        let geometry = Face {
            side: 0,
            plane: crate::Plane::from_points(side.plane.0)
                .ok_or_else(|| invalid("Terrain base plane is degenerate"))?,
            vertices: surface::face_vertices(document, grid.face)?,
        };
        let mut result = info.clone();
        result.offsets = None;
        let baseline = DisplacementMesh::build(&geometry, &result)
            .map_err(|error| invalid(&error.to_string()))?;
        let sample_size = work.sources[index].sample_size;
        let stride = (sample_size - 1) / (size - 1);
        let mut offsets = vec![vec![DVec3::ZERO; size]; size];
        for (row, output) in offsets.iter_mut().enumerate() {
            for (column, offset) in output.iter_mut().enumerate() {
                let vertex = samples[index][row * stride * sample_size + column * stride]
                    .ok_or_else(|| invalid("Subdivision is missing a grid sample"))?;
                *offset = mesh.points[vertex] - baseline.positions[row * size + column];
                if !offset.is_finite() {
                    return Err(invalid("Subdivision offsets must remain finite"));
                }
            }
        }
        result.offsets = Some(offsets);
        result.subdivided = true;
        let stored = DisplacementMesh::build(&geometry, &result)
            .map_err(|error| invalid(&error.to_string()))?;
        for (vertex, point) in stored.positions.iter().enumerate() {
            let row = vertex / size;
            let column = vertex % size;
            let expected =
                mesh.points[samples[index][row * stride * sample_size + column * stride].unwrap()];
            if point.abs().max_element() > MAX_COORDINATE || point.distance(expected) > 1e-8 {
                return Err(invalid(
                    "Stored terrain rows cannot represent the subdivided samples precisely",
                ));
            }
        }
        vertices = vertices
            .checked_add(
                size.checked_mul(size)
                    .ok_or_else(|| invalid("Subdivision sample count overflow"))?,
            )
            .ok_or_else(|| invalid("Subdivision sample count overflow"))?;
        edits.push((grid.face, result));
    }
    let mut final_solids = BTreeMap::new();
    for (face, info) in &edits {
        if let std::collections::btree_map::Entry::Vacant(entry) = final_solids.entry(face.solid) {
            let (_, original) = crate::subobject::solid(document, face.solid)?;
            entry.insert(original.clone());
        }
        let solid = final_solids.get_mut(&face.solid).unwrap();
        let side = solid
            .sides
            .iter_mut()
            .find(|side| side.id == face.side)
            .ok_or_else(|| invalid("Subdivision lost its original terrain side"))?;
        side.displacement = Some(info.clone());
    }
    let native_replacements = final_solids
        .iter()
        .map(|(id, solid)| {
            Ok(super::native_topology::Replacement {
                source: *faces
                    .iter()
                    .find(|face| face.solid == *id)
                    .ok_or_else(|| invalid("Subdivision lost its original terrain face"))?,
                parts: std::slice::from_ref(solid),
            })
        })
        .collect::<Result<Vec<_>, EditError>>()?;
    super::native_topology::check(context, faces, &native_replacements, &mut consume)?;
    work.topology_work_bound = work
        .topology_work_bound
        .checked_add(native_work)
        .filter(|work| *work <= MAX_TOPOLOGY_WORK)
        .ok_or_else(|| invalid("Subdivision exceeds its native and cage topology work limit"))?;
    Ok(Plan {
        edits,
        shared_edges: neighbors.pairs.len(),
        vertices,
        work,
    })
}
