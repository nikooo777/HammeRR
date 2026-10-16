use std::collections::{BTreeMap, VecDeque};

use super::neighbors::{self, NeighborPair, Span, SurfaceGrid};
use glam::DVec3;
use source_formats::bsp::DisplacementInfo;

const MAX_WORK: usize = 2_000_000;

const CHILDREN: [[i32; 2]; 4] = [[1, 1], [-1, 1], [-1, -1], [1, -1]];
const SIDES: [[i32; 2]; 4] = [[1, 0], [0, 1], [-1, 0], [0, -1]];

pub struct Patch {
    pub mask: [u32; 10],
    pub positions: Vec<DVec3>,
    pub active: Vec<bool>,
    pub indices: Vec<[usize; 3]>,
}

#[derive(Default)]
pub struct Stats {
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

pub fn prepare(
    grids: &[SurfaceGrid<'_>],
    pairs: &[NeighborPair],
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<(Vec<Option<Patch>>, Stats), String> {
    if grids.len() > source_formats::bsp::MAX_DISPLACEMENTS
        || pairs.len() > grids.len() * 4
        || grids.iter().any(|grid| {
            !(2..=4).contains(&grid.power)
                || grid.positions.len() != ((1usize << grid.power) + 1).pow(2)
                || grid.positions.iter().any(|point| !point.is_finite())
        })
        || pairs.iter().any(|pair| {
            pair.first >= grids.len()
                || pair.second >= grids.len()
                || pair.first == pair.second
                || pair.first_edge >= 4
                || pair.second_edge >= 4
        })
    {
        return Err("Terrain LOD needs bounded finite grids and valid neighbor pairs".into());
    }
    let mut work = 0usize;
    let mut consume = |amount| {
        consume(amount)?;
        work = work
            .checked_add(amount)
            .ok_or("Terrain LOD work overflow")?;
        if work > MAX_WORK {
            return Err("Terrain LOD exceeded its work budget".into());
        }
        Ok(())
    };
    if pairs.iter().all(|pair| {
        effective_power(grids[pair.first].power, pair.first_span)
            == effective_power(grids[pair.second].power, pair.second_span)
    }) {
        return Ok(((0..grids.len()).map(|_| None).collect(), Stats::default()));
    }
    let mut masks = vec![[u32::MAX; 10]; grids.len()];
    let mut shared = BTreeMap::<(usize, usize), Vec<(usize, usize)>>::new();
    let mut queue = VecDeque::new();
    for pair in pairs {
        let a = grids[pair.first].power;
        let b = grids[pair.second].power;
        for (i, j) in neighbors::common_span_indices(
            (a, pair.first_edge, pair.first_span),
            (b, pair.second_edge, pair.second_span),
        )? {
            consume(1)?;
            shared
                .entry((pair.first, i))
                .or_default()
                .push((pair.second, j));
            shared
                .entry((pair.second, j))
                .or_default()
                .push((pair.first, i));
        }
        let a_steps = effective_power(a, pair.first_span);
        let b_steps = effective_power(b, pair.second_span);
        if a_steps != b_steps {
            let (patch, edge, span, power, shift) = if a_steps > b_steps {
                (
                    pair.first,
                    pair.first_edge,
                    pair.first_span,
                    a,
                    a_steps - b_steps,
                )
            } else {
                (
                    pair.second,
                    pair.second_edge,
                    pair.second_span,
                    b,
                    b_steps - a_steps,
                )
            };
            let stride = 1 << shift;
            for (step, vertex) in span.indices(power, edge)?.into_iter().enumerate() {
                if step % stride != 0 {
                    queue.push_back((patch, vertex));
                }
            }
        }
    }
    let reverse = [dependencies(2), dependencies(3), dependencies(4)];
    let mut stats = Stats::default();
    while let Some((patch, vertex)) = queue.pop_front() {
        consume(1)?;
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
    let mut pending = Vec::with_capacity(grids.len());
    for (index, grid) in grids.iter().enumerate() {
        if masks[index] == [u32::MAX; 10] {
            pending.push(None);
            continue;
        }
        let info = DisplacementInfo {
            allowed_vertices: masks[index],
            power: grid.power as i32,
            ..Default::default()
        };
        let indices = info
            .allowed_triangle_indices()
            .map_err(|error| error.to_string())?;
        let mut touched = vec![false; grids[index].positions.len()];
        for triangle in &indices {
            consume(1)?;
            for &vertex in triangle {
                touched[vertex] = true;
            }
        }
        let mut positions = grids[index].positions.to_vec();
        let size = (1 << grid.power) + 1;
        let uv = |vertex: usize| glam::DVec2::new((vertex % size) as f64, (vertex / size) as f64);
        for (vertex, &touched) in touched.iter().enumerate() {
            if touched {
                continue;
            }
            let p = uv(vertex);
            let mut snapped = None;
            for &[a, b, c] in &indices {
                consume(1)?;
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
            active: touched,
            indices,
        }));
    }
    Ok((pending, stats))
}

fn effective_power(power: u32, span: Span) -> u32 {
    power - u32::from(span != Span::Full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_grids_pairs_and_cancelled_work_are_rejected() {
        let positions = vec![DVec3::ZERO; 25];
        let grids = [SurfaceGrid {
            power: 2,
            corners: [DVec3::ZERO; 4],
            positions: &positions,
        }];
        for pair in [
            NeighborPair::default(),
            NeighborPair {
                second: 1,
                ..Default::default()
            },
        ] {
            assert!(prepare(&grids, &[pair], &mut |_| Ok(())).is_err());
        }
        let bad = [SurfaceGrid {
            power: 5,
            corners: [DVec3::ZERO; 4],
            positions: &positions,
        }];
        assert!(prepare(&bad, &[], &mut |_| Ok(())).is_err());
        let fine = vec![DVec3::ZERO; 81];
        let grids = [
            SurfaceGrid {
                power: 2,
                corners: [DVec3::ZERO; 4],
                positions: &positions,
            },
            SurfaceGrid {
                power: 3,
                corners: [DVec3::ZERO; 4],
                positions: &fine,
            },
        ];
        let result = prepare(
            &grids,
            &[NeighborPair {
                second: 1,
                ..Default::default()
            }],
            &mut |_| Err("cancelled".into()),
        );
        assert!(matches!(result, Err(error) if error == "cancelled"));
    }

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
