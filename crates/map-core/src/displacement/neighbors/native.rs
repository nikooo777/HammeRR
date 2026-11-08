use std::collections::{BTreeMap, BTreeSet};

use source_formats::bsp::{DisplacementCorner, DisplacementNeighbor};

use super::{Budget, EDGE_TOLERANCE, MAX_TOPOLOGY_WORK, NeighborPair, Neighbors, SurfaceGrid};

pub struct NativeConnections {
    pub edges: Vec<[[DisplacementNeighbor; 2]; 4]>,
    pub corners: Vec<[DisplacementCorner; 4]>,
    pub corner_pairs: usize,
}

pub fn native_connections(
    grids: &[SurfaceGrid<'_>],
    pairs: &[NeighborPair],
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<NativeConnections, String> {
    connections(grids, pairs, None, consume)
}

/// Discovers reciprocal native edges and displaced corners within one shared work budget.
/// Positions must already be reconstructed from native storage; authored grids use the exact APIs.
pub fn native_neighborhood(
    grids: &[SurfaceGrid<'_>],
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<(Neighbors, NativeConnections), String> {
    let mut budget = Budget {
        work: 0,
        limit: MAX_TOPOLOGY_WORK,
        consume,
    };
    let super::discovery::Discovery {
        neighbors,
        edge_pairs,
    } = super::discovery::find_native_spans(grids, &mut |amount| budget.consume(amount))?;
    if neighbors.ambiguous != 0 {
        return Err("Native terrain endpoint discovery has ambiguous occupied spans".into());
    }
    let connections = connections(grids, &neighbors.pairs, Some(&edge_pairs), &mut |amount| {
        budget.consume(amount)
    })?;
    Ok((neighbors, connections))
}

fn connections(
    grids: &[SurfaceGrid<'_>],
    pairs: &[NeighborPair],
    endpoint_pairs: Option<&BTreeSet<(usize, usize)>>,
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<NativeConnections, String> {
    if grids.len() > source_formats::bsp::MAX_DISPLACEMENTS
        || pairs.len() > grids.len() * 4
        || grids.iter().any(|grid| {
            !(2..=4).contains(&grid.power)
                || grid.positions.len() != ((1usize << grid.power) + 1).pow(2)
        })
        || pairs.iter().any(|pair| {
            pair.first >= grids.len()
                || pair.second >= grids.len()
                || pair.first == pair.second
                || pair.first_edge >= 4
                || pair.second_edge >= 4
        })
    {
        return Err("Native terrain connections require bounded grids and valid pairs".into());
    }
    let mut budget = Budget {
        work: 0,
        limit: MAX_TOPOLOGY_WORK,
        consume,
    };
    let mut edges = vec![[[DisplacementNeighbor::default(); 2]; 4]; grids.len()];
    for pair in pairs {
        for (index, edge, span, other, other_edge, other_span) in [
            (
                pair.first,
                pair.first_edge,
                pair.first_span,
                pair.second,
                pair.second_edge,
                pair.second_span,
            ),
            (
                pair.second,
                pair.second_edge,
                pair.second_span,
                pair.first,
                pair.first_edge,
                pair.first_span,
            ),
        ] {
            let span = span.native(edge);
            let slot = usize::from(span == 2);
            if edges[index][edge][slot].index != u16::MAX {
                return Err("Terrain neighbor spans overlap an occupied native slot".into());
            }
            edges[index][edge][slot] = DisplacementNeighbor {
                index: other as u16,
                orientation: ((2 + other_edge + 4 - edge) % 4) as u8,
                span,
                neighbor_span: other_span.native(other_edge),
                ..Default::default()
            };
        }
    }
    let shared = if endpoint_pairs.is_some() {
        super::discovery::shared_corners(grids, &mut budget)?
    } else {
        exact_corners(grids, &mut budget)?
    };
    let mut corners = vec![[DisplacementCorner::default(); 4]; grids.len()];
    let mut edge_pairs = edges
        .iter()
        .enumerate()
        .flat_map(|(index, edges)| {
            edges
                .iter()
                .flatten()
                .filter(|neighbor| neighbor.index != u16::MAX)
                .map(move |neighbor| (index, neighbor.index as usize))
        })
        .collect::<BTreeSet<_>>();
    if let Some(endpoint_pairs) = endpoint_pairs {
        budget.consume(endpoint_pairs.len())?;
        edge_pairs.extend(endpoint_pairs.iter().copied());
    }
    let mut corner_pairs = 0;
    for ((a, b), (corner_a, corner_b, count)) in shared {
        budget.consume(1)?;
        if count != 1 || edge_pairs.contains(&(a, b)) || edge_pairs.contains(&(b, a)) {
            continue;
        }
        let point = |index: usize, corner: usize| {
            let grid = &grids[index];
            let size = (1usize << grid.power) + 1;
            grid.positions[[0, (size - 1) * size, size * size - 1, size - 1][corner]]
        };
        if point(a, corner_a).distance(point(b, corner_b)) > EDGE_TOLERANCE {
            continue;
        }
        for (index, corner, other) in [(a, corner_a, b), (b, corner_b, a)] {
            let list = &mut corners[index][corner];
            if list.count as usize == list.neighbors.len() {
                return Err("Native terrain corners allow up to four non-edge neighbors".into());
            }
            list.neighbors[list.count as usize] = other as u16;
            list.count += 1;
        }
        corner_pairs += 1;
    }
    Ok(NativeConnections {
        edges,
        corners,
        corner_pairs,
    })
}

fn exact_corners<F: FnMut(usize) -> Result<(), String>>(
    grids: &[SurfaceGrid<'_>],
    budget: &mut Budget<'_, F>,
) -> Result<super::SharedCorners, String> {
    let mut groups = BTreeMap::<[u64; 3], Vec<(usize, usize)>>::new();
    for (index, grid) in grids.iter().enumerate() {
        for (corner, point) in grid.corners.iter().enumerate() {
            budget.consume(1)?;
            let key = point
                .to_array()
                .map(|v| if v == 0.0 { 0 } else { v.to_bits() });
            groups.entry(key).or_default().push((index, corner));
        }
    }
    let mut shared = BTreeMap::<(usize, usize), (usize, usize, usize)>::new();
    for group in groups.values() {
        for a in 0..group.len() {
            for b in a + 1..group.len() {
                budget.consume(1)?;
                let (i, c) = group[a];
                let (j, d) = group[b];
                if i == j {
                    continue;
                }
                let entry = shared.entry((i, j)).or_insert((c, d, 0));
                entry.2 += 1;
            }
        }
    }
    Ok(shared)
}
