use std::collections::{BTreeMap, BTreeSet};

use glam::{DVec3, Vec3};

use super::spans::{Occupancy, matching_samples};
use super::{
    Budget, MAX_TOPOLOGY_WORK, NeighborPair, Neighbors, SharedCorners, Span, SurfaceGrid, key,
    validate,
};

const EDGE_ENDPOINT_TOLERANCE: f32 = 0.01;
const CORNER_ENDPOINT_TOLERANCE: f32 = 0.001;
const BASE_BOUNDS_PUFF: f32 = 0.1;

type PairKey = [usize; 6];
type Row = BTreeMap<i64, Vec<usize>>;
type Layer = BTreeMap<i64, Row>;

pub(super) struct Discovery {
    pub(super) neighbors: Neighbors,
    pub(super) edge_pairs: BTreeSet<(usize, usize)>,
}

struct Index {
    width: f64,
    buckets: BTreeMap<i64, Layer>,
    entries: usize,
}

impl Index {
    fn new<F: FnMut(usize) -> Result<(), String>>(
        points: &[Vec3],
        tolerance: f32,
        budget: &mut Budget<'_, F>,
    ) -> Result<Self, String> {
        budget.consume(points.len() * 3)?;
        let mut result = Self {
            width: 2.0 * f64::from(tolerance),
            buckets: BTreeMap::new(),
            entries: points.len(),
        };
        for (index, &point) in points.iter().enumerate() {
            let [x, y, z] = result.bucket(point);
            result
                .buckets
                .entry(x)
                .or_default()
                .entry(y)
                .or_default()
                .entry(z)
                .or_default()
                .push(index);
        }
        Ok(result)
    }

    fn bucket(&self, point: Vec3) -> [i64; 3] {
        point
            .to_array()
            .map(|value| (f64::from(value) / self.width).floor() as i64)
    }

    fn candidates<F: FnMut(usize) -> Result<(), String>>(
        &self,
        point: Vec3,
        budget: &mut Budget<'_, F>,
    ) -> Result<Vec<usize>, String> {
        if !point.is_finite() {
            return Err("Native terrain endpoint arithmetic must remain finite".into());
        }
        let [x, y, z] = self
            .bucket(point)
            .map(|value| value.saturating_sub(1)..=value.saturating_add(1));
        let mut result = Vec::new();
        budget.consume(1)?;
        for layer in self.buckets.range(x).map(|(_, layer)| layer) {
            budget.consume(1)?;
            for row in layer.range(y.clone()).map(|(_, row)| row) {
                budget.consume(1)?;
                for entries in row.range(z.clone()).map(|(_, entries)| entries) {
                    budget.consume(1 + entries.len())?;
                    if result
                        .len()
                        .checked_add(entries.len())
                        .is_none_or(|count| count > self.entries)
                    {
                        return Err(
                            "Native terrain endpoint query exceeds its bounded candidate limit"
                                .into(),
                        );
                    }
                    result.extend(entries);
                }
            }
        }
        Ok(result)
    }
}

fn stored(point: DVec3) -> Result<Vec3, String> {
    let result = point.as_vec3();
    if !result.is_finite() {
        return Err("Terrain endpoints must fit finite native storage".into());
    }
    Ok(result)
}

fn near(a: Vec3, b: Vec3, tolerance: f32) -> bool {
    a.to_array()
        .into_iter()
        .zip(b.to_array())
        .all(|(a, b)| (a - b).abs() <= tolerance)
}

pub(crate) fn native_corner_contact(a: DVec3, b: DVec3) -> Result<bool, String> {
    Ok(near(stored(a)?, stored(b)?, CORNER_ENDPOINT_TOLERANCE))
}

/// Points must be nonempty finite native coordinates.
pub(crate) fn native_bounds(points: &[Vec3]) -> [Vec3; 2] {
    let min = points
        .iter()
        .copied()
        .fold(Vec3::splat(f32::INFINITY), Vec3::min);
    let max = points
        .iter()
        .copied()
        .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
    [
        min - Vec3::splat(BASE_BOUNDS_PUFF),
        max + Vec3::splat(BASE_BOUNDS_PUFF),
    ]
}

pub(crate) fn native_bounds_touch(first: [Vec3; 2], second: [Vec3; 2]) -> bool {
    first[0].cmple(second[1]).all() && second[0].cmple(first[1]).all()
}

