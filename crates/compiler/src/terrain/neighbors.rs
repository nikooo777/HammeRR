use map_core::displacement::neighbors::{self, Span, SurfaceGrid};
use source_formats::bsp::{DisplacementCorner, DisplacementNeighbor};
use std::collections::{BTreeMap, BTreeSet};

use super::{NeighborStats, Terrain};
use crate::emit::stored_vertex;
use crate::{Control, Diagnostic, Report};

const MAX_WORK: usize = 2_000_000;

pub(super) fn consume(work: &mut usize, amount: usize, control: &Control) -> Result<(), String> {
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
) -> Result<NeighborStats, String> {
    let mut work = 0;
    let mut positions = Vec::with_capacity(terrains.len());
    for terrain in terrains.iter() {
        consume(&mut work, terrain.triangles.len(), control)?;
        positions.push(
            terrain
                .native_positions()?
                .into_iter()
                .map(|point| glam::DVec3::from_array(point.map(f64::from)))
                .collect::<Vec<_>>(),
        );
    }
    let grids = terrains
        .iter()
        .zip(&positions)
        .map(|(terrain, positions)| {
            let size = terrain.mesh.size;
            SurfaceGrid {
                power: terrain.info.power as u32,
                corners: [0, (size - 1) * size, size * size - 1, size - 1]
                    .map(|index| stored_vertex(terrain.mesh.base_positions[index])),
                positions,
            }
        })
        .collect::<Vec<_>>();
    let neighbors =
        neighbors::find_spans(&grids, &mut |amount| consume(&mut work, amount, control))?;
    let mut pending = vec![[[DisplacementNeighbor::default(); 2]; 4]; terrains.len()];
    for pair in &neighbors.pairs {
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
            if pending[index][edge][slot].index != u16::MAX {
                return Err("Terrain neighbor spans overlap an occupied native slot".into());
            }
            pending[index][edge][slot] = DisplacementNeighbor {
                index: other as u16,
                orientation: ((2 + other_edge + 4 - edge) % 4) as u8,
                span,
                neighbor_span: other_span.native(other_edge),
                ..Default::default()
            };
        }
    }
    let (corners, corner_pairs) = corner_neighbors(&grids, &pending, &mut work, control)?;
    let mixed_pairs = neighbors
        .pairs
        .iter()
        .filter(|pair| grids[pair.first].power != grids[pair.second].power)
        .count();
    let half_pairs = neighbors
        .pairs
        .iter()
        .filter(|pair| pair.first_span != Span::Full || pair.second_span != Span::Full)
        .count();
    let (lod, stats) = super::lod::prepare(terrains, &grids, &neighbors.pairs, &mut work, control)?;
    control.checkpoint()?;
    for (((terrain, edges), corners), lod) in terrains.iter_mut().zip(pending).zip(corners).zip(lod)
    {
        terrain.info.edges = edges;
        terrain.info.corners = corners;
        if let Some(lod) = lod {
            terrain.info.allowed_vertices = lod.mask;
            terrain.mesh.positions = lod.positions;
            terrain.vertices = lod.vertices;
            terrain.indices = lod.indices;
        }
    }
    if neighbors.ambiguous + neighbors.mismatched != 0 {
        report.diagnostics.push(Diagnostic::warning(format!("Terrain shared edges left unlinked: {} ambiguous, {} mismatched surfaces. Native full/half edges require matching common samples; other partial spans are not connected", neighbors.ambiguous, neighbors.mismatched), None, None));
    }
    if stats.snapped != 0 {
        report.diagnostics.push(Diagnostic::warning(format!("Terrain LOD snapped {} inactive vertices onto the compatible surface; editable terrain remains unchanged", stats.snapped), None, None));
    }
    Ok(NeighborStats {
        pairs: neighbors.pairs.len(),
        corner_pairs,
        mixed_pairs,
        half_pairs,
        disabled_vertices: stats.disabled,
        snapped_vertices: stats.snapped,
    })
}

