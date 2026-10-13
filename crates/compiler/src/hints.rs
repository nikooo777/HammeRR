use glam::DVec3;

use crate::map::{Map, MapSide};
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
) -> Result<Stats, String> {
    let mut stats = Stats::default();
    for hint in &map.hints {
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
        if stats.splits == before {
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
