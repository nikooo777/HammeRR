use glam::DVec3;
use std::collections::BTreeSet;

use crate::map::{Map, MapSide};
use crate::planes::PlaneSet;
use crate::tree::{Polyhedron, Tree};
use crate::winding::{ON_EPSILON, Side, Winding};
use crate::{Control, Diagnostic, Report};

const MAX_HINT_SURFACES: usize = 4096;
const MAX_HINT_SPLITS: usize = 16_384;
const MAX_HINT_WORK: usize = 16_000_000;

pub(crate) struct Hint {
    solid: u32,
    side: u32,
    plane: usize,
    winding: Winding,
    bounds: (DVec3, DVec3),
}

#[derive(Default)]
pub(crate) struct Stats {
    pub splits: usize,
    pub work: usize,
    pub priority_splits: usize,
}

pub(crate) struct Fragment {
    pub source: usize,
    pub plane: usize,
    pub winding: Winding,
}

#[derive(Default)]
pub(crate) struct Priority {
    stats: Stats,
    nodes: Vec<(usize, Vec<usize>)>,
}

impl Priority {
    pub fn consume(&mut self, amount: usize) -> Result<(), String> {
        self.stats.consume(amount)
    }

    pub fn fragments(
        &mut self,
        hints: &[Hint],
        volume: &Polyhedron,
        planes: &PlaneSet,
        control: &Control,
    ) -> Result<Vec<Fragment>, String> {
        let mut fragments = Vec::new();
        for (source, hint) in hints.iter().enumerate() {
            let mut winding = Some(hint.winding.clone());
            for side in &volume.sides {
                control.checkpoint()?;
                let Some(current) = winding else {
                    break;
                };
                self.consume(current.0.len())?;
                let plane = planes.get(side.plane);
                winding = current.clip_back(plane.normal, plane.dist);
            }
            if let Some(winding) = winding
                && winding.area() > ON_EPSILON.powi(2)
            {
                fragments.push(Fragment {
                    source,
                    plane: hint.plane,
                    winding,
                });
            }
        }
        Ok(fragments)
    }

    pub fn split(
        &mut self,
        hints: Vec<Fragment>,
        plane: usize,
        planes: &PlaneSet,
        control: &Control,
    ) -> Result<[Vec<Fragment>; 2], String> {
        let mut halves = [Vec::new(), Vec::new()];
        let split = planes.get(plane);
        for hint in hints {
            control.checkpoint()?;
            self.consume(hint.winding.0.len())?;
            if hint.plane == plane {
                continue;
            }
            let [front, back] = {
                let (front, back) = hint.winding.split(split.normal, split.dist);
                [front, back]
            };
            for (half, winding) in halves.iter_mut().zip([front, back]) {
                if let Some(winding) = winding
                    && winding.area() > ON_EPSILON.powi(2)
                {
                    half.push(Fragment {
                        winding,
                        source: hint.source,
                        plane: hint.plane,
                    });
                }
            }
        }
        Ok(halves)
    }

    pub fn record(&mut self, node: usize, plane: usize, hints: &[Fragment]) -> Result<(), String> {
        let sources = hints
            .iter()
            .filter(|hint| hint.plane == plane)
            .map(|hint| hint.source)
            .collect::<Vec<_>>();
        if sources.is_empty() {
            return Ok(());
        }
        self.consume(sources.len())?;
        if self.nodes.len() == MAX_HINT_SPLITS {
            return Err("Hint priority exceeds its 16,384-split allocation budget".into());
        }
        self.nodes.push((node, sources));
        Ok(())
    }

    fn retained(mut self, tree: &Tree) -> Result<(Stats, BTreeSet<usize>), String> {
        if self.nodes.is_empty() {
            return Ok((self.stats, BTreeSet::new()));
        }
        let reached = tree.preorder().into_iter().collect::<BTreeSet<_>>();
        self.consume(reached.len())?;
        let mut sources = BTreeSet::new();
        for (node, hints) in self.nodes {
            if reached.contains(&node) && tree.nodes[node].split.is_some() {
                self.stats.splits += 1;
                sources.extend(hints);
            }
        }
        self.stats.priority_splits = self.stats.splits;
        Ok((self.stats, sources))
    }
}

