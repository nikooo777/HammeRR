use std::collections::BTreeMap;

use glam::DVec3;
use source_formats::bsp::{DisplacementCorner, DisplacementInfo, MAX_DISPLACEMENTS};

pub const EDGE_TOLERANCE: f64 = 0.02;
const MAX_TOPOLOGY_WORK: usize = 2_000_000;
const MAX_NORMAL_WORK: usize = 4_000_000;

type SharedCorners = BTreeMap<(usize, usize), (usize, usize, usize)>;

mod discovery;
mod native;
mod normals;
mod spans;

pub(crate) use discovery::{native_bounds, native_bounds_touch, native_corner_contact};
pub use native::{NativeConnections, native_connections, native_neighborhood};
pub(crate) use normals::corner_links;
pub use spans::find_spans;

pub struct SurfaceGrid<'a> {
    pub power: u32,
    pub corners: [DVec3; 4],
    pub positions: &'a [DVec3],
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NeighborPair {
    pub first: usize,
    pub first_edge: usize,
    pub second: usize,
    pub second_edge: usize,
    pub first_span: Span,
    pub second_span: Span,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Span {
    #[default]
    Full,
    FirstHalf,
    SecondHalf,
}

impl Span {
    pub fn native(self, edge: usize) -> u8 {
        match (self, edge >= 2) {
            (Self::Full, _) => 0,
            (Self::FirstHalf, false) | (Self::SecondHalf, true) => 1,
            _ => 2,
        }
    }

    pub fn from_native(span: u8, edge: usize) -> Result<Self, String> {
        match (span, edge >= 2) {
            (0, _) => Ok(Self::Full),
            (1, false) | (2, true) => Ok(Self::FirstHalf),
            (2, false) | (1, true) => Ok(Self::SecondHalf),
            _ => Err("Terrain neighbor span must be full or half".into()),
        }
    }

    pub fn indices(self, power: u32, edge: usize) -> Result<Vec<usize>, String> {
        if !(2..=4).contains(&power) {
            return Err("Terrain power must be 2, 3 or 4".into());
        }
        let indices = edge_indices((1 << power) + 1, edge)?;
        let middle = indices.len() / 2;
        Ok(match self {
            Self::Full => indices,
            Self::FirstHalf => indices[..=middle].to_vec(),
            Self::SecondHalf => indices[middle..].to_vec(),
        })
    }
}

#[derive(Default)]
pub struct Neighbors {
    pub pairs: Vec<NeighborPair>,
    pub ambiguous: usize,
    pub unequal_power: usize,
    pub mismatched: usize,
}

struct Budget<'a, F> {
    work: usize,
    limit: usize,
    consume: &'a mut F,
}

impl<F: FnMut(usize) -> Result<(), String>> Budget<'_, F> {
    fn consume(&mut self, amount: usize) -> Result<(), String> {
        (self.consume)(amount)?;
        self.work = self
            .work
            .checked_add(amount)
            .ok_or("Terrain surface work overflow")?;
        if self.work > self.limit {
            return Err("Terrain surfaces exceeded their bounded work limit".into());
        }
        Ok(())
    }
}

pub fn edge_indices(size: usize, side: usize) -> Result<Vec<usize>, String> {
    if ![5, 9, 17].contains(&size) {
        return Err("Terrain edge grid size must be 5, 9 or 17".into());
    }
    Ok(match side {
        0 => (0..size).map(|row| row * size).collect(),
        1 => (0..size).map(|col| (size - 1) * size + col).collect(),
        2 => (0..size).rev().map(|row| row * size + size - 1).collect(),
        3 => (0..size).rev().collect(),
        _ => return Err("Terrain edge must be between 0 and 3".into()),
    })
}

