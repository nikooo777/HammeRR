use super::{MAX_CAGE_CELLS, Mesh, Quad, Topology, invalid};
use crate::{
    EditError,
    displacement::neighbors::{self, NeighborPair, Span},
};
use glam::DVec3;
use std::collections::BTreeMap;

const CONTACT_EPSILON: f64 = 1e-8;

fn density(count: usize, pairs: &[NeighborPair]) -> Result<Vec<usize>, EditError> {
    let mut levels = vec![None; count];
    let mut cells = vec![1; count];
    for first in 0..count {
        if levels[first].is_some() {
            continue;
        }
        levels[first] = Some(0i32);
        let mut component = vec![first];
        let mut next = 0;
        while next < component.len() {
            let source = component[next];
            let level = levels[source].unwrap();
            for pair in pairs {
                let delta = i32::from(pair.second_span != Span::Full)
                    - i32::from(pair.first_span != Span::Full);
                let (neighbor, expected) = if pair.first == source {
                    (pair.second, level + delta)
                } else if pair.second == source {
                    (pair.first, level - delta)
                } else {
                    continue;
                };
                if let Some(previous) = levels[neighbor] {
                    if previous != expected {
                        return Err(invalid(
                            "Subdivision cage spans require inconsistent grid densities",
                        ));
                    }
                } else {
                    levels[neighbor] = Some(expected);
                    component.push(neighbor);
                }
            }
            next += 1;
        }
        let minimum = component
            .iter()
            .map(|&source| levels[source].unwrap())
            .min()
            .unwrap();
        let maximum = component
            .iter()
            .map(|&source| levels[source].unwrap())
            .max()
            .unwrap();
        if maximum - minimum > MAX_CAGE_CELLS.ilog2() as i32 {
            return Err(invalid(
                "Subdivision supports two cage densities; nested whole-to-half chains need separate actions",
            ));
        }
        for source in component {
            cells[source] = 1 << (levels[source].unwrap() - minimum);
        }
    }
    Ok(cells)
}

fn validate_contacts(corners: &[[DVec3; 4]], pairs: &[NeighborPair]) -> Result<(), EditError> {
    for (source, ring) in corners.iter().enumerate() {
        for edge in 0..4 {
            let a = ring[edge].as_vec3().as_dvec3();
            let b = ring[(edge + 1) % 4].as_vec3().as_dvec3();
            let direction = b - a;
            let length = direction.length_squared();
            if length == 0.0 {
                return Err(invalid(
                    "Terrain base corners collapse at Source float precision",
                ));
            }
            for (other, points) in corners.iter().enumerate() {
                if source == other {
                    continue;
                }
                for point in points.map(|point| point.as_vec3().as_dvec3()) {
                    let t = (point - a).dot(direction) / length;
                    let projected = a.lerp(b, t);
                    if !(0.0..1.0).contains(&t)
                        || t == 0.0
                        || (point.distance(projected) > CONTACT_EPSILON
                            && neighbors::key(projected.as_vec3().as_dvec3())
                                != neighbors::key(point))
                    {
                        continue;
                    }
                    if !pairs.iter().any(|pair| {
                        (pair.first == source && pair.first_edge == edge && pair.second == other)
                            || (pair.second == source
                                && pair.second_edge == edge
                                && pair.first == other)
                    }) {
                        return Err(invalid(
                            "Subdivision cage contacts must use matching full or whole-to-half edges",
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

fn point(corners: [DVec3; 4], row: usize, column: usize, cells: usize) -> DVec3 {
    match (row, column) {
        (0, 0) => corners[0],
        (r, 0) if r == cells => corners[1],
        (r, c) if r == cells && c == cells => corners[2],
        (0, c) if c == cells => corners[3],
        _ => {
            let t = row as f64 / cells as f64;
            corners[0]
                .lerp(corners[1], t)
                .lerp(corners[3].lerp(corners[2], t), column as f64 / cells as f64)
        }
    }
}

pub(super) fn build(
    corners: &[[DVec3; 4]],
    pairs: &[NeighborPair],
) -> Result<(Mesh, Vec<usize>), EditError> {
    validate_contacts(corners, pairs)?;
    let cells = density(corners.len(), pairs)?;
    let mut mesh = Mesh {
        points: Vec::new(),
        faces: Vec::new(),
    };
    let mut lookup = BTreeMap::new();
    for (source, &corners) in corners.iter().enumerate() {
        let native = corners.map(|point| point.as_vec3().as_dvec3());
        let cells = cells[source];
        for row in 0..cells {
            for column in 0..cells {
                let mut vertices = [0; 4];
                for (slot, (row, column)) in vertices.iter_mut().zip([
                    (row, column),
                    (row + 1, column),
                    (row + 1, column + 1),
                    (row, column + 1),
                ]) {
                    let position = point(corners, row, column, cells);
                    let key =
                        neighbors::key(point(native, row, column, cells).as_vec3().as_dvec3());
                    *slot = *lookup.entry(key).or_insert_with(|| {
                        let id = mesh.points.len();
                        mesh.points.push(position);
                        id
                    });
                }
                mesh.faces.push(Quad {
                    vertices,
                    source,
                    row,
                    column,
                });
            }
        }
    }
    Ok((mesh, cells))
}

pub(super) fn validate_links(
    mesh: &Mesh,
    topology: &Topology,
    cells: &[usize],
    pairs: &[NeighborPair],
) -> Result<(), EditError> {
    let mut expected = BTreeMap::new();
    for pair in pairs {
        let key = [pair.first.min(pair.second), pair.first.max(pair.second)];
        *expected.entry(key).or_insert(0) +=
            cells[pair.first] / if pair.first_span == Span::Full { 1 } else { 2 };
    }
    let mut actual = BTreeMap::new();
    for edge in &topology.edges {
        let [first, second] = edge.faces.as_slice() else {
            continue;
        };
        let a = mesh.faces[*first].source;
        let b = mesh.faces[*second].source;
        if a != b {
            *actual.entry([a.min(b), a.max(b)]).or_insert(0) += 1;
        }
    }
    if actual != expected {
        return Err(invalid(
            "Subdivision cages need matching Source-float edge samples",
        ));
    }
    Ok(())
}