fn corner_neighbors(
    grids: &[SurfaceGrid<'_>],
    edges: &[[[DisplacementNeighbor; 2]; 4]],
    work: &mut usize,
    control: &Control,
) -> Result<(Vec<[DisplacementCorner; 4]>, usize), String> {
    let mut groups = BTreeMap::<[u64; 3], Vec<(usize, usize)>>::new();
    for (index, grid) in grids.iter().enumerate() {
        for (corner, point) in grid.corners.iter().enumerate() {
            consume(work, 1, control)?;
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
                consume(work, 1, control)?;
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
    let mut pending = vec![[DisplacementCorner::default(); 4]; grids.len()];
    let edge_pairs = edges
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
    let mut pairs = 0;
    for ((a, b), (corner_a, corner_b, count)) in shared {
        consume(work, 1, control)?;
        if count != 1 || edge_pairs.contains(&(a, b)) || edge_pairs.contains(&(b, a)) {
            continue;
        }
        let point = |index: usize, corner: usize| {
            let grid = &grids[index];
            let size = (1usize << grid.power) + 1;
            grid.positions[[0, (size - 1) * size, size * size - 1, size - 1][corner]]
        };
        if point(a, corner_a).distance(point(b, corner_b)) > neighbors::EDGE_TOLERANCE {
            continue;
        }
        for (index, corner, other) in [(a, corner_a, b), (b, corner_b, a)] {
            let list = &mut pending[index][corner];
            if list.count as usize == list.neighbors.len() {
                return Err("Native terrain corners allow up to four non-edge neighbors".into());
            }
            list.neighbors[list.count as usize] = other as u16;
            list.count += 1;
        }
        pairs += 1;
    }
    Ok((pending, pairs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;

    #[test]
    fn corner_links_are_reciprocal_and_exclude_existing_edge_links() {
        let a = [DVec3::ZERO, DVec3::Y, DVec3::new(1.0, 1.0, 0.0), DVec3::X];
        let b = a.map(|point| point + DVec3::X + DVec3::Y);
        let positions = |corners: [DVec3; 4]| {
            (0..5)
                .flat_map(move |r| {
                    (0..5).map(move |c| {
                        corners[0]
                            .lerp(corners[1], r as f64 / 4.0)
                            .lerp(corners[3].lerp(corners[2], r as f64 / 4.0), c as f64 / 4.0)
                    })
                })
                .collect::<Vec<_>>()
        };
        let pa = positions(a);
        let pb = positions(b);
        let grids = [
            SurfaceGrid {
                power: 2,
                corners: a,
                positions: &pa,
            },
            SurfaceGrid {
                power: 2,
                corners: b,
                positions: &pb,
            },
        ];
        let mut edges = vec![[[DisplacementNeighbor::default(); 2]; 4]; 2];
        let (lists, pairs) = corner_neighbors(&grids, &edges, &mut 0, &Control::default()).unwrap();
        assert_eq!(pairs, 1);
        assert_eq!(lists[0][2].count, 1);
        assert_eq!(lists[0][2].neighbors[0], 1);
        assert_eq!(lists[1][0].neighbors[0], 0);
        edges[0][0][0].index = 1;
        assert_eq!(
            corner_neighbors(&grids, &edges, &mut 0, &Control::default())
                .unwrap()
                .1,
            0
        );
        let shifted = pb.iter().map(|point| *point + DVec3::Z).collect::<Vec<_>>();
        let mismatched = [
            SurfaceGrid {
                power: 2,
                corners: a,
                positions: &pa,
            },
            SurfaceGrid {
                power: 2,
                corners: b,
                positions: &shifted,
            },
        ];
        edges[0][0][0].index = u16::MAX;
        assert_eq!(
            corner_neighbors(&mismatched, &edges, &mut 0, &Control::default())
                .unwrap()
                .1,
            0
        );
    }
    #[test]
    fn corner_capacity_work_and_cancellation_reject_before_publication() {
        let positions = (1..=6)
            .map(|size| {
                (0..5)
                    .flat_map(move |r| {
                        (0..5).map(move |c| {
                            DVec3::new(
                                c as f64 * size as f64 / 4.0,
                                r as f64 * size as f64 / 4.0,
                                0.0,
                            )
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let grids = positions
            .iter()
            .enumerate()
            .map(|(index, points)| {
                let scale = (index + 1) as f64;
                SurfaceGrid {
                    power: 2,
                    corners: [
                        DVec3::ZERO,
                        DVec3::Y * scale,
                        (DVec3::X + DVec3::Y) * scale,
                        DVec3::X * scale,
                    ],
                    positions: points,
                }
            })
            .collect::<Vec<_>>();
        let edges = vec![[[DisplacementNeighbor::default(); 2]; 4]; grids.len()];
        assert!(
            corner_neighbors(&grids, &edges, &mut 0, &Control::default())
                .unwrap_err()
                .contains("four")
        );
        let mut exhausted = MAX_WORK;
        assert!(
            corner_neighbors(
                &grids[..2],
                &edges[..2],
                &mut exhausted,
                &Control::default()
            )
            .is_err()
        );
        let control = Control::default();
        control.cancel().unwrap();
        assert!(corner_neighbors(&grids[..2], &edges[..2], &mut 0, &control).is_err());
    }
}