pub(crate) fn collect(
    map: &mut Map,
    id: u32,
    entity: usize,
    extra_contents: i32,
    sides: &[MapSide],
) -> Result<(), String> {
    if entity != 0 || extra_contents != 0 {
        return Err(
            "Hint brushes must be world brushes; move this helper out of its entity".into(),
        );
    }
    if sides.iter().any(|side| {
        let material = map.materials.get(side.material);
        (!material.hint && !material.skip) || material.contents != 0
    }) {
        return Err("Hint brushes require only hint/skip materials; use tools/toolsskip on the bounds and tools/toolshint on the intended split faces".into());
    }
    let count = sides
        .iter()
        .filter(|side| map.materials.get(side.material).hint)
        .count();
    if map.hints.len() + count > MAX_HINT_SURFACES {
        return Err("Hint surfaces exceed their 4,096-surface allocation budget".into());
    }
    for side in sides
        .iter()
        .filter(|side| map.materials.get(side.material).hint)
    {
        let winding = side
            .winding
            .clone()
            .ok_or("Hint side has no supported polygon")?;
        let bounds = winding
            .0
            .iter()
            .fold((DVec3::MAX, DVec3::MIN), |(low, high), &point| {
                (low.min(point), high.max(point))
            });
        map.hints.push(Hint {
            solid: id,
            side: side.source_id.ok_or("Hint side has no source ID")?,
            plane: side.plane & !1,
            winding,
            bounds,
        });
    }
    Ok(())
}

impl Stats {
    fn consume(&mut self, count: usize) -> Result<(), String> {
        self.work = self
            .work
            .checked_add(count)
            .ok_or("Hint work count overflow")?;
        if self.work > MAX_HINT_WORK {
            return Err("Hint partitioning exceeds its sixteen-million work-unit budget; reduce hint scope or helper count".into());
        }
        Ok(())
    }
}

pub(crate) fn apply(
    map: &Map,
    tree: &mut Tree,
    report: &mut Report,
    control: &Control,
    priority: Priority,
) -> Result<Stats, String> {
    let (mut stats, prioritized) = priority.retained(tree)?;
    for (source, hint) in map.hints.iter().enumerate() {
        control.checkpoint()?;
        let before = stats.splits;
        for index in tree.preorder() {
            control.checkpoint()?;
            stats.consume(1)?;
            let node = &tree.nodes[index];
            if node.split.is_some() || node.solid() {
                continue;
            }
            stats.consume(
                node.volume
                    .sides
                    .iter()
                    .map(|side| side.winding.0.len())
                    .sum(),
            )?;
            let (low, high) = node.volume.bounds();
            if !hint.bounds.0.cmple(high + DVec3::splat(ON_EPSILON)).all()
                || !hint.bounds.1.cmpge(low - DVec3::splat(ON_EPSILON)).all()
            {
                continue;
            }
            if node.volume.classify(hint.plane, &map.planes) != Side::Cross {
                continue;
            }
            if !intersects(&hint.winding, &node.volume, map, &mut stats, control)? {
                continue;
            }
            if stats.splits >= MAX_HINT_SPLITS {
                return Err("Hint partitioning exceeds its 16,384-split allocation budget".into());
            }
            tree.split_leaf(index, hint.plane, &map.planes)?;
            stats.splits += 1;
        }
        if stats.splits == before && !prioritized.contains(&source) {
            report.diagnostics.push(Diagnostic::info(
                format!("Hint side {} needs no additional split: its plane is already present or its polygon misses non-solid leaf interiors", hint.side),
                Some(format!("solid:{}", hint.solid)),
                Some((hint.bounds.0 + hint.bounds.1) * 0.5),
            ));
        }
    }
    Ok(stats)
}

fn intersects(
    winding: &Winding,
    volume: &Polyhedron,
    map: &Map,
    stats: &mut Stats,
    control: &Control,
) -> Result<bool, String> {
    let mut clipped = winding.clone();
    for side in &volume.sides {
        control.checkpoint()?;
        stats.consume(clipped.0.len())?;
        let plane = map.planes.get(side.plane);
        let Some(next) = clipped.clip_back(plane.normal, plane.dist) else {
            return Ok(false);
        };
        clipped = next;
    }
    Ok(clipped.area() > ON_EPSILON * ON_EPSILON)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_checks_work_before_clipping_and_observes_cancellation() {
        let mut planes = PlaneSet::default();
        let plane = planes.find(DVec3::X, 0.0);
        let fragment = || Fragment {
            source: 0,
            plane,
            winding: Winding(vec![
                DVec3::new(0.0, -1.0, -1.0),
                DVec3::new(0.0, 1.0, -1.0),
                DVec3::new(0.0, 1.0, 1.0),
                DVec3::new(0.0, -1.0, 1.0),
            ]),
        };
        let mut priority = Priority::default();
        priority.stats.work = MAX_HINT_WORK - 3;
        assert!(
            priority
                .split(vec![fragment()], plane, &planes, &Control::default())
                .err()
                .unwrap()
                .contains("work-unit budget")
        );
        let cancelled = Control::default();
        cancelled.cancel().unwrap();
        assert!(
            Priority::default()
                .split(vec![fragment()], plane, &planes, &cancelled)
                .err()
                .unwrap()
                .contains("cancelled")
        );
    }
}
