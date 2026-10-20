use glam::DVec3;
use std::ops::Range;

use super::Occluder;

const MAX_OCCLUDERS: usize = 65_536;
const LEAF_SIZE: usize = 4;
const STACK_SIZE: usize = usize::BITS as usize;
const TRIANGLE_EPSILON: f64 = 1e-9;
pub(super) const CONTACT_EPSILON: f64 = 1e-6;

enum Visit {
    Continue(f64),
    Stop,
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
}

impl ShadowObject for Occluder {
    const MAX_OBJECTS: usize = MAX_OCCLUDERS;
    fn bounds(&self) -> (DVec3, DVec3) {
        (self.mins, self.maxs)
    }
    fn work(&self) -> usize {
        self.plane_count() + 1
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
            && t > TRIANGLE_EPSILON
            && t < 1.0 - TRIANGLE_EPSILON)
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
        let mut index = Self {
            order: (0..hulls.len()).collect(),
            nodes: Vec::with_capacity(hulls.len().saturating_mul(2)),
            hulls,
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

    fn visit(
        &self,
        start: DVec3,
        end: DVec3,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
        visitor: &mut impl FnMut(&T) -> Result<Visit, String>,
    ) -> Result<bool, String> {
        if !start.is_finite() || !end.is_finite() {
            return Err("Shadow ray must be finite".into());
        }
        if self.nodes.is_empty() {
            return Ok(false);
        }
        let delta = end - start;
        if !delta.is_finite() {
            return Err("Shadow ray arithmetic must be finite".into());
        }
        let mut limit = 1.0;
        let mut pending = [0; STACK_SIZE];
        let mut count = 1;
        while count != 0 {
            count -= 1;
            let node = &self.nodes[pending[count]];
            consume(1)?;
            if !ray_box(start, delta, node.mins, node.maxs, limit) {
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
                for &hull in &self.order[node.range.clone()] {
                    let hull = &self.hulls[hull];
                    consume(1)?;
                    let (mins, maxs) = hull.bounds();
                    if !ray_box(start, delta, mins, maxs, limit) {
                        continue;
                    }
                    consume(hull.work())?;
                    match visitor(hull)? {
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
    pub(super) fn sky_fraction(
        &self,
        start: DVec3,
        end: DVec3,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<Option<f64>, String> {
        let mut first: Option<(f64, bool)> = None;
        self.visit(start, end, consume, &mut |hull| {
            if let Some((fraction, sky)) = hull.entry(start, end)? {
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
