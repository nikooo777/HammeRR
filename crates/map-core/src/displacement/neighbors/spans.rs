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
    fn occupy(self, slots: &mut Occupancy) {
        slots.occupy(self.surface, self.edge, self.span);
    }

    fn ambiguous(self, slots: &Occupancy) -> bool {
        slots.ambiguous(self.surface, self.edge, self.span)
    }
}

pub(super) struct Occupancy(Vec<[[usize; 2]; 4]>);

impl Occupancy {
    pub(super) fn new(surfaces: usize) -> Self {
        Self(vec![[[0; 2]; 4]; surfaces])
    }

    pub(super) fn occupy(&mut self, surface: usize, edge: usize, span: Span) {
        match span {
            Span::Full => {
                self.0[surface][edge][0] += 1;
                self.0[surface][edge][1] += 1;
            }
            Span::FirstHalf => self.0[surface][edge][0] += 1,
            Span::SecondHalf => self.0[surface][edge][1] += 1,
        }
    }

    pub(super) fn ambiguous(&self, surface: usize, edge: usize, span: Span) -> bool {
        let counts = self.0[surface][edge];
        match span {
            Span::Full => counts.into_iter().any(|count| count > 1),
            Span::FirstHalf => counts[0] > 1,
            Span::SecondHalf => counts[1] > 1,
        }
    }
}

pub(super) fn matching_samples<F: FnMut(usize) -> Result<(), String>>(
    grids: &[SurfaceGrid<'_>],
    pair: &NeighborPair,
    budget: &mut Budget<'_, F>,
) -> Result<bool, String> {
    let first = &grids[pair.first];
    let second = &grids[pair.second];
    for (i, j) in common_span_indices(
        (first.power, pair.first_edge, pair.first_span),
        (second.power, pair.second_edge, pair.second_span),
    )? {
        budget.consume(1)?;
        if first.positions[i].distance(second.positions[j]) > EDGE_TOLERANCE {
            return Ok(false);
        }
    }
    Ok(true)
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
    let mut slots = Occupancy::new(grids.len());
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
        let pair = NeighborPair {
            first: a.surface,
            first_edge: a.edge,
            first_span: a.span,
            second: b.surface,
            second_edge: b.edge,
            second_span: b.span,
        };
        if matching_samples(grids, &pair, &mut budget)? {
            result.pairs.push(pair);
        } else {
            result.mismatched += 1;
        }
    }
    Ok(result)
}
