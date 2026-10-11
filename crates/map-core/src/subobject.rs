use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use source_formats::vmf::Solid;

use crate::{BrushGeometry, Document, EditError, ObjectId, Plane, Transaction};

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
        let result = if sides.len() == 2 {
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
                "Brush {} has no vertex or edge incident to faces {sides:?}",
                self.solid
            ))
        })
    }
}

#[derive(Clone)]
pub struct Replacement {
    owner: ObjectId,
    solid: Solid,
}

pub fn plan_move(
    document: &Document,
    components: &[Component],
    delta: DVec3,
) -> Result<Vec<Replacement>, EditError> {
    if components.is_empty() || components.len() > MAX_COMPONENTS {
        return Err(invalid("Select between 1 and 512 vertices or edges"));
    }
    if !delta.is_finite() || delta.abs().max_element() > MAX_COORDINATE || delta == DVec3::ZERO {
        return Err(invalid(
            "Move needs a finite nonzero offset within ±65536 units",
        ));
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
                "Brush {id} has displacements; component movement is unsupported"
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
                vertex.position
                    + if selected.contains(&index) {
                        delta
                    } else {
                        DVec3::ZERO
                    }
            })
            .collect::<Vec<_>>();
        if positions
            .iter()
            .any(|point| !point.is_finite() || point.abs().max_element() > MAX_COORDINATE)
        {
            return Err(invalid(format!(
                "Brush {id}: moved vertices exceed ±65536 units"
            )));
        }
        for edge in &topology.edges {
            if positions[edge.vertices[0]].distance(positions[edge.vertices[1]]) <= MIN_EDGE {
                return Err(invalid(format!(
                    "Brush {id}: move collapses edge {:?}",
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
            let triple = ring[1..]
                .windows(2)
                .map(|pair| [ring[0], pair[0], pair[1]])
                .find(|points| {
                    (points[0] - points[1])
                        .cross(points[2] - points[1])
                        .length()
                        > EPSILON
                })
                .ok_or_else(|| {
                    invalid(format!(
                        "Brush {id}, face {}: move collapses the face",
                        face.side
                    ))
                })?;
            let plane = Plane::from_points(triple).unwrap();
            if ring
                .iter()
                .any(|point| plane.distance_to(*point).abs() > EPSILON)
            {
                return Err(invalid(format!(
                    "Brush {id}, face {}: move makes the face nonplanar; move the whole edge or face together",
                    face.side
                )));
            }
            if positions
                .iter()
                .any(|point| plane.distance_to(*point) > EPSILON)
            {
                return Err(invalid(format!(
                    "Brush {id}, face {}: move makes the brush concave or inverted",
                    face.side
                )));
            }
            if face.vertices.iter().any(|index| selected.contains(index)) {
                replacement
                    .sides
                    .iter_mut()
                    .find(|side| side.id == face.side)
                    .unwrap()
                    .plane
                    .0 = triple;
            }
        }
        let rebuilt = self::topology(&replacement)?;
        if rebuilt.vertices.len() != topology.vertices.len()
            || rebuilt.edges.len() != topology.edges.len()
        {
            return Err(invalid(format!(
                "Brush {id}: move changes topology; splitting and welding are not supported"
            )));
        }
        for (index, vertex) in topology.vertices.iter().enumerate() {
            if !rebuilt.vertices.iter().any(|after| {
                after.sides == vertex.sides
                    && after.position.distance(positions[index]) < EPSILON * 10.0
            }) {
                return Err(invalid(format!(
                    "Brush {id}: move changes vertex identity or enclosed geometry"
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{clip, edit, primitive};

    fn block(document: &mut Document, origin: DVec3, size: f64) -> u32 {
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(tx, origin, origin + DVec3::splat(size), "brick/test")
            })
            .unwrap()
        else {
            unreachable!()
        };
        id
    }

    fn edge(document: &Document, id: u32) -> Component {
        let topology = topology(solid(document, id).unwrap().1).unwrap();
        let max = topology
            .vertices
            .iter()
            .map(|vertex| vertex.position.z)
            .fold(f64::MIN, f64::max);
        let edge = topology
            .edges
            .iter()
            .find(|edge| {
                edge.vertices
                    .iter()
                    .all(|index| (topology.vertices[*index].position.z - max).abs() < EPSILON)
            })
            .unwrap();
        Component {
            solid: id,
            sides: edge.sides.clone(),
        }
    }

    #[test]
    fn cube_topology_has_stable_incidence_and_closed_adjacency() {
        let mut document = Document::empty();
        let id = block(&mut document, DVec3::ZERO, 64.0);
        let shape = topology(document.solid(id).unwrap()).unwrap();
        assert_eq!(
            (shape.vertices.len(), shape.edges.len(), shape.faces.len()),
            (8, 12, 6)
        );
        for vertex in &shape.vertices {
            assert_eq!(shape.resolve(&vertex.sides).unwrap().len(), 1);
        }
        for edge in &shape.edges {
            assert_eq!(shape.resolve(&edge.sides).unwrap(), edge.vertices);
        }
        let mut reordered = document.solid(id).unwrap().clone();
        reordered.sides.reverse();
        let reordered = topology(&reordered).unwrap();
        for vertex in &shape.vertices {
            let index = reordered.resolve(&vertex.sides).unwrap()[0];
            assert!(
                reordered.vertices[index]
                    .position
                    .abs_diff_eq(vertex.position, EPSILON)
            );
        }
    }

    #[test]
    fn edge_moves_preserve_metadata_and_round_trip_through_undo_and_vmf() {
        let mut document = Document::empty();
        let id = block(&mut document, DVec3::ZERO, 64.0);
        document
            .edit("metadata", |tx| {
                let side = &mut tx.solid_mut(id)?.sides[0];
                side.extra_properties.set("custom", "retained");
                side.lightmap_scale = 8;
                side.smoothing_groups = Some(7);
                side.u_axis.offset = 17.25;
                Ok(())
            })
            .unwrap();
        let before = document.solid(id).unwrap().clone();
        let selected = edge(&document, id);
        let plan = plan_move(&document, &[selected], DVec3::Z * 16.0).unwrap();
        document.edit("move", |tx| apply(tx, &plan)).unwrap();
        let after = document.solid(id).unwrap().clone();
        assert_ne!(before, after);
        for (before, after) in before.sides.iter().zip(&after.sides) {
            let mut expected = before.clone();
            expected.plane = after.plane;
            assert_eq!(expected, *after);
        }
        let parsed =
            source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        assert_eq!(topology(&parsed.world.solids[0]).unwrap().vertices.len(), 8);
        document.undo().unwrap();
        assert_eq!(document.solid(id).unwrap(), &before);
        document.redo().unwrap();
        assert_eq!(document.solid(id).unwrap(), &after);
    }

    #[test]
    fn rejects_nonplanar_corners_collapses_inversions_and_invalid_offsets() {
        let mut document = Document::empty();
        let id = block(&mut document, DVec3::ZERO, 64.0);
        let shape = topology(document.solid(id).unwrap()).unwrap();
        let vertex = Component {
            solid: id,
            sides: shape.vertices[0].sides.clone(),
        };
        let error = plan_move(&document, &[vertex], DVec3::splat(4.0))
            .err()
            .unwrap();
        assert!(error.to_string().contains("nonplanar"));
        let selected = edge(&document, id);
        for delta in [
            DVec3::ZERO,
            DVec3::splat(f64::NAN),
            DVec3::splat(f64::INFINITY),
            DVec3::Z * -64.0,
            DVec3::Z * -96.0,
            DVec3::Z * 100_000.0,
        ] {
            assert!(
                plan_move(&document, std::slice::from_ref(&selected), delta).is_err(),
                "{delta}"
            );
        }
    }

    #[test]
    fn face_vertex_sets_translate_small_and_large_brushes_precisely() {
        for (origin, size, delta) in [
            (DVec3::ZERO, 0.125, 0.012_345_678_9),
            (DVec3::splat(32_000.0), 64.0, 0.012_345_678_9),
        ] {
            let mut document = Document::empty();
            let id = block(&mut document, origin, size);
            let shape = topology(document.solid(id).unwrap()).unwrap();
            let selected = shape
                .vertices
                .iter()
                .filter(|vertex| vertex.position.z == origin.z + size)
                .map(|vertex| Component {
                    solid: id,
                    sides: vertex.sides.clone(),
                })
                .collect::<Vec<_>>();
            let plan = plan_move(&document, &selected, DVec3::Z * delta).unwrap();
            document.edit("move", |tx| apply(tx, &plan)).unwrap();
            let moved = BrushGeometry::from_solid(document.solid(id).unwrap()).unwrap();
            assert!((moved.maxs.z - (origin.z + size + delta)).abs() < 1e-8);
        }
    }

    #[test]
    fn wedge_and_oblique_brushes_keep_component_identity() {
        let mut document = Document::empty();
        let ids = document
            .edit("wedge", |tx| {
                primitive::create(tx, &primitive::Spec::default())
            })
            .unwrap();
        let ObjectId::Solid(id) = ids[0] else {
            unreachable!()
        };
        let shape = topology(document.solid(id).unwrap()).unwrap();
        assert_eq!(shape.vertices.len(), 6);
        let selected = edge(&document, id);
        assert!(plan_move(&document, &[selected], DVec3::Z * 16.0).is_ok());
        let id = block(&mut document, DVec3::ZERO, 64.0);
        document
            .edit("clip", |tx| {
                clip::clip(
                    tx,
                    &[ObjectId::Solid(id)],
                    Plane {
                        normal: DVec3::new(1.0, 1.0, 1.0),
                        dist: 140.0,
                    },
                    clip::Keep::Back,
                    None,
                )
            })
            .unwrap();
        let shape = topology(document.solid(id).unwrap()).unwrap();
        let selected = shape
            .vertices
            .iter()
            .map(|vertex| Component {
                solid: id,
                sides: vertex.sides.clone(),
            })
            .collect::<Vec<_>>();
        assert!(plan_move(&document, &selected, DVec3::new(1.2, -3.4, 5.6)).is_ok());
    }

    #[test]
    fn malformed_and_ambiguous_brushes_are_rejected() {
        let mut document = Document::empty();
        let id = block(&mut document, DVec3::ZERO, 64.0);
        let source = document.solid(id).unwrap().clone();
        let mut malformed = source.clone();
        malformed.sides[1].id = malformed.sides[0].id;
        assert!(topology(&malformed).is_err());
        let mut malformed = source.clone();
        malformed.sides.pop();
        assert!(topology(&malformed).is_err());
        let mut malformed = source.clone();
        malformed.sides.push(source.sides[0].clone());
        malformed.sides.last_mut().unwrap().id = 900;
        assert!(topology(&malformed).is_err());
        let mut vmf = document.vmf().clone();
        vmf.world.solids.push(source);
        assert!(solid(&Document::new(vmf), id).is_err());
    }

    #[test]
    fn rejected_multibrush_batch_changes_nothing() {
        let mut document = Document::empty();
        let first = block(&mut document, DVec3::ZERO, 64.0);
        let second = block(&mut document, DVec3::splat(128.0), 64.0);
        let shape = topology(document.solid(second).unwrap()).unwrap();
        let selected = vec![
            edge(&document, first),
            Component {
                solid: second,
                sides: shape.vertices[0].sides.clone(),
            },
        ];
        let before = document.vmf().clone();
        let revision = document.revision();
        assert!(plan_move(&document, &selected, DVec3::splat(16.0)).is_err());
        assert_eq!(*document.vmf(), before);
        assert_eq!(document.revision(), revision);
    }

    #[test]
    fn entity_brush_moves_preserve_ownership_and_reject_displacements() {
        let mut document = Document::empty();
        let id = block(&mut document, DVec3::ZERO, 64.0);
        let selected = edge(&document, id);
        let owner = document
            .edit("tie", |tx| {
                edit::tie_to_entity(tx, &[id], "func_detail", &[])
            })
            .unwrap();
        let before = document.vmf().clone();
        let plan = plan_move(&document, std::slice::from_ref(&selected), DVec3::Z * 16.0).unwrap();
        document.edit("move", |tx| apply(tx, &plan)).unwrap();
        assert_eq!(document.owner_of_solid(id), Some(owner));
        assert_eq!(
            document.vmf().entities[0].properties,
            before.entities[0].properties
        );
        document.undo().unwrap();
        assert_eq!(*document.vmf(), before);
        let mut displaced = before;
        displaced.entities[0].solids[0].sides[0].displacement =
            Some(source_formats::vmf::DispInfo {
                power: 2,
                start_position: DVec3::ZERO,
                flags: None,
                elevation: 0.0,
                subdivided: false,
                normals: None,
                distances: None,
                offsets: None,
                offset_normals: None,
                alphas: None,
                triangle_tags: None,
                allowed_verts: None,
                extra_properties: Default::default(),
                extra: Vec::new(),
            });
        let error = plan_move(&Document::new(displaced), &[selected], DVec3::Z * 16.0)
            .err()
            .unwrap();
        assert!(error.to_string().contains("displacements"));
    }
}
