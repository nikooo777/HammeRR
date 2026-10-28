use std::collections::HashMap;
use std::sync::Arc;

use map_core::Plane;
use source_formats::bsp::MAX_VISIBILITY_BYTES;

use crate::Control;
use crate::VisibilityMode;
use crate::portals::Portal;
use crate::tree::Tree;
use crate::winding::Winding;

mod base;
mod budget;
mod flow;
mod parallel;
mod water;

const MAX_CLUSTERS: usize = i16::MAX as usize + 1;
pub(crate) const MAX_VISIBILITY_WORK: usize = 100_000_000;
const VISIBILITY_EPSILON: f64 = 0.1;

pub(crate) struct Visibility {
    pub clusters: Vec<Option<usize>>,
    pub count: usize,
    pub portals: usize,
    pub data: Vec<u8>,
    pub sky: Vec<i16>,
    pub work: usize,
    pub removed_pairs: usize,
    pub workers: usize,
}

struct Solution {
    data: Vec<u8>,
    work: usize,
    removed_pairs: usize,
    workers: usize,
}

struct DirectedPortal<'a> {
    from: usize,
    to: usize,
    plane: Plane,
    winding: &'a Winding,
}

enum Work {
    Local(usize),
    Shared(budget::Lease),
}

const WORK_LIMIT: &str = "Visibility work limit exceeded";

fn work_limit(count: usize, remaining: usize) -> String {
    format!("{WORK_LIMIT}; next charge {count} work units, {remaining} remaining")
}

impl Work {
    fn consume(&mut self, count: usize) -> Result<(), String> {
        match self {
            Self::Local(remaining) => {
                *remaining = remaining
                    .checked_sub(count)
                    .ok_or_else(|| work_limit(count, *remaining))?
            }
            Self::Shared(remaining) => {
                remaining.consume(count)?;
            }
        }
        Ok(())
    }

    fn remaining(&self) -> usize {
        match self {
            Self::Local(remaining) => *remaining,
            Self::Shared(remaining) => remaining.remaining(),
        }
    }
}

fn build_budgeted(
    tree: &Tree,
    portals: &[Portal],
    control: &Control,
    mode: VisibilityMode,
    budget: usize,
) -> Result<Visibility, String> {
    let mut clusters = vec![None; tree.nodes.len()];
    let mut count = 0;
    for index in tree.preorder() {
        control.checkpoint()?;
        let node = &tree.nodes[index];
        if node.split.is_none() && !node.solid() {
            if count >= MAX_CLUSTERS {
                return Err(format!(
                    "BSP visibility requires at most {MAX_CLUSTERS} world clusters"
                ));
            }
            clusters[index] = Some(count);
            count += 1;
        }
    }
    if count == 0 {
        return Err("No open world cluster remains after leak checking".into());
    }
    let mut directed = Vec::new();
    for portal in portals {
        control.checkpoint()?;
        flow::validate(&portal.winding)?;
        let [a, b] = portal
            .leaves
            .map(|index| clusters.get(index).copied().flatten());
        let (Some(a), Some(b)) = (a, b) else { continue };
        if a == b {
            continue;
        }
        let points = &portal.winding.0;
        let plane = (1..points.len().saturating_sub(1))
            .find_map(|index| Plane::from_points([points[0], points[index], points[index + 1]]))
            .ok_or("Visibility portal has no stable plane")?;
        directed.push(DirectedPortal {
            from: a,
            to: b,
            plane: Plane {
                normal: -plane.normal,
                dist: -plane.dist,
            },
            winding: &portal.winding,
        });
        directed.push(DirectedPortal {
            from: b,
            to: a,
            plane,
            winding: &portal.winding,
        });
    }
    let solution = solve(count, &directed, control, budget, mode)?;
    Ok(Visibility {
        clusters,
        count,
        portals: directed.len() / 2,
        data: solution.data,
        sky: vec![0; count],
        work: solution.work,
        removed_pairs: solution.removed_pairs,
        workers: solution.workers,
    })
}

pub(crate) fn build_authored(
    context: crate::visclusters::Context<'_>,
    portals: &[Portal],
    report: &mut crate::Report,
    mode: VisibilityMode,
) -> Result<(Visibility, crate::visclusters::Summary), String> {
    build_authored_budgeted(context, portals, report, mode, MAX_VISIBILITY_WORK)
}

