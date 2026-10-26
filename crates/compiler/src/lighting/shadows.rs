use glam::DVec3;
use std::ops::{ControlFlow, Range};

use super::Occluder;

const MAX_OCCLUDERS: usize = 65_536;
const LEAF_SIZE: usize = 4;
const STACK_SIZE: usize = usize::BITS as usize;
pub(super) const TRIANGLE_EPSILON: f64 = 1e-9;
pub(super) const CONTACT_EPSILON: f64 = 1e-6;

pub(super) enum Visit {
    Continue(f64),
    Stop,
}

#[derive(Clone, Copy)]
enum Query {
    Segment { start: DVec3, delta: DVec3 },
    Bounds { mins: DVec3, maxs: DVec3 },
}

impl Query {
    fn bounds(mins: DVec3, maxs: DVec3) -> Result<Self, String> {
        if !mins.is_finite() || !maxs.is_finite() || !mins.cmple(maxs).all() {
            return Err("Shadow query bounds must be finite and ordered".into());
        }
        Ok(Self::Bounds { mins, maxs })
    }

    fn overlaps(self, mins: DVec3, maxs: DVec3, limit: f64) -> bool {
        match self {
            Self::Segment { start, delta } => ray_box(start, delta, mins, maxs, limit),
            Self::Bounds {
                mins: low,
                maxs: high,
            } => low.cmple(maxs).all() && mins.cmple(high).all(),
        }
    }
}

fn ray_box(start: DVec3, delta: DVec3, mins: DVec3, maxs: DVec3, limit: f64) -> bool {
    let mut near: f64 = 0.0;
    let mut far = limit + TRIANGLE_EPSILON;
    for axis in 0..3 {
        if delta[axis] == 0.0 {
            if start[axis] < mins[axis] || start[axis] > maxs[axis] {
                return false;
            }
        } else {
            let a = (mins[axis] - start[axis]) / delta[axis];
            let b = (maxs[axis] - start[axis]) / delta[axis];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
            if near > far {
                return false;
            }
        }
    }
    true
}

pub(super) trait ShadowObject {
    const MAX_OBJECTS: usize;
    fn bounds(&self) -> (DVec3, DVec3);
    fn work(&self) -> usize;
    fn intersects(&self, start: DVec3, end: DVec3) -> Result<bool, String>;
    fn fractional(&self) -> bool {
        false
    }
}

impl ShadowObject for Occluder {
    const MAX_OBJECTS: usize = MAX_OCCLUDERS;
    fn bounds(&self) -> (DVec3, DVec3) {
        (self.mins, self.maxs)
    }
    fn work(&self) -> usize {
        self.plane_count()
            + 1
            + self.optical.as_ref().map_or(0, |optical| {
                let inverse = optical
                    .pose
                    .map_or(0, |pose| if pose.rotation.is_some() { 18 } else { 3 });
                optical.faces.len() * (32 + inverse)
            })
    }
    fn intersects(&self, start: DVec3, end: DVec3) -> Result<bool, String> {
        self.intersects(start, end)
    }
}

#[derive(Debug, Clone)]
pub(super) struct Triangle {
    points: [DVec3; 3],
    mins: DVec3,
    maxs: DVec3,
}

impl Triangle {
    pub(super) fn point(&self, weights: [f64; 3]) -> DVec3 {
        self.points
            .into_iter()
            .zip(weights)
            .map(|(point, weight)| point * weight)
            .sum()
    }

    pub(super) fn new(points: [DVec3; 3]) -> Result<Self, String> {
        let [a, b, c] = points;
        let normal = (b - a).cross(c - a);
        if points.iter().any(|point| !point.is_finite())
            || !normal.is_finite()
            || !normal.length_squared().is_finite()
            || normal.length_squared() == 0.0
        {
            return Err("Shadow triangle must be finite and noncollapsed".into());
        }
        Ok(Self {
            points,
            mins: a.min(b).min(c) - DVec3::splat(0.0001),
            maxs: a.max(b).max(c) + DVec3::splat(0.0001),
        })
    }

    pub(super) fn point_contact(&self, point: DVec3) -> Result<bool, String> {
        let [a, b, c] = self.points;
        let normal = (b - a).cross(c - a).normalize();
        let distance = (point - a).dot(normal);
        if !point.is_finite() || !normal.is_finite() || !distance.is_finite() {
            return Err("Triangle contact arithmetic must be finite".into());
        }
        if distance.abs() > CONTACT_EPSILON {
            return Ok(false);
        }
        let mut inside = true;
        let mut edge_distance = f64::INFINITY;
        for (start, end) in [(a, b), (b, c), (c, a)] {
            let edge = end - start;
            let offset = point - start;
            let side = edge.cross(offset).dot(normal);
            let along = offset.dot(edge) / edge.length_squared();
            let squared = (offset - edge * along.clamp(0.0, 1.0)).length_squared();
            if !side.is_finite() || !along.is_finite() || !squared.is_finite() {
                return Err("Triangle contact arithmetic must be finite".into());
            }
            inside &= side >= 0.0;
            edge_distance = edge_distance.min(squared);
        }
        Ok(inside || edge_distance <= CONTACT_EPSILON * CONTACT_EPSILON)
    }

    pub(super) fn fraction(&self, start: DVec3, end: DVec3) -> Result<Option<f64>, String> {
        Ok(self.intersection(start, end)?.map(|(fraction, _)| fraction))
    }

    pub(super) fn intersection(
        &self,
        start: DVec3,
        end: DVec3,
    ) -> Result<Option<(f64, [f64; 3])>, String> {
        self.intersection_with_endpoints(start, end, TRIANGLE_EPSILON)
    }

