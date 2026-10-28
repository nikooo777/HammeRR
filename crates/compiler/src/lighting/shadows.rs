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
        self.plane_count() * 2
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
    fn fractional(&self) -> bool {
        self.optical.as_ref().is_some_and(|optical| {
            optical
                .faces
                .iter()
                .flatten()
                .any(super::brush_alpha::Face::fractional)
        })
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

    pub(super) fn topological_intersection(
        &self,
        start: DVec3,
        end: DVec3,
        endpoint: f64,
    ) -> Result<Option<(f64, [f64; 3])>, String> {
        let [a, b, c] = self.points;
        let delta = end - start;
        let edges = [b - a, c - a];
        let determinant = edges[0].dot(delta.cross(edges[1]));
        let scale = edges[0].length() * edges[1].length() * delta.length();
        if !delta.is_finite() || !determinant.is_finite() || !scale.is_finite() {
            return Err("Shadow ray arithmetic must be finite".into());
        }
        if scale == 0.0 || determinant.abs() <= scale * 1e-12 {
            return Ok(None);
        }
        let extent = delta.abs();
        let axis = if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        };
        let mut x = (axis + 1) % 3;
        let mut y = (axis + 2) % 3;
        if delta[axis] < 0.0 {
            std::mem::swap(&mut x, &mut y);
        }
        let shear = [delta[x] / delta[axis], delta[y] / delta[axis]];
        let projected = self.points.map(|point| {
            let relative = point - start;
            [
                relative[x] - shear[0] * relative[axis],
                relative[y] - shear[1] * relative[axis],
                relative[axis] / delta[axis],
            ]
        });
        let weights: [f64; 3] = std::array::from_fn(|corner| {
            let a = projected[(corner + 1) % 3];
            let b = projected[(corner + 2) % 3];
            a[0] * b[1] - a[1] * b[0]
        });
        if projected.iter().flatten().any(|value| !value.is_finite())
            || weights.iter().any(|value| !value.is_finite())
        {
            return Err("Terrain shadow edge arithmetic must be finite".into());
        }
        if weights.iter().any(|&weight| weight < 0.0) && weights.iter().any(|&weight| weight > 0.0)
        {
            return Ok(None);
        }
        let total = weights.iter().sum::<f64>();
        if total == 0.0 {
            return Ok(None);
        }
        let fraction = weights
            .into_iter()
            .zip(projected)
            .map(|(weight, point)| weight * point[2])
            .sum::<f64>()
            / total;
        if !total.is_finite() || !fraction.is_finite() {
            return Err("Terrain shadow crossing arithmetic must be finite".into());
        }
        Ok((fraction > endpoint && fraction < 1.0 - endpoint)
            .then_some((fraction, weights.map(|weight| weight / total))))
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

    pub(super) fn objects(&self) -> &[T] {
        &self.hulls
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

    pub(super) fn accumulate_coverage(
        &self,
        start: DVec3,
        end: DVec3,
        coverage: &mut super::transport::Coverage,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<(), String> {
        if coverage.opaque() {
            consume(0)?;
            return Ok(());
        }
        if !self.has_fractional() {
            if self.optical_blocked(start, end, consume)? {
                coverage.add(u8::MAX);
            }
            return Ok(());
        }
        self.visit(start, end, consume, &mut |hull| {
            for boundary in hull.optical_boundaries(start, end)?.into_iter().flatten() {
                coverage.add(boundary.coverage);
                if coverage.opaque() {
                    return Ok(Visit::Stop);
                }
            }
            Ok(Visit::Continue(1.0))
        })?;
        Ok(())
    }

    pub(super) fn accumulate_sky_coverage(
        &self,
        start: DVec3,
        end: DVec3,
        sky_fraction: f64,
        coverage: &mut super::transport::Coverage,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<(), String> {
        if !sky_fraction.is_finite() || sky_fraction <= 0.0 || sky_fraction >= 1.0 {
            return Err("Sky endpoint fraction must be finite and inside its ray".into());
        }
        if coverage.opaque() {
            consume(0)?;
            return Ok(());
        }
        self.visit(start, end, consume, &mut |hull| {
            for boundary in hull.optical_boundaries(start, end)?.into_iter().flatten() {
                if boundary.fraction > sky_fraction + TRIANGLE_EPSILON {
                    continue;
                }
                let value = if (boundary.fraction - sky_fraction).abs() <= TRIANGLE_EPSILON {
                    boundary.non_sky_coverage
                } else {
                    boundary.coverage
                };
                coverage.add(value);
                if coverage.opaque() {
                    return Ok(Visit::Stop);
                }
            }
            Ok(Visit::Continue(sky_fraction))
        })?;
        Ok(())
    }

    pub(super) fn optical_sky_fraction(
        &self,
        start: DVec3,
        end: DVec3,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<Option<f64>, String> {
        let mut sky: Option<f64> = None;
        let mut opaque: Option<f64> = None;
        self.visit(start, end, consume, &mut |hull| {
            for boundary in hull.optical_boundaries(start, end)?.into_iter().flatten() {
                if boundary.sky {
                    sky = Some(sky.map_or(boundary.fraction, |previous| {
                        previous.min(boundary.fraction)
                    }));
                }
                if boundary.non_sky_coverage == u8::MAX {
                    opaque = Some(opaque.map_or(boundary.fraction, |previous| {
                        previous.min(boundary.fraction)
                    }));
                }
            }
            Ok(Visit::Continue(
                sky.into_iter().chain(opaque).fold(1.0, f64::min),
            ))
        })?;
        Ok(sky
            .filter(|fraction| opaque.is_none_or(|blocker| blocker > *fraction + TRIANGLE_EPSILON)))
    }
}