fn build_authored_budgeted(
    context: crate::visclusters::Context<'_>,
    portals: &[Portal],
    report: &mut crate::Report,
    mode: VisibilityMode,
    budget: usize,
) -> Result<(Visibility, crate::visclusters::Summary), String> {
    build_authored_inner(context, portals, report, mode, budget).map_err(|error| {
        if !error.starts_with(WORK_LIMIT) {
            return error;
        }
        let map = context.map;
        let (structural_brushes, structural_sides, liquid_brushes) = map.entities[0]
            .brushes
            .iter()
            .map(|&index| &map.brushes[index])
            .filter(|brush| brush.structural())
            .fold((0, 0, 0), |(brushes, sides, liquid), brush| {
                (brushes + 1,
                 sides + brush.sides.iter().filter(|side| !side.bevel).count(),
                 liquid + usize::from(brush.contents & crate::water::LIQUID_CONTENTS != 0))
            });
        let open_leaves = context.tree.preorder().into_iter().filter(|&index| {
            let node = &context.tree.nodes[index];
            node.split.is_none() && !node.solid()
        }).count();
        let liquid_leaves = context.water.leaves.iter().filter(|&&region| region >= 0).count();
        format!(
            "{error}; visibility work limit {budget} units; \
             captured geometry after flood/hint partitioning: {open_leaves} open world leaves, \
             {structural_brushes} structural world brush inputs ({structural_sides} non-bevel sides), \
             {liquid_brushes} structural liquid brushes, {liquid_leaves} liquid leaves in {} liquid regions; \
             {} portal records before open-leaf filtering; authored viscluster preparation recorded {} work units. \
             Counts describe input size; per-brush cost attribution is unavailable. \
             Use func_detail only for non-sealing geometry; liquid leaves are excluded from authored viscluster grouping, \
             and grouping does not reduce solver cost",
            context.water.records.len(),
            portals.len(),
            map.viscluster_work,
        )
    })
}

fn build_authored_inner(
    context: crate::visclusters::Context<'_>,
    portals: &[Portal],
    report: &mut crate::Report,
    mode: VisibilityMode,
    budget: usize,
) -> Result<(Visibility, crate::visclusters::Summary), String> {
    let crate::visclusters::Context {
        map, tree, control, ..
    } = context;
    if map.visclusters.is_empty() {
        let visibility = build_budgeted(tree, portals, control, mode, budget)?;
        let summary = crate::visclusters::Summary {
            solved_clusters: visibility.count,
            ..Default::default()
        };
        return Ok((visibility, summary));
    }
    let mut work = Work::Local(budget);
    work.consume(map.viscluster_work)?;
    let mut visibility = build_budgeted(tree, portals, control, mode, work.remaining())?;
    work.consume(visibility.work)?;
    let partition = crate::visclusters::partition(
        context,
        &visibility.clusters,
        visibility.count,
        report,
        &mut |amount| {
            control.checkpoint()?;
            work.consume(amount)
        },
    )?;
    if partition.count < visibility.count {
        let rows = quotient(
            &visibility.data,
            &partition.mapping,
            partition.count,
            control,
            &mut work,
        )?;
        visibility.data = encode_rows(&rows, control, &mut work)?;
        for cluster in visibility.clusters.iter_mut().flatten() {
            *cluster = partition.mapping[*cluster];
        }
        visibility.count = partition.count;
        visibility.sky = vec![0; partition.count];
    }
    visibility.work = budget - work.remaining();
    report.diagnostics.push(crate::Diagnostic::info(
        format!("Authored visclusters solved {} leaf clusters and emitted {} clusters ({} helper groups, {} member leaves); coarsening may expand visibility and does not reduce solver cost", partition.summary.solved_clusters, visibility.count, partition.summary.groups, partition.summary.grouped_leaves), None, None,
    ));
    Ok((visibility, partition.summary))
}

fn quotient(
    data: &[u8],
    mapping: &[usize],
    count: usize,
    control: &Control,
    work: &mut Work,
) -> Result<Vec<Vec<u8>>, String> {
    use source_formats::bsp::{Visibility as ParsedVisibility, VisibilityKind};
    let parsed = ParsedVisibility::parse(data).map_err(|error| error.to_string())?;
    if parsed.clusters() != mapping.len()
        || count == 0
        || mapping.iter().any(|&group| group >= count)
    {
        return Err("Viscluster quotient mapping disagrees with solved visibility".into());
    }
    work.consume(mapping.len().checked_mul(mapping.len()).ok_or(WORK_LIMIT)?)?;
    let mut rows = vec![vec![0; count.div_ceil(8)]; count];
    for (source, &group) in mapping.iter().enumerate() {
        control.checkpoint()?;
        let row = parsed
            .row(source, VisibilityKind::Visible)
            .map_err(|error| error.to_string())?;
        for (target, &destination) in mapping.iter().enumerate() {
            if bit(&row, target) {
                set(&mut rows[group], destination);
            }
        }
    }
    Ok(rows)
}