    pub(super) fn source_intersection(
        &self,
        start: DVec3,
        end: DVec3,
    ) -> Result<Option<(f64, [f64; 3])>, String> {
        self.intersection_with_endpoints(start, end, 0.0)
    }

    fn intersection_with_endpoints(
        &self,
        start: DVec3,
        end: DVec3,
        endpoint: f64,
    ) -> Result<Option<(f64, [f64; 3])>, String> {
        let [a, b, c] = self.points;
        let delta = end - start;
        let edges = [b - a, c - a];
        let cross = delta.cross(edges[1]);
        let determinant = edges[0].dot(cross);
        let scale = edges[0].length() * edges[1].length() * delta.length();
        if !delta.is_finite() || !determinant.is_finite() || !scale.is_finite() {
            return Err("Shadow ray arithmetic must be finite".into());
        }
        if scale == 0.0 || determinant.abs() <= scale * 1e-12 {
            return Ok(None);
        }
        let offset = start - a;
        let u = offset.dot(cross) / determinant;
        let q = offset.cross(edges[0]);
        let v = delta.dot(q) / determinant;
        let t = edges[1].dot(q) / determinant;
        Ok((u >= -TRIANGLE_EPSILON
            && v >= -TRIANGLE_EPSILON
            && u + v <= 1.0 + TRIANGLE_EPSILON
            && t > endpoint
            && t < 1.0 - endpoint)
            .then_some((t, [1.0 - u - v, u, v])))
    }
}

impl ShadowObject for Triangle {
    const MAX_OBJECTS: usize = source_formats::bsp::MAX_DISPLACEMENT_TRIANGLES;
    fn bounds(&self) -> (DVec3, DVec3) {
        (self.mins, self.maxs)
    }
    fn work(&self) -> usize {
        8
    }
    fn intersects(&self, start: DVec3, end: DVec3) -> Result<bool, String> {
        Ok(self.fraction(start, end)?.is_some())
    }
}

struct Node {
    mins: DVec3,
    maxs: DVec3,
    range: Range<usize>,
    children: Option<[usize; 2]>,
}

pub(super) struct ShadowIndex<T> {
    hulls: Vec<T>,
    order: Vec<usize>,
    nodes: Vec<Node>,
    fractional: bool,
}

impl<T: ShadowObject> ShadowIndex<T> {
    pub(super) fn allocation_bytes(count: usize) -> Result<usize, String> {
        count
            .checked_mul(size_of::<T>() + size_of::<usize>() + 2 * size_of::<Node>())
            .ok_or_else(|| "Shadow-index allocation size overflow".into())
    }

