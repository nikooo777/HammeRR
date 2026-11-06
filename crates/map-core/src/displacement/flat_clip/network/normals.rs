use std::collections::BTreeSet;

use source_formats::bsp::DisplacementInfo;

use super::{LodState, Surface, Work, invalid, neighbors};
use crate::EditError;

const CORNER_SEARCH_BOUND: f64 = 0.125;

struct Supports {
    parents: Vec<usize>,
    signs: Vec<[i8; 3]>,
    patches: usize,
}

impl Supports {
    fn root(&mut self, mut index: usize, work: &mut Work) -> Result<usize, EditError> {
        while self.parents[index] != index {
            work.consume(1).map_err(invalid)?;
            self.parents[index] = self.parents[self.parents[index]];
            index = self.parents[index];
        }
        Ok(index)
    }

    fn join(&mut self, first: usize, second: usize, work: &mut Work) -> Result<(), EditError> {
        work.consume(1).map_err(invalid)?;
        let first = self.root(first, work)?;
        let second = self.root(second, work)?;
        if first != second {
            let common = DisplacementInfo::intersect_native_normal_support(
                self.signs[first],
                self.signs[second],
                self.patches,
            )
            .map_err(|error| invalid(error.to_string()))?;
            let owner = first.min(second);
            self.parents[first.max(second)] = owner;
            self.signs[owner] = common;
        }
        Ok(())
    }
}

fn corner_indices(surface: &Surface<'_>) -> [usize; 4] {
    let size = surface.mesh.size;
    [0, (size - 1) * size, size * size - 1, size - 1]
}

struct Smoothing<'a, 'b> {
    surfaces: &'a [&'b Surface<'b>],
    states: &'a [LodState],
    offsets: &'a [usize],
    supports: &'a mut Supports,
    endpoints: &'a mut BTreeSet<usize>,
}

impl Smoothing<'_, '_> {
    fn corners(
        &mut self,
        patch: usize,
        vertex: usize,
        peer: usize,
        work: &mut Work,
    ) -> Result<(), EditError> {
        let first = self.offsets[patch] + vertex;
        self.endpoints.insert(first);
        work.consume(4 * (self.states[patch].positions.len() + 1))
            .map_err(invalid)?;
        for other in corner_indices(self.surfaces[peer]) {
            // An exact 1/8 coordinate gap excludes the native 0.1 distance search.
            let possible = self.states[patch]
                .positions
                .iter()
                .zip(&self.states[peer].positions)
                .any(|(a, b)| {
                    (0..3)
                        .all(|axis| (a[vertex][axis] - b[other][axis]).abs() < CORNER_SEARCH_BOUND)
                });
            if possible {
                let second = self.offsets[peer] + other;
                self.endpoints.insert(second);
                self.supports.join(first, second, work)?;
            }
        }
        Ok(())
    }
}

pub(super) fn certify(
    surfaces: &[&Surface<'_>],
    states: &[LodState],
    pairs: &[neighbors::NeighborPair],
    connections: &neighbors::NativeConnections,
    work: &mut Work,
) -> Result<(), EditError> {
    let mut offsets = Vec::with_capacity(surfaces.len());
    let mut signs = Vec::new();
    for (surface, state) in surfaces.iter().zip(states) {
        offsets.push(signs.len());
        work.consume(state.positions.iter().map(Vec::len).sum())
            .map_err(invalid)?;
        if state.positions == surface.positions {
            signs.extend_from_slice(
                surface.normal_support.as_ref().ok_or_else(|| {
                    invalid("Terrain normal proof lost its prepared native support")
                })?,
            );
        } else {
            signs.extend(super::super::local_normal_support(
                surface.info,
                &state.positions,
                work,
            )?);
        }
    }
    work.consume(signs.len()).map_err(invalid)?;
    let mut supports = Supports {
        parents: (0..signs.len()).collect(),
        signs,
        patches: surfaces.len(),
    };
    let mut endpoints = BTreeSet::new();
    let mut interpolations = Vec::new();
    for pair in pairs {
        let first = surfaces[pair.first];
        let second = surfaces[pair.second];
        let first_indices = pair
            .first_span
            .indices(first.power, pair.first_edge)
            .map_err(invalid)?;
        let mut second_indices = pair
            .second_span
            .indices(second.power, pair.second_edge)
            .map_err(invalid)?;
        second_indices.reverse();
        let steps = (first_indices.len() - 1).min(second_indices.len() - 1);
        work.consume(first_indices.len() + second_indices.len())
            .map_err(invalid)?;
        for index in 0..=steps {
            let a = offsets[pair.first] + first_indices[index * (first_indices.len() - 1) / steps];
            let b =
                offsets[pair.second] + second_indices[index * (second_indices.len() - 1) / steps];
            endpoints.extend([a, b]);
            supports.join(a, b, work)?;
        }
        for (patch, indices) in [(pair.first, first_indices), (pair.second, second_indices)] {
            let stride = (indices.len() - 1) / steps;
            if stride == 1 {
                continue;
            }
            for interval in 0..steps {
                let ends = [
                    offsets[patch] + indices[interval * stride],
                    offsets[patch] + indices[(interval + 1) * stride],
                ];
                for &index in &indices[interval * stride + 1..(interval + 1) * stride] {
                    interpolations.push((offsets[patch] + index, ends));
                }
            }
        }
    }
    let mut smoothing = Smoothing {
        surfaces,
        states,
        offsets: &offsets,
        supports: &mut supports,
        endpoints: &mut endpoints,
    };
    for (patch, surface) in surfaces.iter().enumerate() {
        let mut peers = BTreeSet::new();
        for list in &connections.corners[patch] {
            peers.extend(
                list.neighbors[..usize::from(list.count)]
                    .iter()
                    .map(|&id| usize::from(id)),
            );
        }
        for (edge, slots) in connections.edges[patch].iter().enumerate() {
            peers.extend(
                slots
                    .iter()
                    .filter(|slot| slot.index != u16::MAX)
                    .map(|slot| usize::from(slot.index)),
            );
            if slots.iter().all(|slot| slot.index != u16::MAX) {
                let indices = neighbors::edge_indices(surface.mesh.size, edge).map_err(invalid)?;
                for slot in slots {
                    smoothing.corners(
                        patch,
                        indices[indices.len() / 2],
                        usize::from(slot.index),
                        work,
                    )?;
                }
            }
        }
        for vertex in corner_indices(surface) {
            smoothing.endpoints.insert(offsets[patch] + vertex);
            for &peer in &peers {
                smoothing.corners(patch, vertex, peer, work)?;
            }
        }
    }
    let mut targets = BTreeSet::new();
    for (target, ends) in interpolations {
        work.consume(3).map_err(invalid)?;
        if endpoints.contains(&target) || !targets.insert(target) {
            return Err(invalid(
                "Native normal interpolation requires unique fine samples outside all averaging endpoints",
            ));
        }
        let a = supports.root(ends[0], work)?;
        let b = supports.root(ends[1], work)?;
        DisplacementInfo::intersect_native_normal_support(
            supports.signs[a],
            supports.signs[b],
            supports.patches,
        )
        .map_err(|error| invalid(error.to_string()))?;
    }
    Ok(())
}
