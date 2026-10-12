use std::collections::HashMap;

use glam::DVec3;
use source_formats::bsp::{self, contents, surf};

use crate::faces::Face;
use crate::map::Map;
use crate::portals::Portal;
use crate::tree::Tree;
use crate::{Control, Diagnostic, Report};

const LIQUID_CONTENTS: i32 = contents::WATER | contents::SLIME;
const SURFACE_EPSILON: f64 = 0.01;

pub(crate) struct Water {
    pub leaves: Vec<i16>,
    pub records: Vec<bsp::LeafWaterData>,
    surfaces: Vec<(DVec3, DVec3)>,
}

#[derive(Clone, Copy)]
struct Candidate {
    surface: f64,
    texinfo: usize,
    material: usize,
    contents: i32,
    bottom: f64,
}

impl Candidate {
    fn compatible(self, other: Self) -> bool {
        self.surface == other.surface
            && self.material == other.material
            && self.contents == other.contents
    }
}

pub(crate) fn build(
    map: &Map,
    tree: &Tree,
    portals: &[Portal],
    report: &mut Report,
    control: &Control,
) -> Result<Water, String> {
    let mut water = Water {
        leaves: vec![-1; tree.nodes.len()],
        records: Vec::new(),
        surfaces: Vec::new(),
    };
    let mut brushes = vec![None; map.brushes.len()];
    for (index, brush) in map.brushes.iter().enumerate() {
        control.checkpoint()?;
        if brush.contents & LIQUID_CONTENTS == 0 {
            continue;
        }
        if brush.entity != 0 || !brush.structural() {
            report.diagnostics.push(Diagnostic::warning("Liquid leaf metadata currently requires structural world brushes; this detail or brush-model fluid has no native water region", Some(format!("solid:{}", brush.id)), Some((brush.mins + brush.maxs) * 0.5)));
            continue;
        }
        let top = brush
            .sides
            .iter()
            .filter(|side| {
                !side.bevel
                    && side.visible
                    && map.materials.get(side.material).contents & LIQUID_CONTENTS != 0
            })
            .find(|side| {
                map.planes
                    .get(side.plane)
                    .normal
                    .abs_diff_eq(DVec3::Z, 1e-8)
            });
        let Some(top) = top else {
            report.diagnostics.push(Diagnostic::error("Native liquid leaf metadata requires a visible horizontal top face; sloped or enclosed water volumes are not supported yet", Some(format!("solid:{}", brush.id)), Some((brush.mins + brush.maxs) * 0.5)));
            continue;
        };
        let plane = map.planes.get(top.plane);
        brushes[index] = Some(Candidate {
            surface: plane.dist,
            texinfo: top.texinfo,
            material: top.material,
            contents: brush.contents & LIQUID_CONTENTS,
            bottom: brush.mins.z,
        });
        let winding = top
            .winding
            .as_ref()
            .ok_or("Liquid top has no surface winding")?;
        let (mins, maxs) = winding
            .0
            .iter()
            .fold((DVec3::MAX, DVec3::MIN), |(mins, maxs), &point| {
                (mins.min(point), maxs.max(point))
            });
        water.surfaces.push((mins, maxs));
    }
    if report.has_errors() || water.surfaces.is_empty() {
        return Ok(water);
    }
    let mut candidates = vec![None; tree.nodes.len()];
    let order = tree.preorder();
    for &index in &order {
        control.checkpoint()?;
        let node = &tree.nodes[index];
        if node.split.is_some() || node.solid() || node.contents & LIQUID_CONTENTS == 0 {
            continue;
        }
        let mut chosen: Option<Candidate> = None;
        for &brush in &node.brushes {
            let Some(candidate) = brushes[brush] else {
                continue;
            };
            if chosen.is_some_and(|prior| !prior.compatible(candidate)) {
                report.diagnostics.push(Diagnostic::error("Overlapping liquid brushes with different surfaces or materials are not supported yet; split them into non-overlapping volumes", Some(format!("solid:{}", map.brushes[brush].id)), Some(node.volume.bounds().0)));
                break;
            }
            chosen = Some(match chosen {
                Some(prior) => Candidate {
                    bottom: prior.bottom.min(candidate.bottom),
                    ..prior
                },
                None => candidate,
            });
        }
        if let Some(candidate) = chosen {
            candidates[index] = Some(Candidate {
                bottom: node.volume.bounds().0.z.max(candidate.bottom),
                ..candidate
            });
        }
    }
    if report.has_errors() {
        return Ok(water);
    }
    let mut edges = vec![Vec::new(); tree.nodes.len()];
    for portal in portals {
        control.checkpoint()?;
        let [a, b] = portal.leaves;
        if a < candidates.len()
            && b < candidates.len()
            && candidates[a].is_some()
            && candidates[b].is_some()
        {
            edges[a].push((b, &portal.winding));
            edges[b].push((a, &portal.winding));
        }
    }
    let mut visited = vec![false; tree.nodes.len()];
    let mut stored = HashMap::new();
    for index in order {
        control.checkpoint()?;
        let Some(base) = candidates[index] else {
            continue;
        };
        if visited[index] {
            continue;
        }
        let mut pending = vec![index];
        let mut region = Vec::new();
        let mut bottom = base.bottom;
        visited[index] = true;
        while let Some(leaf) = pending.pop() {
            control.checkpoint()?;
            region.push(leaf);
            bottom = bottom.min(candidates[leaf].unwrap().bottom);
            for &(next, winding) in &edges[leaf] {
                if !visited[next]
                    && candidates[next].is_some_and(|candidate| base.compatible(candidate))
                    && winding
                        .0
                        .iter()
                        .all(|point| point.z <= base.surface + SURFACE_EPSILON)
                {
                    visited[next] = true;
                    pending.push(next);
                }
            }
        }
        let texinfo = i16::try_from(base.texinfo)
            .map_err(|_| "Liquid surface texinfo exceeds the BSP short-index limit")?;
        let record = bsp::LeafWaterData {
            surface_z: base.surface as f32,
            min_z: bottom as f32,
            surface_texinfo: texinfo,
            padding: 0,
        };
        let key = (
            record.surface_z.to_bits(),
            record.min_z.to_bits(),
            record.surface_texinfo,
        );
        let id = if let Some(&id) = stored.get(&key) {
            id
        } else {
            if water.records.len() >= bsp::MAX_LEAF_WATER_RECORDS {
                return Err("BSP liquid region limit exceeded".into());
            }
            let id = water.records.len() as i16;
            water.records.push(record);
            stored.insert(key, id);
            id
        };
        for leaf in region {
            water.leaves[leaf] = id;
        }
    }
    if !water.records.is_empty() {
        report.diagnostics.push(Diagnostic::warning("Native liquid leaf and surface metadata compiled; physics fluid volumes, depth-specific material patches and CS:S rendering validation remain unfinished", None, None));
    }
    Ok(water)
}

impl Water {
    pub(crate) fn face_volume(&self, face: &Face, map: &Map, tree: &Tree) -> i16 {
        if map.texinfos.infos[face.texinfo].flags & surf::WARP == 0 {
            return -1;
        }
        let point = face.winding.center() - map.planes.get(face.plane).normal * SURFACE_EPSILON;
        self.leaves[tree.leaf_at(point, &map.planes)]
    }

    pub(crate) fn distance(&self, bounds: (DVec3, DVec3)) -> u16 {
        self.surfaces
            .iter()
            .map(|&(mins, maxs)| {
                let gap = (mins - bounds.1).max(bounds.0 - maxs).max(DVec3::ZERO);
                gap.length().floor().clamp(0.0, f64::from(u16::MAX)) as u16
            })
            .min()
            .unwrap_or(u16::MAX)
    }
}
