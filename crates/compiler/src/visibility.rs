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

const MAX_CLUSTERS: usize = i16::MAX as usize + 1;
const MAX_VISIBILITY_WORK: usize = 100_000_000;
const VISIBILITY_EPSILON: f64 = 0.1;

pub(crate) struct Visibility {
    pub clusters: Vec<Option<usize>>,
    pub count: usize,
    pub portals: usize,
    pub data: Vec<u8>,
    pub sky: Vec<i16>,
    pub work: usize,
    pub removed_pairs: usize,
    pub fallbacks: usize,
    pub workers: usize,
}

struct Solution {
    data: Vec<u8>,
    work: usize,
    removed_pairs: usize,
    fallbacks: usize,
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

const WORK_LIMIT: &str = "Visibility work limit exceeded; simplify structural geometry or use func_detail for non-sealing detail";

impl Work {
    fn consume(&mut self, count: usize) -> Result<(), String> {
        match self {
            Self::Local(remaining) => {
                *remaining = remaining.checked_sub(count).ok_or(WORK_LIMIT)?
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

pub(crate) fn build(
    tree: &Tree,
    portals: &[Portal],
    control: &Control,
    mode: VisibilityMode,
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
    let solution = solve(count, &directed, control, MAX_VISIBILITY_WORK, mode)?;
    Ok(Visibility {
        clusters,
        count,
        portals: directed.len() / 2,
        data: solution.data,
        sky: vec![0; count],
        work: solution.work,
        removed_pairs: solution.removed_pairs,
        fallbacks: solution.fallbacks,
        workers: solution.workers,
    })
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
    work.consume(
        portals
            .len()
            .checked_mul(portals.len())
            .ok_or("Visibility work limit exceeded")?,
    )?;
    work.consume(
        count
            .checked_mul(count)
            .ok_or("Visibility work limit exceeded")?,
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
    let mut fallbacks = 0;
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
    .build(workers, &mut work)?;
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
                fallbacks += graph.refine(cluster, row, &mut work)?;
            }
        } else {
            let remaining = Arc::new(budget::Budget::new(work.remaining(), control.clone()));
            work = Work::Shared(budget::Lease::new(remaining.clone()));
            fallbacks = graph.parallel(refined, flow_workers, remaining)?;
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
    Ok(Solution {
        data,
        work: budget - work.remaining(),
        removed_pairs,
        fallbacks,
        workers,
    })
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

#[cfg(test)]
mod tests {
    use glam::DVec3;
    use source_formats::bsp::{Visibility as ParsedVisibility, VisibilityKind};

    use super::*;

    fn pair(from: usize, to: usize, plane: Plane, winding: &Winding) -> [DirectedPortal<'_>; 2] {
        [
            DirectedPortal {
                from,
                to,
                plane,
                winding,
            },
            DirectedPortal {
                from: to,
                to: from,
                plane: Plane {
                    normal: -plane.normal,
                    dist: -plane.dist,
                },
                winding,
            },
        ]
    }

    fn rectangle_x(x: f64, low: f64, high: f64) -> Winding {
        Winding(vec![
            DVec3::new(x, low, 0.0),
            DVec3::new(x, high, 0.0),
            DVec3::new(x, high, 2.0),
            DVec3::new(x, low, 2.0),
        ])
    }

    #[test]
    fn straight_portals_preserve_visibility_and_isolated_clusters_stay_isolated() {
        let windings = [
            rectangle_x(0.0, 0.0, 2.0),
            rectangle_x(10.0, 0.0, 2.0),
            rectangle_x(20.0, 0.0, 2.0),
        ];
        let mut portals = Vec::new();
        for (index, winding) in windings.iter().enumerate() {
            portals.extend(pair(
                index,
                index + 1,
                Plane {
                    normal: DVec3::X,
                    dist: index as f64 * 10.0,
                },
                winding,
            ));
        }
        let data = solve(
            5,
            &portals,
            &Control::default(),
            MAX_VISIBILITY_WORK,
            VisibilityMode::Fast,
        )
        .unwrap()
        .data;
        let visibility = ParsedVisibility::parse(&data).unwrap();
        for cluster in 0..4 {
            assert_eq!(
                visibility.row(cluster, VisibilityKind::Visible).unwrap(),
                [15]
            );
            assert_eq!(
                visibility.row(cluster, VisibilityKind::Audible).unwrap(),
                [15]
            );
        }
        assert_eq!(visibility.row(4, VisibilityKind::Visible).unwrap(), [16]);
        assert_eq!(visibility.row(4, VisibilityKind::Audible).unwrap(), [16]);
    }

    #[test]
    fn bent_portals_cull_occluded_clusters_while_audibility_expands_one_visible_step() {
        let first = rectangle_x(0.0, 0.0, 2.0);
        let second = Winding(vec![
            DVec3::new(-12.0, 10.0, 0.0),
            DVec3::new(2.0, 10.0, 0.0),
            DVec3::new(2.0, 10.0, 2.0),
            DVec3::new(-12.0, 10.0, 2.0),
        ]);
        let third = rectangle_x(-10.0, 12.0, 14.0);
        let mut portals = Vec::new();
        portals.extend(pair(
            0,
            1,
            Plane {
                normal: DVec3::X,
                dist: 0.0,
            },
            &first,
        ));
        portals.extend(pair(
            1,
            2,
            Plane {
                normal: DVec3::Y,
                dist: 10.0,
            },
            &second,
        ));
        portals.extend(pair(
            2,
            3,
            Plane {
                normal: -DVec3::X,
                dist: 10.0,
            },
            &third,
        ));
        let data = solve(
            4,
            &portals,
            &Control::default(),
            MAX_VISIBILITY_WORK,
            VisibilityMode::Fast,
        )
        .unwrap()
        .data;
        let visibility = ParsedVisibility::parse(&data).unwrap();
        let first = visibility.row(0, VisibilityKind::Visible).unwrap();
        assert!(!bit(&first, 3));
        assert!(bit(&first, 0) && bit(&first, 1) && bit(&first, 2));
        assert_eq!(visibility.row(0, VisibilityKind::Audible).unwrap(), [15]);
        for a in 0..4 {
            let visible = visibility.row(a, VisibilityKind::Visible).unwrap();
            let audible = visibility.row(a, VisibilityKind::Audible).unwrap();
            for b in 0..4 {
                assert_eq!(
                    bit(&visible, b),
                    bit(&visibility.row(b, VisibilityKind::Visible).unwrap(), a)
                );
                assert!(!bit(&visible, b) || bit(&audible, b));
            }
        }
    }

    #[test]
    fn encoding_handles_long_zero_runs_and_shares_identical_rows() {
        let mut row = vec![0; 600];
        row[0] = 1;
        row[599] = 128;
        assert_eq!(compress(&row), [1, 0, 255, 0, 255, 0, 88, 128]);
        let mut data = vec![0; 12];
        data[..4].copy_from_slice(&1_i32.to_le_bytes());
        let mut stored = HashMap::new();
        store_row(&mut data, &mut stored, &[1], 4).unwrap();
        store_row(&mut data, &mut stored, &[1], 8).unwrap();
        assert_eq!(data.len(), 13);
        assert_eq!(&data[4..8], &data[8..12]);
    }

    #[test]
    fn parallel_open_grid_preserves_both_modes_and_the_exact_global_budget() {
        const SIZE: usize = 10;
        let mut windings = Vec::new();
        let mut links = Vec::new();
        for y in 0..SIZE {
            for x in 0..SIZE {
                let from = y * SIZE + x;
                let low_x = x as f64 * 10.0;
                let low_y = y as f64 * 10.0;
                if x + 1 < SIZE {
                    windings.push(Winding(vec![
                        DVec3::new(low_x + 10.0, low_y, 0.0),
                        DVec3::new(low_x + 10.0, low_y + 10.0, 0.0),
                        DVec3::new(low_x + 10.0, low_y + 10.0, 10.0),
                        DVec3::new(low_x + 10.0, low_y, 10.0),
                    ]));
                    links.push((
                        from,
                        from + 1,
                        Plane {
                            normal: DVec3::X,
                            dist: low_x + 10.0,
                        },
                    ));
                }
                if y + 1 < SIZE {
                    windings.push(Winding(vec![
                        DVec3::new(low_x, low_y + 10.0, 0.0),
                        DVec3::new(low_x + 10.0, low_y + 10.0, 0.0),
                        DVec3::new(low_x + 10.0, low_y + 10.0, 10.0),
                        DVec3::new(low_x, low_y + 10.0, 10.0),
                    ]));
                    links.push((
                        from,
                        from + SIZE,
                        Plane {
                            normal: DVec3::Y,
                            dist: low_y + 10.0,
                        },
                    ));
                }
            }
        }
        let portals = links
            .iter()
            .zip(&windings)
            .flat_map(|(&(from, to, plane), winding)| pair(from, to, plane, winding))
            .collect::<Vec<_>>();
        for mode in [VisibilityMode::Fast, VisibilityMode::Detailed] {
            let started = std::time::Instant::now();
            let serial = solve_workers(
                SIZE * SIZE,
                &portals,
                &Control::default(),
                MAX_VISIBILITY_WORK,
                mode,
                1,
            )
            .unwrap();
            let serial_time = started.elapsed();
            let started = std::time::Instant::now();
            let solution = solve_workers(
                SIZE * SIZE,
                &portals,
                &Control::default(),
                MAX_VISIBILITY_WORK,
                mode,
                8,
            )
            .unwrap();
            eprintln!(
                "{mode:?} open grid: {} work, serial {:?}, eight workers {:?}",
                solution.work,
                serial_time,
                started.elapsed()
            );
            assert_eq!(solution.workers, 8);
            assert_eq!(solution.work, serial.work);
            assert_eq!(solution.data, serial.data);
            assert_eq!(solution.fallbacks, serial.fallbacks);
            let exact = solve_workers(
                SIZE * SIZE,
                &portals,
                &Control::default(),
                serial.work,
                mode,
                8,
            )
            .unwrap();
            assert_eq!(exact.work, serial.work);
            assert_eq!(exact.data, serial.data);
            assert!(
                solve_workers(
                    SIZE * SIZE,
                    &portals,
                    &Control::default(),
                    serial.work - 1,
                    mode,
                    8
                )
                .is_err()
            );
            let parsed = ParsedVisibility::parse(&solution.data).unwrap();
            for source in 0..SIZE * SIZE {
                let row = parsed.row(source, VisibilityKind::Visible).unwrap();
                assert!((0..SIZE * SIZE).all(|target| bit(&row, target)));
            }
        }
        let cancelled = Control::default();
        cancelled.cancel().unwrap();
        assert!(
            solve_workers(
                SIZE * SIZE,
                &portals,
                &cancelled,
                MAX_VISIBILITY_WORK,
                VisibilityMode::Fast,
                8,
            )
            .err()
            .unwrap()
            .contains("cancelled")
        );
    }

    #[test]
    fn separator_flow_culls_offset_openings_and_retains_alternate_and_narrow_routes() {
        let winding = [
            rectangle_x(0.0, 0.0, 2.0),
            rectangle_x(10.0, 8.0, 10.0),
            rectangle_x(20.0, 0.0, 2.0),
        ];
        let mut portals = Vec::new();
        for (index, winding) in winding.iter().enumerate() {
            portals.extend(pair(
                index,
                index + 1,
                Plane {
                    normal: DVec3::X,
                    dist: index as f64 * 10.0,
                },
                winding,
            ));
        }
        let fast = solve(
            4,
            &portals,
            &Control::default(),
            MAX_VISIBILITY_WORK,
            VisibilityMode::Fast,
        )
        .unwrap();
        let detailed = solve(
            4,
            &portals,
            &Control::default(),
            MAX_VISIBILITY_WORK,
            VisibilityMode::Detailed,
        )
        .unwrap();
        let fast = ParsedVisibility::parse(&fast.data).unwrap();
        let parsed = ParsedVisibility::parse(&detailed.data).unwrap();
        assert!(bit(&fast.row(0, VisibilityKind::Visible).unwrap(), 3));
        assert!(!bit(&parsed.row(0, VisibilityKind::Visible).unwrap(), 3));
        assert!(detailed.removed_pairs >= 2 && detailed.work > 0);
        let alternate = rectangle_x(10.0, 0.0, 2.0);
        portals.extend(pair(
            1,
            2,
            Plane {
                normal: DVec3::X,
                dist: 10.0,
            },
            &alternate,
        ));
        let data = solve(
            4,
            &portals,
            &Control::default(),
            MAX_VISIBILITY_WORK,
            VisibilityMode::Detailed,
        )
        .unwrap()
        .data;
        assert!(bit(
            &ParsedVisibility::parse(&data)
                .unwrap()
                .row(0, VisibilityKind::Visible)
                .unwrap(),
            3
        ));
        let narrow = [
            rectangle_x(0.0, 0.0, 0.0001),
            rectangle_x(0.00001, 0.0, 0.0001),
            rectangle_x(10.0, 0.0, 0.0001),
        ];
        let mut portals = Vec::new();
        for (index, winding) in narrow.iter().enumerate() {
            portals.extend(pair(
                index,
                index + 1,
                Plane {
                    normal: DVec3::X,
                    dist: winding.0[0].x,
                },
                winding,
            ));
        }
        let data = solve(
            4,
            &portals,
            &Control::default(),
            MAX_VISIBILITY_WORK,
            VisibilityMode::Detailed,
        )
        .unwrap()
        .data;
        assert!(bit(
            &ParsedVisibility::parse(&data)
                .unwrap()
                .row(0, VisibilityKind::Visible)
                .unwrap(),
            3
        ));
        assert!(
            solve(
                4,
                &portals,
                &Control::default(),
                100,
                VisibilityMode::Detailed
            )
            .is_err()
        );
    }

    #[test]
    fn work_and_output_limits_fail_before_unbounded_work_and_cancellation_is_checked() {
        assert!(solve(100, &[], &Control::default(), 99, VisibilityMode::Fast).is_err());
        let control = Control::default();
        control.cancel().unwrap();
        assert!(solve(1, &[], &control, MAX_VISIBILITY_WORK, VisibilityMode::Fast).is_err());
        let mut data = vec![0; MAX_VISIBILITY_BYTES];
        assert!(store_row(&mut data, &mut HashMap::new(), &[1], 4).is_err());
    }
}