fn validate<F: FnMut(usize) -> Result<(), String>>(
    grids: &[SurfaceGrid<'_>],
    budget: &mut Budget<'_, F>,
) -> Result<(), String> {
    if grids.len() > MAX_DISPLACEMENTS {
        return Err("Terrain surface count exceeds the native displacement limit".into());
    }
    for grid in grids {
        if !(2..=4).contains(&grid.power) {
            return Err("Terrain power must be 2, 3 or 4".into());
        }
        let size = (1 << grid.power) + 1;
        if grid.positions.len() != size * size {
            return Err("Terrain surface grid does not match its power".into());
        }
        budget.consume(grid.positions.len() + 1)?;
        if grid
            .positions
            .iter()
            .chain(&grid.corners)
            .any(|point| !point.is_finite())
        {
            return Err("Terrain surface positions must be finite".into());
        }
    }
    Ok(())
}

pub(super) fn key(point: DVec3) -> [u64; 3] {
    point
        .to_array()
        .map(|value| if value == 0.0 { 0 } else { value.to_bits() })
}

pub fn find(
    grids: &[SurfaceGrid<'_>],
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Neighbors, String> {
    find_edges(grids, false, consume)
}

pub fn find_mixed(
    grids: &[SurfaceGrid<'_>],
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Neighbors, String> {
    find_edges(grids, true, consume)
}

fn find_edges(
    grids: &[SurfaceGrid<'_>],
    mixed: bool,
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Neighbors, String> {
    let mut budget = Budget {
        work: 0,
        limit: MAX_TOPOLOGY_WORK,
        consume,
    };
    validate(grids, &mut budget)?;
    let mut groups: BTreeMap<[[u64; 3]; 2], Vec<(usize, usize)>> = BTreeMap::new();
    for (index, grid) in grids.iter().enumerate() {
        for edge in 0..4 {
            budget.consume(1)?;
            let a = key(grid.corners[edge]);
            let b = key(grid.corners[(edge + 1) % 4]);
            groups
                .entry(if a < b { [a, b] } else { [b, a] })
                .or_default()
                .push((index, edge));
        }
    }
    let mut neighbors = Neighbors::default();
    for group in groups.values() {
        budget.consume(1)?;
        if group.len() < 2 {
            continue;
        }
        let &[(first, first_edge), (second, second_edge)] = group.as_slice() else {
            neighbors.ambiguous += 1;
            continue;
        };
        let a = &grids[first];
        let b = &grids[second];
        if first == second
            || key(a.corners[first_edge]) != key(b.corners[(second_edge + 1) % 4])
            || key(a.corners[(first_edge + 1) % 4]) != key(b.corners[second_edge])
        {
            neighbors.ambiguous += 1;
            continue;
        }
        if a.power != b.power && !mixed {
            neighbors.unequal_power += 1;
            continue;
        }
        let mut matches = true;
        for (i, j) in common_edge_indices(a.power, first_edge, b.power, second_edge)? {
            budget.consume(1)?;
            if a.positions[i].distance(b.positions[j]) > EDGE_TOLERANCE {
                matches = false;
                break;
            }
        }
        if matches {
            neighbors.pairs.push(NeighborPair {
                first,
                first_edge,
                second,
                second_edge,
                ..Default::default()
            });
        } else {
            neighbors.mismatched += 1;
        }
    }
    Ok(neighbors)
}

pub fn common_edge_indices(
    first_power: u32,
    first_edge: usize,
    second_power: u32,
    second_edge: usize,
) -> Result<Vec<(usize, usize)>, String> {
    common_span_indices(
        (first_power, first_edge, Span::Full),
        (second_power, second_edge, Span::Full),
    )
}

pub fn common_span_indices(
    first: (u32, usize, Span),
    second: (u32, usize, Span),
) -> Result<Vec<(usize, usize)>, String> {
    let a = first.2.indices(first.0, first.1)?;
    let b = second.2.indices(second.0, second.1)?;
    let steps = (a.len() - 1).min(b.len() - 1);
    Ok((0..=steps)
        .map(|i| {
            (
                a[i * (a.len() - 1) / steps],
                b[b.len() - 1 - i * (b.len() - 1) / steps],
            )
        })
        .collect())
}

fn root<F: FnMut(usize) -> Result<(), String>>(
    parents: &mut [usize],
    mut index: usize,
    budget: &mut Budget<'_, F>,
) -> Result<usize, String> {
    while parents[index] != index {
        budget.consume(1)?;
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    Ok(index)
}

/// Additional continuity scratch for one patch in an approved native neighbor catalog.
pub fn vertex_normal_scratch_bytes(power: u32) -> Result<usize, String> {
    normals::continuity::scratch_bytes(power)
}

/// Unequal-density edges can return non-unit affine fields; normalize after interpolation.
pub fn vertex_normals(
    grids: &[SurfaceGrid<'_>],
    pairs: &[NeighborPair],
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Vec<Vec<[f64; 3]>>, String> {
    vertex_normals_with_smoothing(grids, pairs, &vec![None; grids.len()], consume)
}

pub fn vertex_normals_with_smoothing(
    grids: &[SurfaceGrid<'_>],
    pairs: &[NeighborPair],
    angles: &[Option<f64>],
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Vec<Vec<[f64; 3]>>, String> {
    normal_fields(grids, pairs, None, angles, consume)
}

pub fn vertex_normals_with_corners(
    grids: &[SurfaceGrid<'_>],
    pairs: &[NeighborPair],
    corners: &[[DisplacementCorner; 4]],
    angles: &[Option<f64>],
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Vec<Vec<[f64; 3]>>, String> {
    normal_fields(grids, pairs, Some(corners), angles, consume)
}

fn normal_fields(
    grids: &[SurfaceGrid<'_>],
    pairs: &[NeighborPair],
    corners: Option<&[[DisplacementCorner; 4]]>,
    angles: &[Option<f64>],
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Vec<Vec<[f64; 3]>>, String> {
    let mut budget = Budget {
        work: 0,
        limit: MAX_NORMAL_WORK,
        consume,
    };
    validate(grids, &mut budget)?;
    if angles.len() != grids.len() {
        return Err("Terrain smoothing choices do not match the surface count".into());
    }
    for &angle in angles {
        super::smoothing::validate(angle)?;
    }
    if pairs.len() > grids.len() * 4 {
        return Err("Terrain normal pair count exceeds the bounded edge limit".into());
    }
    let corner_links = if let Some(corners) = corners {
        if corners.len() != grids.len() {
            return Err("Terrain normal corner records do not match the surface count".into());
        }
        let links = corner_links(pairs, corners, &mut |amount| budget.consume(amount))?;
        normals::validate_contacts(grids, &links, &mut budget)?;
        links
    } else {
        Vec::new()
    };
    let mut offsets = vec![0];
    for grid in grids {
        offsets.push(offsets.last().unwrap() + grid.positions.len());
    }
    let count = *offsets.last().unwrap();
    let mut local = Vec::with_capacity(grids.len());
    for grid in grids {
        budget.consume(2 * (1 << grid.power) * (1 << grid.power))?;
        let positions = grid
            .positions
            .iter()
            .map(|point| point.to_array())
            .collect::<Vec<_>>();
        let sums = DisplacementInfo {
            power: grid.power as i32,
            ..Default::default()
        }
        .vertex_normal_sums(&positions)
        .map_err(|error| error.to_string())?;
        local.push(sums.into_iter().map(DVec3::from_array).collect::<Vec<_>>());
    }
    let mut parents = (0..count).collect::<Vec<_>>();
    let mut groups = normals::Groups {
        offsets: &offsets,
        local: &local,
        angles,
        parents: &mut parents,
    };
    for pair in pairs {
        budget.consume(1)?;
        let a = grids
            .get(pair.first)
            .ok_or("Terrain normal neighbor index is invalid")?;
        let b = grids
            .get(pair.second)
            .ok_or("Terrain normal neighbor index is invalid")?;
        if pair.first == pair.second {
            return Err("Terrain normals require distinct neighbors".into());
        }
        for (i, j) in common_span_indices(
            (a.power, pair.first_edge, pair.first_span),
            (b.power, pair.second_edge, pair.second_span),
        )? {
            budget.consume(1)?;
            if a.positions[i].distance(b.positions[j]) > EDGE_TOLERANCE {
                return Err("Terrain normal neighbor surfaces do not match".into());
            }
            groups.join([pair.first, i], [pair.second, j], &mut budget)?;
        }
    }
    for link in corner_links {
        budget.consume(1)?;
        groups.join(
            [
                link.first,
                normals::corner_vertex(&grids[link.first], link.first_corner),
            ],
            [
                link.second,
                normals::corner_vertex(&grids[link.second], link.second_corner),
            ],
            &mut budget,
        )?;
    }
    let continuity =
        normals::continuity::Constraints::prepare(grids, pairs, &mut groups, &mut budget)?;
    let mut totals = vec![DVec3::ZERO; count];
    for (patch, sums) in local.into_iter().enumerate() {
        for (vertex, sum) in sums.into_iter().enumerate() {
            budget.consume(1)?;
            let owner = root(&mut parents, offsets[patch] + vertex, &mut budget)?;
            totals[owner] += sum;
            if !totals[owner].is_finite() {
                return Err("Terrain sewn normal exceeds finite arithmetic".into());
            }
        }
    }
    let constrained = continuity.apply(&mut totals, &mut budget)?;
    let mut result = Vec::with_capacity(grids.len());
    for (patch, grid) in grids.iter().enumerate() {
        let mut normals = Vec::with_capacity(grid.positions.len());
        for vertex in 0..grid.positions.len() {
            budget.consume(1)?;
            let owner = root(&mut parents, offsets[patch] + vertex, &mut budget)?;
            let normal = if constrained {
                totals[owner]
            } else {
                totals[owner].normalize_or_zero()
            };
            normals.push(normal.to_array());
        }
        result.push(normals);
    }
    Ok(result)
}
