use std::collections::{BTreeMap, VecDeque};

use glam::DVec3;
use map_core::displacement::neighbors::{self, NeighborPair, SurfaceGrid};
use source_formats::bsp::{DisplacementInfo, DisplacementVertex};

use super::{Terrain, neighbors::consume};
use crate::Control;

const CHILDREN: [[i32; 2]; 4] = [[1, 1], [-1, 1], [-1, -1], [1, -1]];
const SIDES: [[i32; 2]; 4] = [[1, 0], [0, 1], [-1, 0], [0, -1]];

pub(super) struct Patch {
    pub mask: [u32; 10],
    pub positions: Vec<DVec3>,
    pub vertices: Vec<DisplacementVertex>,
    pub indices: Vec<[usize; 3]>,
}

#[derive(Default)]
pub(super) struct Stats {
    pub disabled: usize,
    pub snapped: usize,
}

fn dependencies(power: u32) -> Vec<Vec<usize>> {
    let size = (1 << power) + 1;
    let mut reverse = vec![Vec::new(); size * size];
    let center = (size / 2) as i32;
    dependency_node(
        [center; 2],
        size / 2,
        size,
        [[(size - 1) as i32; 2], [0; 2]],
        &mut reverse,
    );
    reverse
}

fn dependency_node(
    center: [i32; 2],
    step: usize,
    size: usize,
    parents: [[i32; 2]; 2],
    reverse: &mut [Vec<usize>],
) {
    let index = |p: [i32; 2]| p[1] as usize * size + p[0] as usize;
    let point = |offset: [i32; 2], scale: usize| {
        std::array::from_fn(|axis| center[axis] + offset[axis] * scale as i32)
    };
    for parent in parents {
        reverse[index(parent)].push(index(center));
    }
    for offset in SIDES {
        reverse[index(center)].push(index(point(offset, step)));
    }
    if step > 1 {
        for (child, offset) in CHILDREN.into_iter().enumerate() {
            let parents = [
                point(SIDES[child], step),
                point(SIDES[(child + 1) % 4], step),
            ];
            dependency_node(point(offset, step / 2), step / 2, size, parents, reverse);
        }
    }
}

