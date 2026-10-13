use std::collections::BTreeMap;

use source_formats::bsp::DisplacementNeighbor;

use super::{GEOMETRY_EPSILON, Terrain};
use crate::emit::stored_vertex;
use crate::{Control, Diagnostic, Report};

const MAX_WORK: usize = 2_000_000;

struct Edge {
    terrain: usize,
    side: usize,
    start: [u32; 3],
    end: [u32; 3],
    indices: Vec<usize>,
}

#[derive(Default)]
struct Deferred {
    ambiguous: usize,
    power: usize,
    geometry: usize,
}

fn key(point: [f32; 3]) -> [u32; 3] {
    point.map(|value| if value == 0.0 { 0 } else { value.to_bits() })
}

fn edge_indices(size: usize, side: usize) -> Vec<usize> {
    match side {
        0 => (0..size).map(|row| row * size).collect(),
        1 => (0..size).map(|col| (size - 1) * size + col).collect(),
        2 => (0..size).rev().map(|row| row * size + size - 1).collect(),
        3 => (0..size).rev().collect(),
        _ => unreachable!(),
    }
}

fn consume(work: &mut usize, amount: usize, control: &Control) -> Result<(), String> {
    control.checkpoint()?;
    *work = work
        .checked_add(amount)
        .ok_or("Terrain neighbor work count overflow")?;
    if *work > MAX_WORK {
        return Err("Terrain neighbors exceeded their work budget".into());
    }
    Ok(())
}

pub(super) fn connect(
    terrains: &mut [Terrain],
    report: &mut Report,
    control: &Control,
) -> Result<usize, String> {
    let mut work = 0;
    let mut groups: BTreeMap<[[u32; 3]; 2], Vec<Edge>> = BTreeMap::new();
    let mut positions = Vec::with_capacity(terrains.len());
    for (index, terrain) in terrains.iter().enumerate() {
        consume(
            &mut work,
            terrain.vertices.len() + terrain.triangles.len(),
            control,
        )?;
        positions.push(terrain.native_positions()?);
        for side in 0..4 {
            consume(&mut work, 1, control)?;
            let indices = edge_indices(terrain.mesh.size, side);
            let start = key(stored_vertex(terrain.mesh.base_positions[indices[0]])
                .as_vec3()
                .to_array());
            let end = key(
                stored_vertex(terrain.mesh.base_positions[*indices.last().unwrap()])
                    .as_vec3()
                    .to_array(),
            );
            let group = if start < end {
                [start, end]
            } else {
                [end, start]
            };
            groups.entry(group).or_default().push(Edge {
                terrain: index,
                side,
                start,
                end,
                indices,
            });
        }
    }
    let mut pending = vec![[[DisplacementNeighbor::default(); 2]; 4]; terrains.len()];
    let mut pairs = 0;
    let mut deferred = Deferred::default();
    for group in groups.values() {
        consume(&mut work, 1, control)?;
        if group.len() < 2 {
            continue;
        }
        let [a, b] = group.as_slice() else {
            deferred.ambiguous += 1;
            continue;
        };
        if a.terrain == b.terrain || a.start != b.end || a.end != b.start {
            deferred.ambiguous += 1;
            continue;
        }
        if terrains[a.terrain].info.power != terrains[b.terrain].info.power {
            deferred.power += 1;
            continue;
        }
        let mut matches = true;
        for (&first, &second) in a.indices.iter().zip(b.indices.iter().rev()) {
            consume(&mut work, 1, control)?;
            let a = glam::DVec3::from_array(positions[a.terrain][first].map(f64::from));
            let b = glam::DVec3::from_array(positions[b.terrain][second].map(f64::from));
            if a.distance(b) > GEOMETRY_EPSILON {
                matches = false;
                break;
            }
        }
        if !matches {
            deferred.geometry += 1;
            continue;
        }
        for (edge, neighbor) in [(a, b), (b, a)] {
            pending[edge.terrain][edge.side][0] = DisplacementNeighbor {
                index: neighbor.terrain as u16,
                orientation: ((2 + neighbor.side + 4 - edge.side) % 4) as u8,
                ..Default::default()
            };
        }
        pairs += 1;
    }
    control.checkpoint()?;
    for (terrain, edges) in terrains.iter_mut().zip(pending) {
        terrain.info.edges = edges;
    }
    if deferred.ambiguous + deferred.power + deferred.geometry != 0 {
        report.diagnostics.push(Diagnostic::warning(format!("Terrain shared edges left unlinked: {} ambiguous, {} unequal-power, {} mismatched surfaces. Neighbor metadata currently supports exact native base edges with matching equal-power grids", deferred.ambiguous, deferred.power, deferred.geometry), None, None));
    }
    Ok(pairs)
}
