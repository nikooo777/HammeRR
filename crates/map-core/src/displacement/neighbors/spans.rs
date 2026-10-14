use super::{
    Budget, EDGE_TOLERANCE, MAX_TOPOLOGY_WORK, NeighborPair, Neighbors, Span, SurfaceGrid,
    common_span_indices, key, validate,
};
use glam::DVec3;
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
struct Segment {
    surface: usize,
    edge: usize,
    span: Span,
    points: [DVec3; 2],
}

impl Segment {
    fn occupy(self, slots: &mut [[[usize; 2]; 4]]) {
        match self.span {
            Span::Full => {
                slots[self.surface][self.edge][0] += 1;
                slots[self.surface][self.edge][1] += 1;
            }
            Span::FirstHalf => slots[self.surface][self.edge][0] += 1,
            Span::SecondHalf => slots[self.surface][self.edge][1] += 1,
        }
    }

    fn ambiguous(self, slots: &[[[usize; 2]; 4]]) -> bool {
        let counts = slots[self.surface][self.edge];
        match self.span {
            Span::Full => counts.into_iter().any(|count| count > 1),
            Span::FirstHalf => counts[0] > 1,
            Span::SecondHalf => counts[1] > 1,
        }
    }
}

pub fn find_spans(
    grids: &[SurfaceGrid<'_>],
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Neighbors, String> {
    let mut budget = Budget {
        work: 0,
        limit: MAX_TOPOLOGY_WORK,
        consume,
    };
    validate(grids, &mut budget)?;
    let mut groups = BTreeMap::<[[u64; 3]; 2], Vec<Segment>>::new();
    for (surface, grid) in grids.iter().enumerate() {
        for edge in 0..4 {
            let a = grid.corners[edge];
            let b = grid.corners[(edge + 1) % 4];
            let middle = ((a + b) * 0.5).as_vec3().as_dvec3();
            if !middle.is_finite() {
                return Err("Terrain edge midpoint must fit finite native coordinates".into());
            }
            for (span, points) in [
                (Span::Full, [a, b]),
                (Span::FirstHalf, [a, middle]),
                (Span::SecondHalf, [middle, b]),
            ] {
                budget.consume(1)?;
                let [a, b] = points.map(key);
                groups
                    .entry(if a < b { [a, b] } else { [b, a] })
                    .or_default()
                    .push(Segment {
                        surface,
                        edge,
                        span,
                        points,
                    });
            }
        }
    }
    let mut result = Neighbors::default();
    let mut slots = vec![[[0; 2]; 4]; grids.len()];
    let mut candidates = Vec::new();
    for group in groups.values() {
        budget.consume(1)?;
        if group.len() < 2 || !group.iter().any(|segment| segment.span == Span::Full) {
            continue;
        }
        for &segment in group {
            budget.consume(1)?;
            segment.occupy(&mut slots);
        }
        let &[a, b] = group.as_slice() else {
            result.ambiguous += 1;
            continue;
        };
        if a.surface == b.surface
            || key(a.points[0]) != key(b.points[1])
            || key(a.points[1]) != key(b.points[0])
        {
            result.ambiguous += 1;
            continue;
        }
        candidates.push((a, b));
    }
    for (a, b) in candidates {
        budget.consume(1)?;
        if a.ambiguous(&slots) || b.ambiguous(&slots) {
            result.ambiguous += 1;
            continue;
        }
        let first = &grids[a.surface];
        let second = &grids[b.surface];
        let mut matching = true;
        for (i, j) in common_span_indices(
            (first.power, a.edge, a.span),
            (second.power, b.edge, b.span),
        )? {
            budget.consume(1)?;
            if first.positions[i].distance(second.positions[j]) > EDGE_TOLERANCE {
                matching = false;
                break;
            }
        }
        if matching {
            result.pairs.push(NeighborPair {
                first: a.surface,
                first_edge: a.edge,
                first_span: a.span,
                second: b.surface,
                second_edge: b.edge,
                second_span: b.span,
            });
        } else {
            result.mismatched += 1;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(
        power: u32,
        rotation: usize,
        low: [f64; 2],
        high: [f64; 2],
    ) -> ([DVec3; 4], Vec<DVec3>) {
        let ring = [
            [low[0], low[1]],
            [low[0], high[1]],
            [high[0], high[1]],
            [high[0], low[1]],
        ]
        .map(|[x, y]| DVec3::new(x, y, 16.0));
        let corners = std::array::from_fn(|i| ring[(rotation + i) % 4]);
        let size = (1usize << power) + 1;
        let positions = (0..size * size)
            .map(|index| {
                let row = (index / size) as f64 / (size - 1) as f64;
                let col = (index % size) as f64 / (size - 1) as f64;
                let mut p = corners[0]
                    .lerp(corners[1], row)
                    .lerp(corners[3].lerp(corners[2], row), col);
                p.z += ((p.x - 128.0).powi(2) + (p.y - 128.0).powi(2)) / 512.0;
                p
            })
            .collect();
        (corners, positions)
    }

    #[test]
    fn whole_half_intervals_share_common_samples_at_every_power_and_rotation() {
        for power in 2..=4 {
            for neighbor_power in 2..=4 {
                for rotation in 0..4 {
                    for other_rotation in 0..4 {
                        let a = patch(power, rotation, [0., 0.], [128., 256.]);
                        let b = patch(neighbor_power, other_rotation, [128., 0.], [256., 128.]);
                        let grids = [
                            SurfaceGrid {
                                power,
                                corners: a.0,
                                positions: &a.1,
                            },
                            SurfaceGrid {
                                power: neighbor_power,
                                corners: b.0,
                                positions: &b.1,
                            },
                        ];
                        let result = find_spans(&grids, &mut |_| Ok(())).unwrap();
                        assert_eq!(result.pairs.len(), 1);
                        let pair = result.pairs[0];
                        assert_eq!(pair.second_span, Span::Full);
                        assert_ne!(pair.first_span, Span::Full);
                        let indices = common_span_indices(
                            (power, pair.first_edge, pair.first_span),
                            (neighbor_power, pair.second_edge, pair.second_span),
                        )
                        .unwrap();
                        assert_eq!(indices.len(), (1 << (power - 1).min(neighbor_power)) + 1);
                        for (i, j) in indices {
                            assert_eq!(a.1[i], b.1[j]);
                        }
                        for span in [Span::Full, Span::FirstHalf, Span::SecondHalf] {
                            assert_eq!(
                                Span::from_native(span.native(pair.first_edge), pair.first_edge)
                                    .unwrap(),
                                span
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn split_edges_and_junctions_reject_overlap_and_bound_cancellation() {
        let patches = [
            patch(2, 0, [0., 0.], [128., 256.]),
            patch(3, 1, [128., 0.], [256., 128.]),
            patch(4, 3, [128., 128.], [256., 256.]),
            patch(4, 0, [128., 0.], [256., 256.]),
        ];
        let powers = [2, 3, 4, 4];
        let grids = patches
            .iter()
            .enumerate()
            .map(|(i, (corners, positions))| SurfaceGrid {
                power: powers[i],
                corners: *corners,
                positions,
            })
            .collect::<Vec<_>>();
        let result = find_spans(&grids[..3], &mut |_| Ok(())).unwrap();
        assert_eq!(result.pairs.len(), 3);
        assert_eq!(
            result
                .pairs
                .iter()
                .filter(|pair| pair.first_span != Span::Full || pair.second_span != Span::Full)
                .count(),
            2
        );
        let result = find_spans(&grids, &mut |_| Ok(())).unwrap();
        assert!(result.ambiguous > 0);
        assert!(
            result
                .pairs
                .iter()
                .all(|pair| pair.first != 0 && pair.second != 0)
        );
        assert!(find_spans(&grids, &mut |_| Err("cancelled".into())).is_err());
        let enormous = patches[0]
            .1
            .iter()
            .map(|point| *point * 1e40)
            .collect::<Vec<_>>();
        let enormous_grid = [SurfaceGrid {
            power: 2,
            corners: patches[0].0.map(|point| point * 1e40),
            positions: &enormous,
        }];
        assert!(
            find_spans(&enormous_grid, &mut |_| Ok(()))
                .err()
                .unwrap()
                .contains("finite native")
        );
    }
}
