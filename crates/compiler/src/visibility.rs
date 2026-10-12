use std::collections::HashMap;

use map_core::Plane;
use source_formats::bsp::MAX_VISIBILITY_BYTES;

use crate::Control;
use crate::portals::Portal;
use crate::tree::Tree;
use crate::winding::Winding;

const MAX_CLUSTERS: usize = i16::MAX as usize + 1;
const MAX_VISIBILITY_WORK: usize = 100_000_000;
const VISIBILITY_EPSILON: f64 = 0.1;

pub(crate) struct Visibility {
    pub clusters: Vec<Option<usize>>,
    pub count: usize,
    pub portals: usize,
    pub data: Vec<u8>,
    pub sky: Vec<i16>,
}

struct DirectedPortal<'a> {
    from: usize,
    to: usize,
    plane: Plane,
    winding: &'a Winding,
}

struct Work {
    remaining: usize,
}

impl Work {
    fn consume(&mut self, count: usize) -> Result<(), String> {
        self.remaining = self.remaining.checked_sub(count).ok_or(
            "Visibility work limit exceeded; simplify structural geometry or use func_detail for non-sealing detail",
        )?;
        Ok(())
    }
}

pub(crate) fn build(
    tree: &Tree,
    portals: &[Portal],
    control: &Control,
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
    let data = solve(count, &directed, control, MAX_VISIBILITY_WORK)?;
    Ok(Visibility {
        clusters,
        count,
        portals: directed.len() / 2,
        data,
        sky: vec![0; count],
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
) -> Result<Vec<u8>, String> {
    control.checkpoint()?;
    let mut work = Work { remaining: budget };
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
    let mut eligible = vec![false; portals.len()];
    let mut reached = vec![false; count];
    let mut pending = Vec::new();
    for source in portals {
        control.checkpoint()?;
        for (index, target) in portals.iter().enumerate() {
            control.checkpoint()?;
            eligible[index] = target
                .winding
                .0
                .iter()
                .any(|&point| source.plane.distance_to(point) >= -VISIBILITY_EPSILON)
                && source
                    .winding
                    .0
                    .iter()
                    .any(|&point| target.plane.distance_to(point) <= VISIBILITY_EPSILON);
        }
        reached.fill(false);
        reached[source.to] = true;
        pending.push(source.to);
        while let Some(cluster) = pending.pop() {
            control.checkpoint()?;
            set(&mut rows[source.from], cluster);
            work.consume(edges[cluster].len())?;
            for &index in &edges[cluster] {
                let target = &portals[index];
                if eligible[index] && !reached[target.to] {
                    reached[target.to] = true;
                    pending.push(target.to);
                }
            }
        }
    }
    for a in 0..count {
        control.checkpoint()?;
        for b in a + 1..count {
            if bit(&rows[a], b) || bit(&rows[b], a) {
                set(&mut rows[a], b);
                set(&mut rows[b], a);
            }
        }
    }
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
        let data = solve(5, &portals, &Control::default(), MAX_VISIBILITY_WORK).unwrap();
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
        let data = solve(4, &portals, &Control::default(), MAX_VISIBILITY_WORK).unwrap();
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
    fn work_and_output_limits_fail_before_unbounded_work_and_cancellation_is_checked() {
        assert!(solve(100, &[], &Control::default(), 99).is_err());
        let control = Control::default();
        control.cancel().unwrap();
        assert!(solve(1, &[], &control, MAX_VISIBILITY_WORK).is_err());
        let mut data = vec![0; MAX_VISIBILITY_BYTES];
        assert!(store_row(&mut data, &mut HashMap::new(), &[1], 4).is_err());
    }
}
