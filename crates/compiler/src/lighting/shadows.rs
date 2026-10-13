use glam::DVec3;
use std::ops::Range;

use super::Occluder;

const MAX_OCCLUDERS: usize = 65_536;
const LEAF_SIZE: usize = 4;
const STACK_SIZE: usize = usize::BITS as usize;

struct Node {
    mins: DVec3,
    maxs: DVec3,
    range: Range<usize>,
    children: Option<[usize; 2]>,
}

pub(super) struct ShadowIndex {
    hulls: Vec<Occluder>,
    order: Vec<usize>,
    nodes: Vec<Node>,
}

impl ShadowIndex {
    pub(super) fn new(
        hulls: Vec<Occluder>,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<Self, String> {
        if hulls.len() > MAX_OCCLUDERS {
            return Err("Direct lighting exceeds the 65,536 shadow-hull limit".into());
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
            mins = mins.min(self.hulls[hull].mins);
            maxs = maxs.max(self.hulls[hull].maxs);
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
                let center = |hull: &Occluder| hull.mins[axis] * 0.5 + hull.maxs[axis] * 0.5;
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
        if !start.is_finite() || !end.is_finite() {
            return Err("Shadow ray must be finite".into());
        }
        if self.nodes.is_empty() {
            return Ok(false);
        }
        let mins = start.min(end);
        let maxs = start.max(end);
        let mut pending = [0; STACK_SIZE];
        let mut count = 1;
        while count != 0 {
            count -= 1;
            let node = &self.nodes[pending[count]];
            consume(1)?;
            if mins.cmpgt(node.maxs).any() || maxs.cmplt(node.mins).any() {
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
                    consume(hull.plane_count() + 1)?;
                    if hull.intersects(start, end)? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
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
    fn shadow_index_enforces_cancellation_empty_rays_and_hull_limits() {
        assert!(
            ShadowIndex::new(vec![cube(DVec3::ZERO, 1.0)], &mut |_| Err(
                "cancelled".into()
            ))
            .is_err()
        );
        let empty = ShadowIndex::new(Vec::new(), &mut |_| Ok(())).unwrap();
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
