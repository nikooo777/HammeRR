use std::collections::HashMap;
use std::ops::Range;

use glam::DVec3;

use super::{Source, compatible, normal_key};
use crate::lighting::bake::Budget;

const EDGE_EPSILON: f64 = 1e-6;
const LEAF_SIZE: usize = 8;
const STACK_SIZE: usize = usize::BITS as usize;

struct Point {
    position: DVec3,
    owners: Vec<usize>,
}

struct Node {
    mins: DVec3,
    maxs: DVec3,
    range: Range<usize>,
    children: Option<[usize; 2]>,
}

struct Index {
    points: Vec<Point>,
    order: Vec<usize>,
    nodes: Vec<Node>,
}

impl Index {
    fn build(&mut self, range: Range<usize>, budget: &mut Budget<'_>) -> Result<usize, String> {
        budget.consume(range.len())?;
        let mut mins = DVec3::splat(f64::INFINITY);
        let mut maxs = DVec3::splat(f64::NEG_INFINITY);
        for &index in &self.order[range.clone()] {
            mins = mins.min(self.points[index].position);
            maxs = maxs.max(self.points[index].position);
        }
        let index = self.nodes.len();
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
            budget.consume(range.len() * (range.len().ilog2() as usize + 1))?;
            self.order[range.clone()].sort_unstable_by(|&a, &b| {
                self.points[a].position[axis]
                    .total_cmp(&self.points[b].position[axis])
                    .then(a.cmp(&b))
            });
            let middle = range.start + range.len() / 2;
            let left = self.build(range.start..middle, budget)?;
            let right = self.build(middle..range.end, budget)?;
            self.nodes[index].children = Some([left, right]);
        }
        Ok(index)
    }

    fn candidates(
        &self,
        a: DVec3,
        b: DVec3,
        budget: &mut Budget<'_>,
        visit: &mut impl FnMut(&Point, f64, &mut Budget<'_>) -> Result<(), String>,
    ) -> Result<(), String> {
        let delta = b - a;
        let length = delta.length();
        if length <= 2.0 * EDGE_EPSILON || self.nodes.is_empty() {
            return Ok(());
        }
        let mins = a.min(b) - DVec3::splat(EDGE_EPSILON);
        let maxs = a.max(b) + DVec3::splat(EDGE_EPSILON);
        let mut stack = [0; STACK_SIZE];
        let mut count = 1;
        while count != 0 {
            count -= 1;
            let node = &self.nodes[stack[count]];
            budget.consume(1)?;
            if node.mins.cmpgt(maxs).any() || node.maxs.cmplt(mins).any() {
                continue;
            }
            if let Some(children) = node.children {
                if count + 2 > stack.len() {
                    return Err("Brush smoothing junction lookup exceeds its stack budget".into());
                }
                stack[count..count + 2].copy_from_slice(&children);
                count += 2;
                continue;
            }
            for &index in &self.order[node.range.clone()] {
                budget.consume(1)?;
                let point = &self.points[index];
                let fraction = (point.position - a).dot(delta) / delta.length_squared();
                if fraction * length <= EDGE_EPSILON
                    || (1.0 - fraction) * length <= EDGE_EPSILON
                    || point.position.distance_squared(a + delta * fraction)
                        > EDGE_EPSILON * EDGE_EPSILON
                {
                    continue;
                }
                visit(point, fraction, budget)?;
            }
        }
        Ok(())
    }
}

pub(super) fn insert(
    sources: &mut [Source],
    threshold: Option<f64>,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    let mut index = Index {
        points: Vec::new(),
        order: Vec::new(),
        nodes: Vec::new(),
    };
    let mut keys = HashMap::new();
    let mut retained = 0usize;
    for (owner, source) in sources.iter().enumerate() {
        budget.consume(source.points.len())?;
        retained = retained
            .checked_add(source.points.len())
            .filter(|&count| count <= crate::faces::MAX_SMOOTH_SOURCE_POINTS)
            .ok_or("Brush smoothing junctions exceed the four-million source-point budget")?;
        for &position in &source.points {
            let key = normal_key(position.as_vec3().to_array());
            let point = *keys.entry(key).or_insert_with(|| {
                let point = index.points.len();
                index.points.push(Point {
                    position,
                    owners: Vec::new(),
                });
                point
            });
            if index.points[point].owners.last() != Some(&owner) {
                index.points[point].owners.push(owner);
            }
        }
    }
    index.order = (0..index.points.len()).collect();
    if !index.points.is_empty() {
        index.build(0..index.points.len(), budget)?;
    }
    let mut augmented = Vec::with_capacity(sources.len());
    for (owner, source) in sources.iter().enumerate() {
        let mut points = Vec::with_capacity(source.points.len());
        for edge in 0..source.points.len() {
            let a = source.points[edge];
            let b = source.points[(edge + 1) % source.points.len()];
            points.push(a);
            let mut interior = Vec::new();
            index.candidates(a, b, budget, &mut |point, fraction, budget| {
                budget.consume(point.owners.len())?;
                if point
                    .owners
                    .iter()
                    .any(|&other| other != owner && compatible(source, &sources[other], threshold))
                {
                    retained = retained
                        .checked_add(1)
                        .filter(|&count| count <= crate::faces::MAX_SMOOTH_SOURCE_POINTS)
                        .ok_or(
                            "Brush smoothing junctions exceed the four-million source-point budget",
                        )?;
                    interior.push((fraction, point.position));
                }
                Ok(())
            })?;
            if !interior.is_empty() {
                budget.consume(interior.len() * (interior.len().ilog2() as usize + 1))?;
                interior.sort_unstable_by(|a, b| {
                    a.0.total_cmp(&b.0).then_with(|| {
                        normal_key(a.1.as_vec3().to_array())
                            .cmp(&normal_key(b.1.as_vec3().to_array()))
                    })
                });
                points.extend(interior.into_iter().map(|(_, point)| point));
            }
        }
        augmented.push(points);
    }
    for (source, points) in sources.iter_mut().zip(augmented) {
        source.points = points;
    }
    Ok(())
}
