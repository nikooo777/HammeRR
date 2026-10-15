use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use source_formats::vmf::DispInfo;

use super::{DisplacementMesh, edit, neighbors};
use crate::{
    Document, EditError,
    brush::Face,
    primitive::MAX_COORDINATE,
    surface::{self, FaceId},
};

const LEVELS: usize = 4;
const CELLS: usize = 1 << LEVELS;
const SIZE: usize = CELLS + 1;
const MAX_FACES: usize = 16;

#[cfg(test)]
mod tests;

pub struct Plan {
    pub edits: Vec<(FaceId, DispInfo)>,
    pub shared_edges: usize,
    pub vertices: usize,
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
    fn build(mesh: &Mesh) -> Result<Self, EditError> {
        if mesh.faces.len() > MAX_FACES * CELLS * CELLS
            || mesh.points.len() > MAX_FACES * SIZE * SIZE
        {
            return Err(invalid("Terrain subdivision exceeds its bounded cage size"));
        }
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
    fn refine(self) -> Result<Self, EditError> {
        let topology = Topology::build(&self)?;
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
        let mut points =
            Vec::with_capacity(self.points.len() + topology.edges.len() + centers.len());
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
        let mut faces = Vec::with_capacity(self.faces.len() * 4);
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
                faces.push(Quad {
                    vertices,
                    source: face.source,
                    row: face.row * 2 + row,
                    column: face.column * 2 + column,
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
    let grids = edit::inspect(document, faces)?;
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
    if neighbors.ambiguous > 0
        || neighbors.pairs.iter().any(|pair| {
            pair.first_span != neighbors::Span::Full || pair.second_span != neighbors::Span::Full
        })
    {
        return Err(invalid(
            "Subdivision needs unambiguous full-edge cages; half-edge junctions remain unsupported",
        ));
    }
    let mut mesh = Mesh {
        points: Vec::new(),
        faces: Vec::with_capacity(grids.len()),
    };
    let mut lookup = BTreeMap::new();
    for (source, corners) in corners.into_iter().enumerate() {
        let mut vertices = [0; 4];
        for (slot, point) in vertices.iter_mut().zip(corners) {
            let key = neighbors::key(point.as_vec3().as_dvec3());
            *slot = *lookup.entry(key).or_insert_with(|| {
                let id = mesh.points.len();
                mesh.points.push(point);
                id
            });
        }
        mesh.faces.push(Quad {
            vertices,
            source,
            row: 0,
            column: 0,
        });
    }
    let topology = Topology::build(&mesh)?;
    let shared_edges = topology
        .edges
        .iter()
        .filter(|edge| edge.faces.len() == 2)
        .count();
    if shared_edges != neighbors.pairs.len() {
        return Err(invalid(
            "Subdivision cages need matching Source-float corner positions",
        ));
    }
    for _ in 0..LEVELS {
        mesh = mesh.refine()?;
    }
    let mut samples = vec![vec![None; SIZE * SIZE]; grids.len()];
    for face in &mesh.faces {
        for (vertex, (row, column)) in face.vertices.into_iter().zip([
            (face.row, face.column),
            (face.row + 1, face.column),
            (face.row + 1, face.column + 1),
            (face.row, face.column + 1),
        ]) {
            let previous = samples[face.source][row * SIZE + column].replace(vertex);
            if previous.is_some_and(|previous| previous != vertex) {
                return Err(invalid("Subdivision produced conflicting grid samples"));
            }
        }
    }
    let mut edits = Vec::with_capacity(grids.len());
    let mut vertices = 0;
    for (index, grid) in grids.iter().enumerate() {
        let side = surface::side(document, grid.face).unwrap();
        let info = side.displacement.as_ref().unwrap();
        let size = grid.mesh.size;
        if !info.extra.is_empty() || grid.mesh.offset_normals_need_remapping(info) {
            return Err(invalid(
                "Opaque terrain blocks or offset-normal caches need remapping before subdivision",
            ));
        }
        grid.mesh.validate_edit_bounds()?;
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
        let stride = CELLS / (size - 1);
        let mut offsets = vec![vec![DVec3::ZERO; size]; size];
        for (row, output) in offsets.iter_mut().enumerate() {
            for (column, offset) in output.iter_mut().enumerate() {
                let vertex = samples[index][row * stride * SIZE + column * stride]
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
                mesh.points[samples[index][row * stride * SIZE + column * stride].unwrap()];
            if point.abs().max_element() > MAX_COORDINATE || point.distance(expected) > 1e-8 {
                return Err(invalid(
                    "Stored terrain rows cannot represent the subdivided samples precisely",
                ));
            }
        }
        vertices += size * size;
        edits.push((grid.face, result));
    }
    Ok(Plan {
        edits,
        shared_edges,
        vertices,
    })
}