fn span_key(span: Span) -> usize {
    match span {
        Span::Full => 0,
        Span::FirstHalf => 1,
        Span::SecondHalf => 2,
    }
}

fn pair_key(pair: &NeighborPair) -> PairKey {
    let first = [pair.first, pair.first_edge, span_key(pair.first_span)];
    let second = [pair.second, pair.second_edge, span_key(pair.second_span)];
    if first < second {
        [
            first[0], first[1], first[2], second[0], second[1], second[2],
        ]
    } else {
        [
            second[0], second[1], second[2], first[0], first[1], first[2],
        ]
    }
}

fn canonical(mut pair: NeighborPair) -> NeighborPair {
    if pair.first > pair.second {
        std::mem::swap(&mut pair.first, &mut pair.second);
        std::mem::swap(&mut pair.first_edge, &mut pair.second_edge);
        std::mem::swap(&mut pair.first_span, &mut pair.second_span);
    }
    pair
}

fn segment_key(pair: &NeighborPair, points: &[[Vec3; 4]]) -> [[u64; 3]; 2] {
    let (surface, edge) = if pair.first_span == Span::Full {
        (pair.first, pair.first_edge)
    } else {
        (pair.second, pair.second_edge)
    };
    let a = key(points[surface][edge].as_dvec3());
    let b = key(points[surface][(edge + 1) % 4].as_dvec3());
    if a < b { [a, b] } else { [b, a] }
}