impl Visibility {
    pub(crate) fn mark_sky(
        &mut self,
        map: &crate::map::Map,
        faces: &[crate::faces::Face],
        control: &Control,
    ) -> Result<(), String> {
        use source_formats::bsp::{
            Visibility as ParsedVisibility, VisibilityKind, leaf_flags, surf,
        };
        let mut direct = vec![0_i16; self.count];
        for face in faces {
            control.checkpoint()?;
            let flags = map.texinfos.infos[face.texinfo].flags;
            if flags & surf::SKY == 0 {
                continue;
            }
            let sky = if flags & surf::SKY2D != 0 {
                leaf_flags::SKY2D
            } else {
                leaf_flags::SKY
            };
            for &leaf in &face.leaves {
                if let Some(cluster) = self.clusters[leaf] {
                    direct[cluster] |= sky;
                }
            }
        }
        let parsed = ParsedVisibility::parse(&self.data).map_err(|error| error.to_string())?;
        for (cluster, flags) in self.sky.iter_mut().enumerate() {
            control.checkpoint()?;
            *flags = 0;
            let row = parsed
                .row(cluster, VisibilityKind::Visible)
                .map_err(|error| error.to_string())?;
            for (other, &sky) in direct.iter().enumerate() {
                if sky != 0 && bit(&row, other) {
                    *flags |= sky;
                }
            }
            if *flags & leaf_flags::SKY != 0 {
                *flags &= !leaf_flags::SKY2D;
            }
        }
        Ok(())
    }
}

fn bit(row: &[u8], index: usize) -> bool {
    row[index / 8] & (1 << (index % 8)) != 0
}

fn set(row: &mut [u8], index: usize) {
    row[index / 8] |= 1 << (index % 8);
}

