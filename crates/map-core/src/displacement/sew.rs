use std::collections::BTreeMap;

use glam::DVec3;
use source_formats::vmf::DispInfo;

use super::{
    edit::{TerrainGrid, inspect},
    neighbors::{SurfaceGrid, common_span_indices, find_spans},
};
use crate::{
    Document, EditError,
    surface::{FaceId, side},
};

pub struct Plan {
    pub edits: Vec<(FaceId, DispInfo)>,
    pub pairs: usize,
    pub corner_joins: usize,
    pub vertices: usize,
    pub interpolated_vertices: usize,
}

const POSITION_EPSILON: f64 = 1e-9;

struct Average {
    point: DVec3,
    alpha: f64,
    count: usize,
    first_point: DVec3,
    first_alpha: f64,
    same_point: bool,
    same_alpha: bool,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn root(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    index
}

fn corners(grid: &TerrainGrid) -> [DVec3; 4] {
    let n = grid.mesh.size;
    let points = &grid.mesh.base_positions;
    [
        points[0],
        points[n * (n - 1)],
        points[n * n - 1],
        points[n - 1],
    ]
}

pub fn plan(document: &Document, faces: &[FaceId], blend: bool) -> Result<Plan, EditError> {
    let grids = inspect(document, faces)?;
    let views = grids
        .iter()
        .map(|grid| SurfaceGrid {
            power: grid.power,
            corners: corners(grid).map(|point| point.as_vec3().as_dvec3()),
            positions: &grid.mesh.base_positions,
        })
        .collect::<Vec<_>>();
    let neighbors = find_spans(&views, &mut |_| Ok(())).map_err(invalid)?;
    if neighbors.ambiguous > 0 {
        return Err(invalid(
            "Sewing needs unambiguous full or whole-to-half terrain edges",
        ));
    }
    let mut starts = Vec::with_capacity(grids.len());
    let mut count = 0;
    for grid in &grids {
        starts.push(count);
        count += grid.mesh.positions.len();
    }
    let mut parents = (0..count).collect::<Vec<_>>();
    for pair in &neighbors.pairs {
        for (a, b) in common_span_indices(
            (grids[pair.first].power, pair.first_edge, pair.first_span),
            (grids[pair.second].power, pair.second_edge, pair.second_span),
        )
        .map_err(invalid)?
        {
            let a = root(&mut parents, starts[pair.first] + a);
            let b = root(&mut parents, starts[pair.second] + b);
            let (low, high) = if a < b { (a, b) } else { (b, a) };
            parents[high] = low;
        }
    }
    let mut corner_groups = BTreeMap::<[u64; 3], Vec<(usize, usize)>>::new();
    for (index, grid) in grids.iter().enumerate() {
        let n = grid.mesh.size;
        for (corner, vertex) in [0, n * (n - 1), n * n - 1, n - 1].into_iter().enumerate() {
            corner_groups
                .entry(super::neighbors::key(views[index].corners[corner]))
                .or_default()
                .push((index, vertex));
        }
    }
    let mut corner_joins = 0;
    for group in corner_groups.values() {
        if group
            .iter()
            .map(|(grid, _)| grid)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != group.len()
        {
            return Err(invalid(
                "Terrain base corners collapse at Source float precision",
            ));
        }
        for pair in group.windows(2) {
            let a = root(&mut parents, starts[pair[0].0] + pair[0].1);
            let b = root(&mut parents, starts[pair[1].0] + pair[1].1);
            if a != b {
                let (low, high) = if a < b { (a, b) } else { (b, a) };
                parents[high] = low;
                corner_joins += 1;
            }
        }
    }
    if neighbors.pairs.is_empty() && corner_joins == 0 {
        return Err(invalid(
            "No matching full/half terrain borders or base corners were found",
        ));
    }
    let mut sums = BTreeMap::<usize, Average>::new();
    for (index, grid) in grids.iter().enumerate() {
        for (vertex, point) in grid.mesh.positions.iter().enumerate() {
            let root = root(&mut parents, starts[index] + vertex);
            let alpha = grid.mesh.alphas[vertex];
            let sum = sums.entry(root).or_insert(Average {
                point: DVec3::ZERO,
                alpha: 0.0,
                count: 0,
                first_point: *point,
                first_alpha: alpha,
                same_point: true,
                same_alpha: true,
            });
            sum.point += *point;
            sum.alpha += alpha;
            sum.count += 1;
            sum.same_point &= sum.first_point == *point;
            sum.same_alpha &= sum.first_alpha == alpha;
        }
    }
    let vertices = sums.values().filter(|sum| sum.count > 1).count();
    let mut points = grids
        .iter()
        .map(|grid| grid.mesh.positions.clone())
        .collect::<Vec<_>>();
    let mut blends = grids
        .iter()
        .map(|grid| grid.mesh.alphas.clone())
        .collect::<Vec<_>>();
    for (index, grid) in grids.iter().enumerate() {
        for vertex in 0..grid.mesh.positions.len() {
            let owner = root(&mut parents, starts[index] + vertex);
            let sum = &sums[&owner];
            if sum.count < 2 {
                continue;
            }
            points[index][vertex] = if sum.same_point {
                sum.first_point
            } else {
                sum.point / sum.count as f64
            };
            if blend {
                blends[index][vertex] = if sum.same_alpha {
                    sum.first_alpha
                } else {
                    sum.alpha / sum.count as f64
                };
            }
        }
    }
    let mut interpolated_vertices = 0;
    for pair in &neighbors.pairs {
        let a = pair
            .first_span
            .indices(grids[pair.first].power, pair.first_edge)
            .map_err(invalid)?;
        let b = pair
            .second_span
            .indices(grids[pair.second].power, pair.second_edge)
            .map_err(invalid)?;
        if a.len() == b.len() {
            continue;
        }
        let (fine_patch, fine, coarse_patch, mut coarse) = if a.len() > b.len() {
            (pair.first, a, pair.second, b)
        } else {
            (pair.second, b, pair.first, a)
        };
        coarse.reverse();
        let stride = (fine.len() - 1) / (coarse.len() - 1);
        for (step, &vertex) in fine.iter().enumerate() {
            if step % stride == 0 {
                continue;
            }
            let segment = step / stride;
            let t = (step % stride) as f64 / stride as f64;
            points[fine_patch][vertex] = points[coarse_patch][coarse[segment]]
                .lerp(points[coarse_patch][coarse[segment + 1]], t);
            if blend {
                let a = blends[coarse_patch][coarse[segment]];
                let b = blends[coarse_patch][coarse[segment + 1]];
                blends[fine_patch][vertex] = a + (b - a) * t;
            }
            interpolated_vertices += 1;
        }
    }
    let mut edits = Vec::with_capacity(grids.len());
    for (index, grid) in grids.iter().enumerate() {
        let mut info = side(document, grid.face)
            .unwrap()
            .displacement
            .clone()
            .unwrap();
        if !info.extra.is_empty() || grid.mesh.offset_normals_need_remapping(&info) {
            return Err(invalid(
                "Opaque terrain blocks or offset-normal caches need remapping before sewing",
            ));
        }
        grid.mesh.validate_edit_bounds()?;
        let mut offsets = info
            .offsets
            .clone()
            .unwrap_or_else(|| vec![vec![DVec3::ZERO; grid.mesh.size]; grid.mesh.size]);
        let mut alphas = info
            .alphas
            .clone()
            .unwrap_or_else(|| vec![vec![0.0; grid.mesh.size]; grid.mesh.size]);
        let mut moved = false;
        let mut painted = false;
        for vertex in 0..grid.mesh.positions.len() {
            let row = vertex / grid.mesh.size;
            let col = vertex % grid.mesh.size;
            let mut change = points[index][vertex] - grid.mesh.positions[vertex];
            if change.length_squared() <= POSITION_EPSILON * POSITION_EPSILON {
                change = DVec3::ZERO;
            }
            let updated = offsets[row][col] + change;
            if !updated.is_finite() {
                return Err(invalid("Sewn terrain offsets must be finite"));
            }
            offsets[row][col] = updated;
            moved |= change != DVec3::ZERO;
            if blend {
                let value = blends[index][vertex];
                painted |= value != alphas[row][col];
                alphas[row][col] = value;
            }
        }
        if moved {
            info.offsets = Some(offsets);
        }
        if painted {
            info.alphas = Some(alphas);
        }
        edits.push((grid.face, info));
    }
    Ok(Plan {
        edits,
        pairs: neighbors.pairs.len(),
        corner_joins,
        vertices,
        interpolated_vertices,
    })
}
