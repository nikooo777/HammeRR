use std::collections::{BTreeMap, BTreeSet};

use glam::{DVec3, Vec3};
use source_formats::bsp::{DisplacementCorner, MAX_DISPLACEMENTS};

use super::{
    Budget, NeighborPair, SurfaceGrid, native_bounds, native_bounds_touch, native_corner_contact,
    root,
};

pub(crate) struct CornerLink {
    pub(crate) first: usize,
    pub(crate) first_corner: usize,
    pub(crate) second: usize,
    pub(crate) second_corner: usize,
}

pub(crate) fn corner_links(
    pairs: &[NeighborPair],
    corners: &[[DisplacementCorner; 4]],
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Vec<CornerLink>, String> {
    let surface_count = corners.len();
    if surface_count > MAX_DISPLACEMENTS
        || pairs.len() > surface_count * 4
        || pairs.iter().any(|pair| {
            pair.first >= surface_count
                || pair.second >= surface_count
                || pair.first == pair.second
                || pair.first_edge >= 4
                || pair.second_edge >= 4
        })
    {
        return Err("Terrain corner records require bounded surfaces and valid edge pairs".into());
    }
    consume(surface_count * 4 + pairs.len())?;
    let edge_pairs = pairs
        .iter()
        .map(|pair| (pair.first.min(pair.second), pair.first.max(pair.second)))
        .collect::<BTreeSet<_>>();
    let mut directed = BTreeMap::new();
    for (surface, records) in corners.iter().enumerate() {
        for (corner, record) in records.iter().enumerate() {
            let count = usize::from(record.count);
            if count > record.neighbors.len() {
                return Err("Terrain corner record exceeds its native neighbor capacity".into());
            }
            for &other in &record.neighbors[..count] {
                consume(1)?;
                let other = usize::from(other);
                if other >= surface_count || other == surface {
                    return Err("Terrain corner record has an invalid neighbor index".into());
                }
                if edge_pairs.contains(&(surface.min(other), surface.max(other))) {
                    return Err(
                        "Terrain corner-only records cannot replace an edge relation".into(),
                    );
                }
                if directed.insert((surface, other), corner).is_some() {
                    return Err(
                        "Terrain corner records must identify one unique reciprocal corner".into(),
                    );
                }
            }
        }
    }
    for &(first, second) in directed.keys() {
        consume(1)?;
        if !directed.contains_key(&(second, first)) {
            return Err("Terrain corner records lack a reciprocal neighbor".into());
        }
    }
    consume(directed.len())?;
    let mut result = Vec::with_capacity(directed.len() / 2);
    for (&(first, second), &first_corner) in &directed {
        if first < second {
            result.push(CornerLink {
                first,
                first_corner,
                second,
                second_corner: directed[&(second, first)],
            });
        }
    }
    Ok(result)
}

pub(super) fn corner_vertex(grid: &SurfaceGrid<'_>, corner: usize) -> usize {
    let size = (1usize << grid.power) + 1;
    [0, (size - 1) * size, size * size - 1, size - 1][corner]
}

fn base_bounds(grid: &SurfaceGrid<'_>) -> Result<[Vec3; 2], String> {
    let points = grid.corners.map(|point| point.as_vec3());
    if points.iter().any(|point| !point.is_finite()) {
        return Err("Terrain corner BASE coordinates must fit finite native storage".into());
    }
    Ok(native_bounds(&points))
}

pub(super) fn validate_contacts<F: FnMut(usize) -> Result<(), String>>(
    grids: &[SurfaceGrid<'_>],
    links: &[CornerLink],
    budget: &mut Budget<'_, F>,
) -> Result<(), String> {
    for link in links {
        budget.consume(10)?;
        let first = &grids[link.first];
        let second = &grids[link.second];
        if !native_bounds_touch(base_bounds(first)?, base_bounds(second)?) {
            return Err("Terrain corner records no longer share native BASE bounds".into());
        }
        let a = first.positions[corner_vertex(first, link.first_corner)];
        let b = second.positions[corner_vertex(second, link.second_corner)];
        if !native_corner_contact(a, b)? {
            return Err(
                "Terrain corner records no longer match their native displaced contact".into(),
            );
        }
    }
    Ok(())
}

pub(super) struct Groups<'a> {
    pub(super) offsets: &'a [usize],
    pub(super) local: &'a [Vec<DVec3>],
    pub(super) angles: &'a [Option<f64>],
    pub(super) parents: &'a mut [usize],
}

impl Groups<'_> {
    pub(super) fn join<F: FnMut(usize) -> Result<(), String>>(
        &mut self,
        first: [usize; 2],
        second: [usize; 2],
        budget: &mut Budget<'_, F>,
    ) -> Result<(), String> {
        let limit = self.angles[first[0]]
            .unwrap_or(180.0)
            .min(self.angles[second[0]].unwrap_or(180.0));
        if limit == 0.0 {
            return Ok(());
        }
        if limit < 180.0 {
            let a = self.local[first[0]][first[1]].normalize_or_zero();
            let b = self.local[second[0]][second[1]].normalize_or_zero();
            if a == DVec3::ZERO || b == DVec3::ZERO || a.dot(b) + 1e-12 < limit.to_radians().cos() {
                return Ok(());
            }
        }
        let first = root(self.parents, self.offsets[first[0]] + first[1], budget)?;
        let second = root(self.parents, self.offsets[second[0]] + second[1], budget)?;
        self.parents[first.max(second)] = first.min(second);
        Ok(())
    }
}