pub(super) fn prepare(
    terrains: &[Terrain],
    grids: &[SurfaceGrid<'_>],
    pairs: &[NeighborPair],
    work: &mut usize,
    control: &Control,
) -> Result<(Vec<Option<Patch>>, Stats), String> {
    if pairs
        .iter()
        .all(|pair| grids[pair.first].power == grids[pair.second].power)
    {
        return Ok((
            (0..terrains.len()).map(|_| None).collect(),
            Stats::default(),
        ));
    }
    let mut masks = vec![[u32::MAX; 10]; terrains.len()];
    let mut shared = BTreeMap::<(usize, usize), Vec<(usize, usize)>>::new();
    let mut queue = VecDeque::new();
    for pair in pairs {
        let a = grids[pair.first].power;
        let b = grids[pair.second].power;
        for (i, j) in neighbors::common_edge_indices(a, pair.first_edge, b, pair.second_edge)? {
            consume(work, 1, control)?;
            shared
                .entry((pair.first, i))
                .or_default()
                .push((pair.second, j));
            shared
                .entry((pair.second, j))
                .or_default()
                .push((pair.first, i));
        }
        if a != b {
            let (patch, edge, power, other) = if a > b {
                (pair.first, pair.first_edge, a, b)
            } else {
                (pair.second, pair.second_edge, b, a)
            };
            let stride = 1 << (power - other);
            for (step, vertex) in neighbors::edge_indices((1 << power) + 1, edge)?
                .into_iter()
                .enumerate()
            {
                if step % stride != 0 {
                    queue.push_back((patch, vertex));
                }
            }
        }
    }
    let reverse = [dependencies(2), dependencies(3), dependencies(4)];
    let mut stats = Stats::default();
    while let Some((patch, vertex)) = queue.pop_front() {
        consume(work, 1, control)?;
        let bit = 1 << (vertex % 32);
        if masks[patch][vertex / 32] & bit == 0 {
            continue;
        }
        masks[patch][vertex / 32] &= !bit;
        stats.disabled += 1;
        for &dependent in &reverse[(grids[patch].power - 2) as usize][vertex] {
            queue.push_back((patch, dependent));
        }
        if let Some(neighbors) = shared.get(&(patch, vertex)) {
            queue.extend(neighbors);
        }
    }
    let mut pending = Vec::with_capacity(terrains.len());
    for (index, terrain) in terrains.iter().enumerate() {
        if masks[index] == [u32::MAX; 10] {
            pending.push(None);
            continue;
        }
        let info = DisplacementInfo {
            allowed_vertices: masks[index],
            ..terrain.info
        };
        let indices = info
            .allowed_triangle_indices()
            .map_err(|error| error.to_string())?;
        let mut touched = vec![false; grids[index].positions.len()];
        for triangle in &indices {
            consume(work, 1, control)?;
            for &vertex in triangle {
                touched[vertex] = true;
            }
        }
        let mut positions = grids[index].positions.to_vec();
        let size = terrain.mesh.size;
        let uv = |vertex: usize| glam::DVec2::new((vertex % size) as f64, (vertex / size) as f64);
        for (vertex, &touched) in touched.iter().enumerate() {
            if touched {
                continue;
            }
            let p = uv(vertex);
            let mut snapped = None;
            for &[a, b, c] in &indices {
                consume(work, 1, control)?;
                let [u, v] = [uv(b) - uv(a), uv(c) - uv(a)];
                let d = p - uv(a);
                let determinant = u.perp_dot(v);
                let s = d.perp_dot(v) / determinant;
                let t = u.perp_dot(d) / determinant;
                if s >= 0.0 && t >= 0.0 && s + t <= 1.0 {
                    snapped = Some(
                        grids[index].positions[a] * (1.0 - s - t)
                            + grids[index].positions[b] * s
                            + grids[index].positions[c] * t,
                    );
                    break;
                }
            }
            let point =
                snapped.ok_or("Terrain LOD tessellation does not cover an inactive vertex")?;
            if point.distance(positions[vertex]) > 1e-6 {
                stats.snapped += 1;
            }
            positions[vertex] = point;
        }
        let mut vertices = terrain.vertices.clone();
        for (vertex, &touched) in touched.iter().enumerate() {
            if touched {
                continue;
            }
            let delta = (positions[vertex] - grids[index].positions[vertex]).as_vec3();
            let old = vertices[vertex];
            vertices[vertex].direction =
                (glam::Vec3::from_array(old.direction) * old.distance + delta).to_array();
            vertices[vertex].distance = 1.0;
        }
        if indices.iter().any(|&[a, b, c]| {
            (positions[b] - positions[a])
                .cross(positions[c] - positions[a])
                .length_squared()
                < 1e-12
        }) {
            return Err("Terrain LOD produces collapsed surface triangles".into());
        }
        pending.push(Some(Patch {
            mask: masks[index],
            positions,
            vertices,
            indices,
        }));
    }
    Ok((pending, stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_dependencies_match_native_lod_hierarchy() {
        let reverse = dependencies(4);
        let size = 17;
        assert!(reverse[2 * size].contains(&(size + 1)));
        assert!(reverse[size + 1].contains(&size));
        assert!(!reverse[2 * size].contains(&(8 * size + 8)));
        let mut disabled = vec![false; size * size];
        let mut queue = VecDeque::new();
        for y in 0..size {
            if y % 4 != 0 {
                queue.push_back(y * size);
            }
        }
        while let Some(vertex) = queue.pop_front() {
            if disabled[vertex] {
                continue;
            }
            disabled[vertex] = true;
            queue.extend(&reverse[vertex]);
        }
        assert_eq!(disabled.iter().filter(|&&value| value).count(), 37);
        let mut info = DisplacementInfo {
            power: 4,
            ..Default::default()
        };
        for (vertex, &disabled) in disabled.iter().enumerate() {
            if disabled {
                info.allowed_vertices[vertex / 32] &= !(1 << (vertex % 32));
            }
        }
        let triangles = info.allowed_triangle_indices().unwrap();
        assert_eq!(triangles.len(), 452);
        let uv = |i: usize| glam::DVec2::new((i % size) as f64, (i / size) as f64);
        let area = triangles
            .iter()
            .map(|&[a, b, c]| (uv(b) - uv(a)).perp_dot(uv(c) - uv(a)))
            .sum::<f64>();
        assert_eq!(area, -512.0);
    }
}