    pub(super) fn new(
        hulls: Vec<T>,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<Self, String> {
        if hulls.len() > T::MAX_OBJECTS {
            return Err(format!(
                "Direct lighting exceeds its {} shadow-object limit",
                T::MAX_OBJECTS
            ));
        }
        consume(hulls.len())?;
        let fractional = hulls.iter().any(ShadowObject::fractional);
        let mut index = Self {
            order: (0..hulls.len()).collect(),
            nodes: Vec::with_capacity(hulls.len().saturating_mul(2)),
            hulls,
            fractional,
        };
        if !index.hulls.is_empty() {
            index.build(0..index.hulls.len(), consume)?;
        }
        Ok(index)
    }

    fn build(
        &mut self,
        range: Range<usize>,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<usize, String> {
        consume(range.len())?;
        let mut mins = DVec3::splat(f64::INFINITY);
        let mut maxs = DVec3::splat(f64::NEG_INFINITY);
        for &hull in &self.order[range.clone()] {
            let (low, high) = self.hulls[hull].bounds();
            mins = mins.min(low);
            maxs = maxs.max(high);
        }
        let node = self.nodes.len();
        self.nodes.push(Node {
            mins,
            maxs,
            range: range.clone(),
            children: None,
        });
        if range.len() > LEAF_SIZE {
            let extent = maxs - mins;
            let axis = if extent.x >= extent.y && extent.x >= extent.z {
                0
            } else if extent.y >= extent.z {
                1
            } else {
                2
            };
            let levels = range.len().ilog2() as usize + 1;
            consume(range.len() * levels)?;
            self.order[range.clone()].sort_unstable_by(|&a, &b| {
                let center = |hull: &T| {
                    let (mins, maxs) = hull.bounds();
                    mins[axis] * 0.5 + maxs[axis] * 0.5
                };
                center(&self.hulls[a])
                    .total_cmp(&center(&self.hulls[b]))
                    .then(a.cmp(&b))
            });
            let middle = range.start + range.len() / 2;
            let left = self.build(range.start..middle, consume)?;
            let right = self.build(middle..range.end, consume)?;
            self.nodes[node].children = Some([left, right]);
        }
        Ok(node)
    }

    pub(super) fn blocked(
        &self,
        start: DVec3,
        end: DVec3,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<bool, String> {
        self.visit(start, end, consume, &mut |hull| {
            Ok(if hull.intersects(start, end)? {
                Visit::Stop
            } else {
                Visit::Continue(1.0)
            })
        })
    }

    pub(super) fn nearest<R>(
        &self,
        start: DVec3,
        end: DVec3,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
        hit: &mut impl FnMut(&T) -> Result<Option<(f64, R)>, String>,
    ) -> Result<Option<(f64, R)>, String> {
        let mut first = None;
        self.visit(start, end, consume, &mut |object| {
            if let Some(candidate) = hit(object)?
                && first
                    .as_ref()
                    .is_none_or(|(fraction, _)| candidate.0 < *fraction)
            {
                first = Some(candidate);
            }
            Ok(Visit::Continue(
                first.as_ref().map_or(1.0, |(fraction, _)| *fraction),
            ))
        })?;
        Ok(first)
    }

    pub(super) fn has_fractional(&self) -> bool {
        self.fractional
    }

    pub(super) fn visit(
        &self,
        start: DVec3,
        end: DVec3,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
        visitor: &mut impl FnMut(&T) -> Result<Visit, String>,
    ) -> Result<bool, String> {
        if !start.is_finite() || !end.is_finite() {
            return Err("Shadow ray must be finite".into());
        }
        let delta = end - start;
        if !self.nodes.is_empty() && !delta.is_finite() {
            return Err("Shadow ray arithmetic must be finite".into());
        }
        self.traverse(
            Query::Segment { start, delta },
            consume,
            &mut |_, object| visitor(object),
        )
    }

    /// Visits stored-box candidates; only Continue certifies complete enumeration, not geometry bounds.
    pub(super) fn visit_bounds(
        &self,
        mins: DVec3,
        maxs: DVec3,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
        visitor: &mut impl FnMut(usize, &T) -> Result<ControlFlow<()>, String>,
    ) -> Result<ControlFlow<()>, String> {
        let query = Query::bounds(mins, maxs)?;
        let stopped = self.traverse(query, consume, &mut |slot, object| {
            Ok(match visitor(slot, object)? {
                ControlFlow::Continue(()) => Visit::Continue(1.0),
                ControlFlow::Break(()) => Visit::Stop,
            })
        })?;
        consume(0)?;
        Ok(if stopped {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        })
    }

    fn traverse(
        &self,
        query: Query,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
        visitor: &mut impl FnMut(usize, &T) -> Result<Visit, String>,
    ) -> Result<bool, String> {
        if self.nodes.is_empty() {
            consume(0)?;
            return Ok(false);
        }
        let mut limit = 1.0;
        let mut pending = [0; STACK_SIZE];
        let mut count = 1;
        while count != 0 {
            count -= 1;
            let node = &self.nodes[pending[count]];
            consume(1)?;
            if !query.overlaps(node.mins, node.maxs, limit) {
                continue;
            }
            if let Some([left, right]) = node.children {
                if count + 2 > pending.len() {
                    return Err("Shadow traversal stack limit exceeded".into());
                }
                pending[count] = right;
                pending[count + 1] = left;
                count += 2;
            } else {
                for &slot in &self.order[node.range.clone()] {
                    let hull = &self.hulls[slot];
                    consume(1)?;
                    let (mins, maxs) = hull.bounds();
                    if !query.overlaps(mins, maxs, limit) {
                        continue;
                    }
                    if matches!(query, Query::Segment { .. }) {
                        consume(hull.work())?;
                    }
                    match visitor(slot, hull)? {
                        Visit::Stop => return Ok(true),
                        Visit::Continue(fraction) => limit = limit.min(fraction),
                    }
                }
            }
        }
        Ok(false)
    }
}

impl ShadowIndex<Occluder> {
    pub(super) fn optical_blocked(
        &self,
        start: DVec3,
        end: DVec3,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<bool, String> {
        self.visit(start, end, consume, &mut |hull| {
            Ok(if hull.optical_entry(start, end)?.is_some() {
                Visit::Stop
            } else {
                Visit::Continue(1.0)
            })
        })
    }

    pub(super) fn optical_sky_fraction(
        &self,
        start: DVec3,
        end: DVec3,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<Option<f64>, String> {
        self.sky_fraction_with(start, end, consume, true)
    }

    #[cfg(test)]
    pub(super) fn sky_fraction(
        &self,
        start: DVec3,
        end: DVec3,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<Option<f64>, String> {
        self.sky_fraction_with(start, end, consume, false)
    }

    fn sky_fraction_with(
        &self,
        start: DVec3,
        end: DVec3,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
        optical: bool,
    ) -> Result<Option<f64>, String> {
        let mut first: Option<(f64, bool)> = None;
        self.visit(start, end, consume, &mut |hull| {
            let entry = if optical {
                hull.optical_entry(start, end)?
            } else {
                hull.entry(start, end)?
            };
            if let Some((fraction, sky)) = entry {
                first = match first {
                    Some((previous, visible))
                        if (fraction - previous).abs() <= TRIANGLE_EPSILON =>
                    {
                        Some((previous.min(fraction), visible && sky))
                    }
                    Some((previous, _)) if previous < fraction => first,
                    _ => Some((fraction, sky)),
                };
            }
            Ok(Visit::Continue(
                first.as_ref().map_or(1.0, |(fraction, _)| *fraction),
            ))
        })?;
        Ok(first.and_then(|(fraction, sky)| sky.then_some(fraction)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use map_core::Plane;

    struct BoundsOnly {
        mins: DVec3,
        maxs: DVec3,
    }

    impl ShadowObject for BoundsOnly {
        const MAX_OBJECTS: usize = MAX_OCCLUDERS;

        fn bounds(&self) -> (DVec3, DVec3) {
            (self.mins, self.maxs)
        }

        fn work(&self) -> usize {
            panic!("Box enumeration must not charge an unperformed intersection");
        }

        fn intersects(&self, _: DVec3, _: DVec3) -> Result<bool, String> {
            panic!("Box enumeration must not intersect geometry");
        }
    }

    fn cube(center: DVec3, half: f64) -> Occluder {
        let planes = [DVec3::X, DVec3::Y, DVec3::Z]
            .into_iter()
            .flat_map(|normal| [normal, -normal])
            .map(|normal| Plane {
                normal,
                dist: normal.dot(center) + half,
            })
            .collect();
        Occluder::new(
            planes,
            center - DVec3::splat(half),
            center + DVec3::splat(half),
        )
        .unwrap()
    }

    #[test]
    fn box_candidates_keep_slots_touching_bounds_and_original_borrows() {
        let boxes = [12.0, 2.0, 8.0, 0.0, 10.0, 4.0, 6.0, 14.0].map(|x| BoundsOnly {
            mins: DVec3::new(x - 0.25, -0.25, -0.25),
            maxs: DVec3::new(x + 0.25, 0.25, 0.25),
        });
        let index = ShadowIndex::new(boxes.into(), &mut |_| Ok(())).unwrap();
        let mins = DVec3::new(2.25, 0.0, 0.0);
        let maxs = DVec3::new(9.75, 0.0, 0.0);
        for _ in 0..2 {
            let mut slots = Vec::new();
            let completion = index
                .visit_bounds(mins, maxs, &mut |_| Ok(()), &mut |slot, object| {
                    assert!(std::ptr::eq(object, &index.hulls[slot]));
                    slots.push(slot);
                    Ok(ControlFlow::Continue(()))
                })
                .unwrap();
            assert_eq!(completion, ControlFlow::Continue(()));
            assert_eq!(slots, [1, 5, 6, 2, 4]);
        }
        let mut inner = Vec::new();
        let completion = index
            .visit_bounds(
                DVec3::new(mins.x.next_up(), 0.0, 0.0),
                DVec3::new(maxs.x.next_down(), 0.0, 0.0),
                &mut |_| Ok(()),
                &mut |slot, _| {
                    inner.push(slot);
                    Ok(ControlFlow::Continue(()))
                },
            )
            .unwrap();
        assert_eq!(completion, ControlFlow::Continue(()));
        assert_eq!(inner, [5, 6, 2]);
        let mut work = 0;
        let mut visits = 0;
        let completion = index
            .visit_bounds(
                DVec3::splat(-20.0),
                DVec3::splat(20.0),
                &mut |amount| {
                    work += amount;
                    Ok(())
                },
                &mut |_, _| {
                    visits += 1;
                    Ok(ControlFlow::Continue(()))
                },
            )
            .unwrap();
        assert_eq!(completion, ControlFlow::Continue(()));
        assert_eq!(visits, 8);
        assert_eq!(work, 11);
    }

    #[test]
    fn bundle_boxes_keep_off_axis_blockers_and_conservative_false_positives() {
        let hulls = vec![
            cube(DVec3::new(2.0, 0.0, 5.0), 0.25),
            cube(DVec3::new(8.0, 0.0, 2.0), 0.25),
        ];
        let corners = [
            DVec3::new(-10.0, -10.0, 10.0),
            DVec3::new(-10.0, 10.0, 10.0),
            DVec3::new(10.0, -10.0, 10.0),
            DVec3::new(10.0, 10.0, 10.0),
        ];
        for hull in &hulls {
            for endpoint in corners.into_iter().chain([DVec3::Z * 10.0]) {
                assert!(!hull.intersects(DVec3::ZERO, endpoint).unwrap());
            }
        }
        assert!(
            hulls[0]
                .intersects(DVec3::ZERO, DVec3::new(4.0, 0.0, 10.0))
                .unwrap()
        );
        assert!(hulls[1].mins.x > hulls[1].maxs.z);
        let index = ShadowIndex::new(hulls, &mut |_| Ok(())).unwrap();
        let mut slots = Vec::new();
        let completion = index
            .visit_bounds(
                DVec3::new(-10.0, -10.0, 0.0),
                DVec3::new(10.0, 10.0, 10.0),
                &mut |_| Ok(()),
                &mut |slot, _| {
                    slots.push(slot);
                    Ok(ControlFlow::Continue(()))
                },
            )
            .unwrap();
        assert_eq!(completion, ControlFlow::Continue(()));
        assert_eq!(slots, [0, 1]);
    }

    #[test]
    fn bounds_visits_propagate_shared_limits_cancellation_and_callback_failure() {
        use super::super::bake::Budget;
        use crate::Control;

        let empty = ShadowIndex::new(Vec::<BoundsOnly>::new(), &mut |_| Ok(())).unwrap();
        let completion = empty
            .visit_bounds(DVec3::ZERO, DVec3::ZERO, &mut |_| Ok(()), &mut |_, _| {
                panic!("An empty index must have no candidates");
            })
            .unwrap();
        assert_eq!(completion, ControlFlow::Continue(()));
        let control = Control::default();
        let mut budget = Budget::new(&control);
        control.cancel().unwrap();
        assert!(
            empty
                .visit_bounds(
                    DVec3::ZERO,
                    DVec3::ZERO,
                    &mut |amount| budget.consume(amount),
                    &mut |_, _| Ok(ControlFlow::Continue(())),
                )
                .is_err()
        );
        assert!(
            empty
                .blocked(DVec3::ZERO, DVec3::ZERO, &mut |amount| budget
                    .consume(amount))
                .is_err()
        );
        let index = ShadowIndex::new(vec![cube(DVec3::ZERO, 1.0); 2], &mut |_| Ok(())).unwrap();
        let control = Control::default();
        let mut budget = Budget::with_limit(&control, 2);
        let mut visits = 0;
        let result = index.visit_bounds(
            DVec3::ZERO,
            DVec3::ZERO,
            &mut |amount| budget.consume(amount),
            &mut |_, _| {
                visits += 1;
                Ok(ControlFlow::Continue(()))
            },
        );
        assert!(result.unwrap_err().contains("shared 2 work-unit limit"));
        assert_eq!(visits, 1);
        for (cancel_on, stop) in [(1, false), (2, false), (1, true)] {
            let control = Control::default();
            let mut cancelled_budget = Budget::new(&control);
            let mut visits = 0;
            let result = index.visit_bounds(
                DVec3::ZERO,
                DVec3::ZERO,
                &mut |amount| cancelled_budget.consume(amount),
                &mut |_, _| {
                    visits += 1;
                    if visits == cancel_on {
                        control.cancel()?;
                    }
                    Ok(if stop {
                        ControlFlow::Break(())
                    } else {
                        ControlFlow::Continue(())
                    })
                },
            );
            assert!(result.is_err());
            assert_eq!(visits, cancel_on);
        }
        let error = index
            .visit_bounds(DVec3::ZERO, DVec3::ZERO, &mut |_| Ok(()), &mut |_, _| {
                Err("provider refused".into())
            })
            .unwrap_err();
        assert_eq!(error, "provider refused");
        let result = index.visit_bounds(
            DVec3::ZERO,
            DVec3::ZERO,
            &mut |amount| budget.consume(amount),
            &mut |_, _| Ok(ControlFlow::Continue(())),
        );
        assert!(result.is_err());
        for (mins, maxs) in [
            (DVec3::splat(f64::NAN), DVec3::ONE),
            (DVec3::ZERO, DVec3::splat(f64::INFINITY)),
            (DVec3::X, DVec3::ZERO),
        ] {
            assert!(
                empty
                    .visit_bounds(mins, maxs, &mut |_| Ok(()), &mut |_, _| Ok(
                        ControlFlow::Continue(())
                    ))
                    .is_err()
            );
        }
    }

    #[test]
    fn bounds_visits_stop_at_the_fixed_traversal_stack_limit() {
        let mut index = ShadowIndex::new(
            (0..=STACK_SIZE)
                .map(|_| BoundsOnly {
                    mins: DVec3::ZERO,
                    maxs: DVec3::ONE,
                })
                .collect(),
            &mut |_| Ok(()),
        )
        .unwrap();
        index.nodes = (0..=STACK_SIZE)
            .map(|depth| Node {
                mins: DVec3::ZERO,
                maxs: DVec3::ONE,
                range: 0..STACK_SIZE + 1 - depth,
                children: (depth < STACK_SIZE).then_some([depth + 1, STACK_SIZE + 1 + depth]),
            })
            .chain((0..STACK_SIZE).map(|depth| Node {
                mins: DVec3::ZERO,
                maxs: DVec3::ONE,
                range: STACK_SIZE - depth..STACK_SIZE + 1 - depth,
                children: None,
            }))
            .collect();
        let error = index
            .visit_bounds(DVec3::ZERO, DVec3::ONE, &mut |_| Ok(()), &mut |_, _| {
                panic!("Stack exhaustion must refuse before reaching this deep leaf");
            })
            .unwrap_err();
        assert_eq!(error, "Shadow traversal stack limit exceeded");
    }

    #[test]
    fn shared_traversal_preserves_dynamic_segment_limits() {
        let index = ShadowIndex::new(
            (1..=8)
                .map(|i| cube(DVec3::X * (i * 2) as f64, 0.5))
                .collect(),
            &mut |_| Ok(()),
        )
        .unwrap();
        let mut visits = 0;
        let stopped = index
            .visit(DVec3::ZERO, DVec3::X * 20.0, &mut |_| Ok(()), &mut |_| {
                visits += 1;
                Ok(Visit::Continue(0.1))
            })
            .unwrap();
        assert!(!stopped);
        assert_eq!(visits, 1);
    }

    #[test]
    fn global_opaque_hulls_block_light_and_sky_but_not_world_occupancy() {
        let caster = cube(DVec3::ZERO, 1.0).into_global_caster();
        let sky = cube(DVec3::X * 4.0, 1.0).with_sky(vec![true; 6]).unwrap();
        let index = ShadowIndex::new(vec![caster, sky], &mut |_| Ok(())).unwrap();
        let start = DVec3::NEG_X * 2.0;
        let end = DVec3::X * 6.0;
        assert!(index.optical_blocked(start, end, &mut |_| Ok(())).unwrap());
        assert!(
            index
                .optical_sky_fraction(start, end, &mut |_| Ok(()))
                .unwrap()
                .is_none()
        );
        let occupancy = index
            .nearest(DVec3::ZERO, DVec3::ZERO, &mut |_| Ok(()), &mut |hull| {
                if !hull.occupies_world() {
                    return Ok(None);
                }
                Ok(hull.entry(DVec3::ZERO, DVec3::ZERO)?.map(|hit| (hit.0, ())))
            })
            .unwrap();
        assert!(occupancy.is_none());
        assert!(
            index
                .optical_sky_fraction(DVec3::X * 2.0, end, &mut |_| Ok(()))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn brush_cutouts_sample_both_boundaries_without_changing_occupancy() {
        let face = super::super::brush_alpha::test_face([0, 255, 0, 255]);
        let mut faces = vec![None; 6];
        faces[0] = Some(face.clone());
        faces[1] = Some(face);
        let hull = cube(DVec3::ZERO, 1.0).with_optical(faces, None).unwrap();
        let ray = |y: f64| (DVec3::new(-2.0, y, 0.25), DVec3::new(2.0, y, 0.25));
        let (start, end) = ray(0.25);
        let index = ShadowIndex::new(vec![hull.clone()], &mut |_| Ok(())).unwrap();
        assert!(index.blocked(start, end, &mut |_| Ok(())).unwrap());
        assert!(!index.optical_blocked(start, end, &mut |_| Ok(())).unwrap());
        let (start, end) = ray(0.75);
        assert!(index.optical_blocked(start, end, &mut |_| Ok(())).unwrap());
        assert!(
            hull.optical_entry(DVec3::new(0.0, 0.25, 0.25), DVec3::new(2.0, 0.25, 0.25))
                .unwrap()
                .is_none()
        );
        assert!(
            hull.optical_entry(DVec3::new(0.0, 0.75, 0.25), DVec3::new(2.0, 0.75, 0.25))
                .unwrap()
                .is_some()
        );
        assert!(
            hull.optical_entry(DVec3::new(-2.0, 0.75, 0.25), DVec3::new(-1.0, 0.75, 0.25))
                .unwrap()
                .is_none()
        );
        let mut mixed = hull.clone();
        mixed.optical.as_mut().unwrap().faces[0] = None;
        let (start, end) = ray(0.25);
        assert_eq!(
            mixed.optical_entry(start, end).unwrap(),
            Some((0.75, false))
        );
        assert_eq!(
            mixed.optical_entry(start.lerp(end, 0.5), end).unwrap(),
            Some((0.5, false))
        );
        assert!(
            hull.optical_entry(DVec3::new(-2.0, -2.0, 0.25), DVec3::new(2.0, 2.0, 0.25))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn global_cutouts_retain_coverage_pose_and_exclude_world_occupancy() {
        let face = super::super::brush_alpha::test_face([0, 255, 0, 255]);
        let pose = crate::brush_model_pose::Pose {
            offset: DVec3::Y * 0.5,
            rotation: None,
        };
        let hull = cube(pose.offset, 1.0)
            .with_optical(vec![Some(face); 6], Some(pose))
            .unwrap()
            .into_global_caster();
        assert!(!hull.occupies_world());
        let ray = |y| (DVec3::new(-2.0, y, 0.25), DVec3::new(2.0, y, 0.25));
        let (clear_start, clear_end) = ray(0.75);
        assert!(
            hull.optical_entry(clear_start, clear_end)
                .unwrap()
                .is_none()
        );
        let (opaque_start, opaque_end) = ray(1.25);
        assert!(
            hull.optical_entry(opaque_start, opaque_end)
                .unwrap()
                .is_some()
        );
        let sky = cube(DVec3::X * 4.0, 1.0).with_sky(vec![true; 6]).unwrap();
        let index = ShadowIndex::new(vec![hull.clone(), sky], &mut |_| Ok(())).unwrap();
        let sky_end = DVec3::new(6.0, 0.75, 0.25);
        assert!(
            index
                .optical_sky_fraction(clear_start, sky_end, &mut |_| Ok(()))
                .unwrap()
                .is_some()
        );
        let opaque = cube(pose.offset, 1.0).into_global_caster();
        let index = ShadowIndex::new(vec![hull, opaque], &mut |_| Ok(())).unwrap();
        assert!(
            index
                .optical_blocked(clear_start, clear_end, &mut |_| Ok(()))
                .unwrap()
        );
    }

    #[test]
    fn model_cutouts_use_each_hulls_inverse_pose_and_preserve_geometric_hits() {
        use crate::brush_model_pose::Pose;
        use glam::DMat3;

        let face = super::super::brush_alpha::test_face([0, 255, 255, 255]);
        let c = std::f64::consts::FRAC_1_SQRT_2;
        let mixed = DMat3::from_cols(
            DVec3::new(0.5, 0.5, -c),
            DVec3::new(c / 2.0 - 0.5, c / 2.0 + 0.5, 0.5),
            DVec3::new(c / 2.0 + 0.5, c / 2.0 - 0.5, 0.5),
        );
        for (offset, rotation) in [
            (DVec3::new(10.125, -20.5, 30.5), None),
            (
                DVec3::new(101.5, -67.25, 32.125),
                Some(DMat3::from_cols(DVec3::Y, -DVec3::X, DVec3::Z)),
            ),
            (DVec3::new(-70.25, 41.5, -25.125), Some(mixed)),
        ] {
            let literal = rotation.unwrap_or(DMat3::IDENTITY);
            let pose = Pose { offset, rotation };
            let mut hull = cube(DVec3::ZERO, 1.0);
            for plane in &mut hull.planes {
                plane.normal = literal * plane.normal;
                plane.dist += plane.normal.dot(offset);
            }
            hull.mins = offset - DVec3::splat(2.0);
            hull.maxs = offset + DVec3::splat(2.0);
            let opaque_work = hull.work();
            let untextured = hull
                .clone()
                .with_optical(vec![None; 6], Some(pose))
                .unwrap();
            assert!(untextured.optical.is_none());
            assert_eq!(untextured.work(), opaque_work);
            let hull = hull
                .with_optical(vec![Some(face.clone()); 6], Some(pose))
                .unwrap();
            let inverse_work = if rotation.is_some() { 18 } else { 3 };
            assert_eq!(hull.work(), opaque_work + 6 * (32 + inverse_work));
            for (y, z, covered) in [(0.25, 0.25, false), (0.75, 0.25, true), (0.25, 0.75, true)] {
                let start = literal * DVec3::new(-2.0, y, z) + offset;
                let end = literal * DVec3::new(2.0, y, z) + offset;
                assert!(hull.entry(start, end).unwrap().is_some());
                assert_eq!(hull.optical_entry(start, end).unwrap().is_some(), covered);
            }
        }
    }

    #[test]
    fn sky_queries_continue_through_cutouts_but_keep_opaque_ties() {
        let face = super::super::brush_alpha::test_face([0; 4]);
        let pane = cube(DVec3::ZERO, 1.0)
            .with_optical(vec![Some(face); 6], None)
            .unwrap();
        let sky = cube(DVec3::X * 4.0, 1.0)
            .with_sky(vec![false, true, false, false, false, false])
            .unwrap();
        let start = DVec3::new(-3.0, 0.25, 0.25);
        let end = DVec3::new(8.0, 0.25, 0.25);
        let index = ShadowIndex::new(vec![pane.clone(), sky.clone()], &mut |_| Ok(())).unwrap();
        assert_eq!(
            index
                .optical_sky_fraction(start, end, &mut |_| Ok(()))
                .unwrap(),
            Some(6.0 / 11.0)
        );
        assert_eq!(
            index.sky_fraction(start, end, &mut |_| Ok(())).unwrap(),
            None
        );
        let index =
            ShadowIndex::new(vec![pane, sky, cube(DVec3::X * 4.0, 1.0)], &mut |_| Ok(())).unwrap();
        assert_eq!(
            index
                .optical_sky_fraction(start, end, &mut |_| Ok(()))
                .unwrap(),
            None
        );
    }

    #[test]
    fn sky_visibility_uses_the_first_surface_and_opaque_ties_win() {
        let sky = cube(DVec3::new(0.0, 0.0, 12.0), 4.0)
            .with_sky(vec![false, false, false, false, false, true])
            .unwrap();
        let distant = cube(DVec3::new(0.0, 0.0, 24.0), 2.0);
        let start = DVec3::ZERO;
        let end = DVec3::Z * 32.0;
        for hulls in [vec![sky.clone()], vec![sky.clone(), distant]] {
            let index = ShadowIndex::new(hulls, &mut |_| Ok(())).unwrap();
            assert_eq!(
                index.sky_fraction(start, end, &mut |_| Ok(())).unwrap(),
                Some(0.25)
            );
            assert_eq!(
                index
                    .sky_fraction(
                        DVec3::new(8.0, 0.0, 12.0),
                        DVec3::new(0.0, 0.0, 12.0),
                        &mut |_| Ok(())
                    )
                    .unwrap(),
                None
            );
            assert_eq!(
                index
                    .sky_fraction(
                        DVec3::new(8.0, 0.0, 4.0),
                        DVec3::new(0.0, 0.0, 12.0),
                        &mut |_| Ok(())
                    )
                    .unwrap(),
                None
            );
        }
        let same = cube(DVec3::new(0.0, 0.0, 12.0), 4.0);
        for hulls in [
            vec![sky.clone(), same.clone()],
            vec![same, sky.clone()],
            vec![sky, cube(DVec3::Z * 4.0, 2.0)],
        ] {
            let index = ShadowIndex::new(hulls, &mut |_| Ok(())).unwrap();
            assert_eq!(
                index.sky_fraction(start, end, &mut |_| Ok(())).unwrap(),
                None
            );
        }
    }

    #[test]
    fn triangle_point_contact_keeps_edges_vertices_and_a_bounded_geometric_band() {
        let points = [DVec3::ZERO, DVec3::X * 8.0, DVec3::Y * 8.0];
        for points in [points, [points[2], points[1], points[0]]] {
            let triangle = Triangle::new(points).unwrap();
            let index = ShadowIndex::new(vec![triangle.clone()], &mut |_| Ok(())).unwrap();
            for point in [
                DVec3::ZERO,
                DVec3::new(2.0, 2.0, 0.0),
                DVec3::new(4.0, 4.0, 0.0),
                DVec3::new(2.0, 2.0, CONTACT_EPSILON * 0.5),
                DVec3::new(2.0, 2.0, -CONTACT_EPSILON * 0.5),
                DVec3::new(-CONTACT_EPSILON * 0.5, 0.0, 0.0),
            ] {
                assert!(
                    index
                        .nearest(point, point, &mut |_| Ok(()), &mut |triangle| {
                            Ok(triangle.point_contact(point)?.then_some((0.0, ())))
                        })
                        .unwrap()
                        .is_some(),
                    "{point}"
                );
            }
            for point in [
                DVec3::new(2.0, 2.0, CONTACT_EPSILON * 2.0),
                DVec3::new(2.0, 2.0, -CONTACT_EPSILON * 2.0),
                DVec3::new(-CONTACT_EPSILON * 2.0, 0.0, 0.0),
                DVec3::new(6.0, 6.0, 0.0),
            ] {
                assert!(!triangle.point_contact(point).unwrap(), "{point}");
            }
            assert!(triangle.point_contact(DVec3::splat(f64::NAN)).is_err());
        }
        let thin =
            Triangle::new([DVec3::ZERO, DVec3::X * 8.0, DVec3::new(8.0, 1e-8, 0.0)]).unwrap();
        assert!(
            !thin
                .point_contact(-DVec3::X * 10.0 * CONTACT_EPSILON)
                .unwrap()
        );
    }

    #[test]
    fn terrain_triangles_cast_two_sided_shadows_with_clear_edges_and_endpoints() {
        let triangle = Triangle::new([DVec3::ZERO, DVec3::X * 8.0, DVec3::Y * 8.0]).unwrap();
        let index = ShadowIndex::new(vec![triangle], &mut |_| Ok(())).unwrap();
        for (start, end, expected) in [
            (DVec3::new(2.0, 2.0, -8.0), DVec3::new(2.0, 2.0, 8.0), true),
            (DVec3::new(2.0, 2.0, 8.0), DVec3::new(2.0, 2.0, -8.0), true),
            (DVec3::new(4.0, 4.0, -8.0), DVec3::new(4.0, 4.0, 8.0), true),
            (DVec3::new(6.0, 6.0, -8.0), DVec3::new(6.0, 6.0, 8.0), false),
            (DVec3::new(2.0, 2.0, 0.0), DVec3::new(2.0, 2.0, 8.0), false),
            (DVec3::new(2.0, 2.0, -8.0), DVec3::new(2.0, 2.0, 0.0), false),
            (DVec3::new(2.0, 2.0, 0.0), DVec3::new(4.0, 2.0, 0.0), false),
        ] {
            assert_eq!(
                index.blocked(start, end, &mut |_| Ok(())).unwrap(),
                expected
            );
        }
        assert!(Triangle::new([DVec3::ZERO; 3]).is_err());
        assert!(Triangle::new([DVec3::splat(f64::NAN); 3]).is_err());
    }

    #[test]
    fn indexed_shadows_match_exact_hulls_for_sparse_and_boundary_rays() {
        let mut hulls = (0..128)
            .map(|i| cube(DVec3::new(i as f64 * 16.0, 0.0, 0.0), 4.0))
            .collect::<Vec<_>>();
        let mut clipped = cube(DVec3::new(0.0, 24.0, 0.0), 4.0);
        clipped.planes.push(Plane {
            normal: DVec3::new(1.0, 1.0, 0.0).normalize(),
            dist: 24.0 / 2_f64.sqrt(),
        });
        hulls.push(clipped);
        let index = ShadowIndex::new(hulls.clone(), &mut |_| Ok(())).unwrap();
        let mut rays = vec![
            (DVec3::ZERO, DVec3::ZERO),
            (DVec3::new(-8.0, 4.0, 0.0), DVec3::new(8.0, 4.0, 0.0)),
            (DVec3::new(-8.0, 4.001, 0.0), DVec3::new(8.0, 4.001, 0.0)),
            (DVec3::new(3.0, 27.0, -8.0), DVec3::new(3.0, 27.0, 8.0)),
            (DVec3::new(-3.0, 21.0, -8.0), DVec3::new(-3.0, 21.0, 8.0)),
        ];
        for i in 0..256 {
            let x = i as f64 * 8.0;
            rays.push((DVec3::new(x, -8.0, 0.0), DVec3::new(x, 8.0, 0.0)));
            rays.push((DVec3::new(x, 8.0, 0.0), DVec3::new(x, -8.0, 0.0)));
        }
        for (start, end) in rays {
            let expected = hulls
                .iter()
                .any(|hull| hull.intersects(start, end).unwrap());
            assert_eq!(
                index.blocked(start, end, &mut |_| Ok(())).unwrap(),
                expected
            );
        }
        let mut work = 0;
        assert!(
            !index
                .blocked(
                    DVec3::new(8.0, -8.0, 0.0),
                    DVec3::new(8.0, 8.0, 0.0),
                    &mut |n| {
                        work += n;
                        Ok(())
                    }
                )
                .unwrap()
        );
        assert!(work < hulls.len() * 7 / 8, "{work} index work units");
    }

    #[test]
    fn nearest_surface_keeps_the_closest_payload_and_checks_cancellation() {
        let triangles = [3.0, 2.0].map(|z| {
            Triangle::new([
                DVec3::new(-1.0, -1.0, z),
                DVec3::new(1.0, -1.0, z),
                DVec3::new(0.0, 1.0, z),
            ])
            .unwrap()
        });
        let index = ShadowIndex::new(triangles.to_vec(), &mut |_| Ok(())).unwrap();
        let end = DVec3::Z * 10.0;
        let nearest = index
            .nearest(DVec3::ZERO, end, &mut |_| Ok(()), &mut |triangle| {
                Ok(triangle
                    .fraction(DVec3::ZERO, end)?
                    .map(|fraction| (fraction, triangle.points[0].z)))
            })
            .unwrap();
        assert_eq!(nearest, Some((0.2, 2.0)));
        assert!(
            index
                .nearest(
                    DVec3::ZERO,
                    end,
                    &mut |_| Err("cancelled".into()),
                    &mut |_| Ok(Some((0.2, 2.0)))
                )
                .is_err()
        );
    }

    #[test]
    fn shadow_index_enforces_cancellation_empty_rays_and_hull_limits() {
        assert!(
            ShadowIndex::new(vec![cube(DVec3::ZERO, 1.0)], &mut |_| Err(
                "cancelled".into()
            ))
            .is_err()
        );
        let empty = ShadowIndex::new(Vec::<Occluder>::new(), &mut |_| Ok(())).unwrap();
        assert!(
            !empty
                .blocked(DVec3::ZERO, DVec3::X, &mut |_| Ok(()))
                .unwrap()
        );
        assert!(
            empty
                .blocked(DVec3::splat(f64::NAN), DVec3::X, &mut |_| Ok(()))
                .is_err()
        );
        let index = ShadowIndex::new(vec![cube(DVec3::ZERO, 1.0)], &mut |_| Ok(())).unwrap();
        assert!(
            index
                .blocked(DVec3::ZERO, DVec3::X, &mut |_| Err("cancelled".into()))
                .is_err()
        );
        assert!(
            ShadowIndex::new(
                vec![cube(DVec3::ZERO, 1.0); MAX_OCCLUDERS + 1],
                &mut |_| Ok(())
            )
            .is_err()
        );
    }
}