pub(super) fn find_native_spans(
    grids: &[SurfaceGrid<'_>],
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Discovery, String> {
    let mut budget = Budget {
        work: 0,
        limit: MAX_TOPOLOGY_WORK,
        consume,
    };
    validate(grids, &mut budget)?;
    budget.consume(grids.len() * 8)?;
    let points = grids
        .iter()
        .map(|grid| {
            let mut points = [Vec3::ZERO; 4];
            for (target, &point) in points.iter_mut().zip(&grid.corners) {
                *target = stored(point)?;
            }
            Ok(points)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let starts = points.iter().flatten().copied().collect::<Vec<_>>();
    let boxes = points
        .iter()
        .map(|points| native_bounds(points))
        .collect::<Vec<_>>();
    let index = Index::new(&starts, EDGE_ENDPOINT_TOLERANCE, &mut budget)?;
    let mut candidates = BTreeMap::<PairKey, (NeighborPair, u8)>::new();
    for (surface, corners) in points.iter().enumerate() {
        for edge in 0..4 {
            let a = corners[edge];
            let b = corners[(edge + 1) % 4];
            let middle = (a + b) * 0.5;
            let queries = [
                [b, a],
                [b, a * 2.0 - b],
                [b * 2.0 - a, a],
                [middle, a],
                [b, middle],
            ];
            if queries.iter().flatten().any(|point| !point.is_finite()) {
                return Err(format!(
                    "Native terrain endpoint arithmetic overflowed on surface {surface} edge {edge}"
                ));
            }
            let probes = [
                index.candidates(b, &mut budget)?,
                index.candidates(queries[2][0], &mut budget)?,
                index.candidates(middle, &mut budget)?,
            ];
            let mut matches = BTreeMap::<usize, [Vec<usize>; 5]>::new();
            for (stage, query) in queries.iter().enumerate() {
                let probe = match stage {
                    2 => 1,
                    3 => 2,
                    _ => 0,
                };
                for &candidate in &probes[probe] {
                    let other = candidate / 4;
                    let other_edge = candidate % 4;
                    if other == surface || !native_bounds_touch(boxes[surface], boxes[other]) {
                        continue;
                    }
                    budget.consume(1)?;
                    if near(query[0], points[other][other_edge], EDGE_ENDPOINT_TOLERANCE)
                        && near(
                            query[1],
                            points[other][(other_edge + 1) % 4],
                            EDGE_ENDPOINT_TOLERANCE,
                        )
                    {
                        matches.entry(other).or_default()[stage].push(other_edge);
                    }
                }
            }
            for (other, stages) in matches {
                let first = stages[..3]
                    .iter()
                    .position(|stage| !stage.is_empty())
                    .unwrap_or(3);
                for (stage, edges) in stages.iter().enumerate().skip(first) {
                    if first != 3 && stage != first {
                        break;
                    }
                    let spans = match stage {
                        0 => (Span::Full, Span::Full),
                        1 => (Span::Full, Span::FirstHalf),
                        2 => (Span::Full, Span::SecondHalf),
                        3 => (Span::FirstHalf, Span::Full),
                        _ => (Span::SecondHalf, Span::Full),
                    };
                    for &other_edge in edges {
                        budget.consume(1)?;
                        let pair = canonical(NeighborPair {
                            first: surface,
                            first_edge: edge,
                            first_span: spans.0,
                            second: other,
                            second_edge: other_edge,
                            second_span: spans.1,
                        });
                        let key = pair_key(&pair);
                        if candidates.len() >= grids.len() * 12 && !candidates.contains_key(&key) {
                            return Err(
                                "Native terrain endpoint candidates exceed their bounded limit"
                                    .into(),
                            );
                        }
                        candidates.entry(key).or_insert((pair, 0)).1 |=
                            if surface < other { 1 } else { 2 };
                    }
                }
            }
        }
    }
    let mut slots = Occupancy::new(grids.len());
    for (pair, directions) in candidates.values() {
        budget.consume(1)?;
        if *directions != 3 {
            return Err(format!(
                "Native terrain endpoint admission is asymmetric between surfaces {} edge {} and {} edge {}; reciprocal full/half discovery is required",
                pair.first, pair.first_edge, pair.second, pair.second_edge
            ));
        }
        slots.occupy(pair.first, pair.first_edge, pair.first_span);
        slots.occupy(pair.second, pair.second_edge, pair.second_span);
    }
    budget.consume(candidates.len())?;
    let edge_pairs = candidates
        .values()
        .map(|(pair, _)| (pair.first, pair.second))
        .collect();
    let mut result = Neighbors::default();
    for (pair, _) in candidates.into_values() {
        budget.consume(1)?;
        if slots.ambiguous(pair.first, pair.first_edge, pair.first_span)
            || slots.ambiguous(pair.second, pair.second_edge, pair.second_span)
        {
            result.ambiguous += 1;
        } else if matching_samples(grids, &pair, &mut budget)? {
            result.pairs.push(pair);
        } else {
            result.mismatched += 1;
        }
    }
    budget.consume(
        result.pairs.len() * (result.pairs.len().checked_ilog2().unwrap_or(0) as usize + 1),
    )?;
    result
        .pairs
        .sort_by_key(|pair| (segment_key(pair, &points), pair_key(pair)));
    Ok(Discovery {
        neighbors: result,
        edge_pairs,
    })
}

pub(super) fn shared_corners<F: FnMut(usize) -> Result<(), String>>(
    grids: &[SurfaceGrid<'_>],
    budget: &mut Budget<'_, F>,
) -> Result<SharedCorners, String> {
    budget.consume(grids.len() * 12)?;
    let boxes = grids
        .iter()
        .map(|grid| {
            let mut corners = [Vec3::ZERO; 4];
            for (corner, &point) in corners.iter_mut().zip(&grid.corners) {
                *corner = stored(point)?;
            }
            Ok(native_bounds(&corners))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let points = grids
        .iter()
        .flat_map(|grid| {
            let size = (1usize << grid.power) + 1;
            [0, (size - 1) * size, size * size - 1, size - 1]
                .map(|index| stored(grid.positions[index]))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let index = Index::new(&points, CORNER_ENDPOINT_TOLERANCE, budget)?;
    let mut shared = SharedCorners::new();
    for (first, &point) in points.iter().enumerate() {
        for second in index.candidates(point, budget)? {
            let a = first / 4;
            let b = second / 4;
            if a >= b || !native_bounds_touch(boxes[a], boxes[b]) {
                continue;
            }
            budget.consume(1)?;
            if native_corner_contact(point.as_dvec3(), points[second].as_dvec3())? {
                if shared.len() >= grids.len() * 16 && !shared.contains_key(&(a, b)) {
                    return Err(
                        "Native terrain corner candidates exceed their bounded limit".into(),
                    );
                }
                let entry = shared.entry((a, b)).or_insert((first % 4, second % 4, 0));
                entry.2 += 1;
            }
        }
    }
    Ok(shared)
}