fn solve(
    count: usize,
    portals: &[DirectedPortal<'_>],
    control: &Control,
    budget: usize,
    mode: VisibilityMode,
) -> Result<Solution, String> {
    let workers = if portals.len() >= 64 {
        std::thread::available_parallelism().map_or(1, |count| count.get().min(8))
    } else {
        1
    };
    solve_workers(count, portals, control, budget, mode, workers)
}

fn solve_workers(
    count: usize,
    portals: &[DirectedPortal<'_>],
    control: &Control,
    budget: usize,
    mode: VisibilityMode,
    requested_workers: usize,
) -> Result<Solution, String> {
    control.checkpoint()?;
    let mut work = Work::Local(budget);
    solve_work(count, portals, control, mode, requested_workers, &mut work).map_err(|error| {
        if error.starts_with(WORK_LIMIT) {
            format!("{error}; solver charged {} of {budget} available work units; {count} input leaf clusters, {} directed portals ({} portal pairs), mode {}", budget - work.remaining(), portals.len(), portals.len() / 2, mode.name())
        } else {
            error
        }
    })
}

fn solve_work(
    count: usize,
    portals: &[DirectedPortal<'_>],
    control: &Control,
    mode: VisibilityMode,
    requested_workers: usize,
    work: &mut Work,
) -> Result<Solution, String> {
    let budget = work.remaining();
    work.consume(
        portals
            .len()
            .checked_mul(portals.len())
            .ok_or_else(|| format!("{WORK_LIMIT}; directed-portal pair count overflow"))?,
    )?;
    work.consume(
        count
            .checked_mul(count)
            .ok_or_else(|| format!("{WORK_LIMIT}; cluster pair count overflow"))?,
    )?;
    let width = count.div_ceil(8);
    let mut rows = vec![vec![0; width]; count];
    let mut edges = vec![Vec::new(); count];
    for (index, portal) in portals.iter().enumerate() {
        edges[portal.from].push(index);
    }
    for (index, row) in rows.iter_mut().enumerate() {
        set(row, index);
    }
    let mut refined = (mode == VisibilityMode::Detailed).then(|| rows.clone());
    let mut workers = if portals.len() >= 64 {
        requested_workers.clamp(1, 8).min(portals.len())
    } else {
        1
    };
    let base = base::Graph {
        portals,
        edges: &edges,
        control,
        detailed: refined.is_some(),
    }
    .build(workers, work)?;
    let mut fronts = refined.as_ref().map(|_| Vec::with_capacity(portals.len()));
    let mut bounds = refined.as_ref().map(|_| Vec::with_capacity(portals.len()));
    for row in base {
        control.checkpoint()?;
        for (target, &byte) in rows[portals[row.source].from].iter_mut().zip(&row.clusters) {
            *target |= byte;
        }
        if let (Some(fronts), Some(bounds), Some(front)) = (&mut fronts, &mut bounds, row.fronts) {
            fronts.push(front);
            bounds.push(row.clusters);
        }
    }
    if let (Some(refined), Some(fronts), Some(bounds)) = (&mut refined, fronts, bounds) {
        let graph = parallel::Graph {
            portals,
            edges: &edges,
            fronts: &fronts,
            bounds: &bounds,
            control,
        };
        let flow_workers = requested_workers.clamp(1, 8).min(
            edges
                .iter()
                .filter(|edges| !edges.is_empty())
                .count()
                .max(1),
        );
        workers = workers.max(flow_workers);
        if flow_workers == 1 {
            for (cluster, row) in refined.iter_mut().enumerate() {
                graph.refine(cluster, row, work)?;
            }
        } else {
            let remaining = Arc::new(budget::Budget::new(work.remaining(), control.clone()));
            *work = Work::Shared(budget::Lease::new(remaining.clone()));
            graph.parallel(refined, flow_workers, remaining)?;
        }
    }

    for a in 0..count {
        control.checkpoint()?;
        for b in a + 1..count {
            if bit(&rows[a], b) || bit(&rows[b], a) {
                set(&mut rows[a], b);
                set(&mut rows[b], a);
            }
            if let Some(refined) = &mut refined
                && (bit(&refined[a], b) || bit(&refined[b], a))
            {
                set(&mut refined[a], b);
                set(&mut refined[b], a);
            }
        }
    }
    let removed_pairs = if let Some(refined) = refined {
        let removed = rows
            .iter()
            .flatten()
            .map(|byte| byte.count_ones() as usize)
            .sum::<usize>()
            - refined
                .iter()
                .flatten()
                .map(|byte| byte.count_ones() as usize)
                .sum::<usize>();
        rows = refined;
        removed
    } else {
        0
    };
    let data = encode_rows(&rows, control, work)?;
    Ok(Solution {
        data,
        work: budget - work.remaining(),
        removed_pairs,
        workers,
    })
}

fn encode_rows(rows: &[Vec<u8>], control: &Control, work: &mut Work) -> Result<Vec<u8>, String> {
    let count = rows.len();
    let width = count.div_ceil(8);
    let header = 4 + count * 8;
    let mut data = vec![0; header];
    data[..4].copy_from_slice(&(count as i32).to_le_bytes());
    let mut stored = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        control.checkpoint()?;
        store_row(&mut data, &mut stored, row, 4 + index * 8)?;
    }
    let mut audible = vec![0; width];
    for (index, row) in rows.iter().enumerate() {
        control.checkpoint()?;
        audible.fill(0);
        for (other, visible) in rows.iter().enumerate() {
            if bit(row, other) {
                work.consume(width.div_ceil(8))?;
                for (destination, source) in audible.iter_mut().zip(visible) {
                    *destination |= source;
                }
            }
        }
        store_row(&mut data, &mut stored, &audible, 8 + index * 8)?;
    }
    Ok(data)
}

fn compress(row: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::new();
    let mut index = 0;
    while index < row.len() {
        let value = row[index];
        encoded.push(value);
        index += 1;
        if value == 0 {
            let start = index - 1;
            while index < row.len() && row[index] == 0 && index - start < u8::MAX as usize {
                index += 1;
            }
            encoded.push((index - start) as u8);
        }
    }
    encoded
}

fn store_row(
    data: &mut Vec<u8>,
    stored: &mut HashMap<Vec<u8>, i32>,
    row: &[u8],
    header: usize,
) -> Result<(), String> {
    let encoded = compress(row);
    let offset = if let Some(&offset) = stored.get(&encoded) {
        offset
    } else {
        if encoded.len() > MAX_VISIBILITY_BYTES.saturating_sub(data.len()) {
            return Err(
                "BSP visibility exceeds the 16 MiB lump limit; simplify structural geometry".into(),
            );
        }
        let offset = data.len() as i32;
        data.extend_from_slice(&encoded);
        stored.insert(encoded, offset);
        offset
    };
    data[header..header + 4].copy_from_slice(&offset.to_le_bytes());
    Ok(())
}
