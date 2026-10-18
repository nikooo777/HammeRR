use std::collections::BTreeMap;
use std::sync::Arc;

use glam::DVec3;
use source_formats::bsp::{TexInfo, contents, surf};

use crate::Control;
use crate::map::Map;
use crate::tree::Tree;
use crate::water;
use crate::winding::{ON_EPSILON, Winding};

const MAX_LIGHTMAP_EXTENT: i32 = 32;
const MAX_PROVENANCE_WORK: usize = 4_000_000;
pub(crate) const MAX_SMOOTH_SOURCE_POINTS: usize = 4_000_000;

mod water_budget;
#[cfg(test)]
mod water_budget_tests;

#[derive(Debug)]
pub struct SmoothingSource {
    pub plane: usize,
    pub groups: u32,
    pub winding: Winding,
}

#[derive(Debug)]
pub struct Face {
    pub smoothing_source: Option<Arc<SmoothingSource>>,
    pub source_sides: Arc<[u32]>,
    pub plane: usize,
    pub texinfo: usize,
    pub smoothing_groups: u32,
    pub winding: Winding,
    pub node: Option<usize>,
    pub leaves: Vec<usize>,
    pub water_region: Option<i16>,
    pub displacement: Option<usize>,
}

impl Face {
    pub(crate) fn lightmap_bounds(&self, info: &TexInfo) -> Result<([i32; 2], [i32; 2]), String> {
        lightmap_bounds(&self.winding, info).map_err(|error| match self.source_sides.first() {
            Some(id) => format!("side:{id}: {error}"),
            None => error,
        })
    }
}

pub fn lightmap_bounds(winding: &Winding, info: &TexInfo) -> Result<([i32; 2], [i32; 2]), String> {
    let mut mins = [0; 2];
    let mut size = [0; 2];
    for axis in 0..2 {
        let v = info.lightmap_vecs[axis];
        let direction = DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64);
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for &point in &winding.0 {
            let value = direction.dot(point) + v[3] as f64;
            if !value.is_finite() {
                return Err("Lightmap projection must stay finite".into());
            }
            lo = lo.min(value);
            hi = hi.max(value);
        }
        let (lo, hi) = (lo.floor(), hi.ceil());
        if !lo.is_finite() || !hi.is_finite() || lo < i32::MIN as f64 || hi > i32::MAX as f64 {
            return Err("Lightmap bounds exceed native signed 32-bit coordinates".into());
        }
        mins[axis] = lo as i32;
        size[axis] = (hi as i32)
            .checked_sub(mins[axis])
            .filter(|size| *size >= 0)
            .ok_or("Lightmap extent exceeds native signed 32-bit range")?;
    }
    Ok((mins, size))
}

fn subdivide(
    face: Face,
    map: &Map,
    output: &mut Vec<Face>,
    control: &Control,
) -> Result<(), String> {
    let material = map
        .materials
        .get(map.texdata[map.texinfos.infos[face.texinfo].texdata as usize]);
    if !material.lit() || map.texinfos.infos[face.texinfo].flags & surf::NOLIGHT != 0 {
        output.push(face);
        return Ok(());
    }
    let info = &map.texinfos.infos[face.texinfo];
    let mut pending = vec![face];
    while let Some(face) = pending.pop() {
        control.checkpoint()?;
        let (mins, size) = face.lightmap_bounds(info)?;
        let Some(axis) = (0..2).find(|&axis| size[axis] > MAX_LIGHTMAP_EXTENT) else {
            output.push(face);
            continue;
        };
        let v = info.lightmap_vecs[axis];
        let normal = DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64);
        let distance = (mins[axis] + MAX_LIGHTMAP_EXTENT) as f64 - v[3] as f64;
        let (front, back) = face
            .winding
            .split(normal.normalize(), distance / normal.length());
        let (Some(front), Some(back)) = (front, back) else {
            output.push(face);
            continue;
        };
        pending.push(Face {
            smoothing_source: face.smoothing_source.clone(),
            source_sides: face.source_sides.clone(),
            plane: face.plane,
            texinfo: face.texinfo,
            smoothing_groups: face.smoothing_groups,
            winding: front,
            node: face.node,
            leaves: face.leaves.clone(),
            water_region: face.water_region,
            displacement: face.displacement,
        });
        pending.push(Face {
            winding: back,
            ..face
        });
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SourceRetention {
    None,
    Assigned,
    All,
}

pub fn build(map: &Map, tree: &Tree, entity: usize) -> Vec<Face> {
    build_controlled(map, tree, entity, &Control::default()).expect("uncancelled faces")
}

pub fn build_controlled(
    map: &Map,
    tree: &Tree,
    entity: usize,
    control: &Control,
) -> Result<Vec<Face>, String> {
    let retention = if entity == 0 {
        SourceRetention::Assigned
    } else {
        SourceRetention::None
    };
    build_with_smoothing(map, tree, entity, control, retention, &mut 0)
}

pub fn build_with_smoothing(
    map: &Map,
    tree: &Tree,
    entity: usize,
    control: &Control,
    retention: SourceRetention,
    retained_points: &mut usize,
) -> Result<Vec<Face>, String> {
    control.checkpoint()?;
    let needs_regions = entity == 0
        && map.entities[entity].brushes.iter().any(|&index| {
            let brush = &map.brushes[index];
            water::has_horizontal_surface(map, brush)
                && brush.sides.iter().any(|side| {
                    side.visible
                        && !side.bevel
                        && map.planes.get(side.plane).normal.z.abs() <= 1e-8
                        && map.water_bottom_texinfos.contains_key(&side.texinfo)
                })
        });
    if needs_regions {
        let portals = crate::portals::build_controlled(tree, &map.planes, control)?;
        let mut report = crate::Report::default();
        let water = water::build(map, tree, &portals, &mut report, control)?;
        if report.has_errors() {
            return Err(
                "Water face generation requires valid horizontal world-water regions".into(),
            );
        }
        return build_with_water_regions(
            map,
            tree,
            entity,
            control,
            retention,
            retained_points,
            &water.leaves,
        );
    }
    build_with_water_regions(map, tree, entity, control, retention, retained_points, &[])
}

pub(crate) fn build_with_water_regions(
    map: &Map,
    tree: &Tree,
    entity: usize,
    control: &Control,
    retention: SourceRetention,
    retained_points: &mut usize,
    water_regions: &[i16],
) -> Result<Vec<Face>, String> {
    control.checkpoint()?;
    type Group = (
        Option<usize>,
        usize,
        usize,
        u32,
        Arc<[usize]>,
        Option<usize>,
        Option<i16>,
    );
    let mut groups: BTreeMap<Group, Vec<Face>> = BTreeMap::new();
    let mut provenance_work = 0;
    let mut water_budget = water_budget::Budget::default();
    for &brush_index in &map.entities[entity].brushes {
        control.checkpoint()?;
        let brush = &map.brushes[brush_index];
        let vertical = water::has_horizontal_surface(map, brush);
        for side in brush.sides.iter().filter(|s| s.visible && !s.bevel) {
            let Some(winding) = &side.winding else {
                continue;
            };
            let source_sides: Arc<[u32]> = side.source_id.into_iter().collect::<Vec<_>>().into();
            let bottom = if entity == 0
                && brush.structural()
                && brush.contents & contents::WATER != 0
                && water::inward_surface(map.planes.get(side.plane).normal, vertical)
            {
                map.water_bottom_texinfos.get(&side.texinfo).copied()
            } else {
                None
            };
            let vertical_side =
                bottom.is_some() && map.planes.get(side.plane).normal.z.abs() <= 1e-8;
            let mut pending = vec![(
                0,
                winding.clone(),
                None,
                side.plane,
                side.texinfo,
                false,
                None,
            )];
            while let Some((index, winding, on_node, face_plane, texinfo, below_water, paired)) =
                pending.pop()
            {
                control.checkpoint()?;
                if below_water {
                    water_budget.visit(winding.0.len(), control)?;
                }
                let node = &tree.nodes[index];
                match node.split {
                    Some((plane, children)) if face_plane & !1 == plane => {
                        let front = face_plane & 1 == 0;
                        let attached = if entity != 0 || brush.structural() {
                            Some(index)
                        } else {
                            on_node
                        };
                        pending.push((
                            children[usize::from(!front)],
                            winding,
                            attached,
                            face_plane,
                            texinfo,
                            below_water,
                            paired,
                        ));
                    }
                    Some((plane, children)) => {
                        let plane = map.planes.get(plane);
                        if below_water {
                            water_budget.split(winding.0.len(), control)?;
                        }
                        let (front, back) = winding.split(plane.normal, plane.dist);
                        for (child, half) in children.into_iter().zip([front, back]) {
                            if let Some(half) = half {
                                pending.push((
                                    child,
                                    half,
                                    on_node,
                                    face_plane,
                                    texinfo,
                                    below_water,
                                    paired,
                                ));
                            }
                        }
                    }
                    None if !node.solid() && winding.area() > ON_EPSILON * ON_EPSILON => {
                        if below_water && node.contents & contents::WATER == 0 {
                            continue;
                        }
                        if vertical_side
                            && !below_water
                            && node.contents & (contents::WATER | contents::SLIME) != 0
                        {
                            continue;
                        }
                        let water_region = if below_water
                            && map.planes.get(face_plane).normal.z.abs() <= 1e-8
                        {
                            let Some(&region) = water_regions.get(index) else {
                                return Err("Vertical water faces require checked world-water region ownership".into());
                            };
                            if region < 0 {
                                continue;
                            }
                            Some(region)
                        } else {
                            None
                        };
                        if let Some((dry_leaf, dry_node)) = paired {
                            water_budget.polygon(winding.0.len(), control)?;
                            let mut outward = winding.clone();
                            outward.0.reverse();
                            groups
                                .entry((
                                    dry_node,
                                    side.plane,
                                    side.texinfo,
                                    side.smoothing_groups,
                                    map.overlays.membership(side.source_id),
                                    None,
                                    water_region,
                                ))
                                .or_default()
                                .push(Face {
                                    smoothing_source: None,
                                    source_sides: source_sides.clone(),
                                    displacement: None,
                                    plane: side.plane,
                                    texinfo: side.texinfo,
                                    smoothing_groups: side.smoothing_groups,
                                    winding: outward,
                                    node: dry_node,
                                    leaves: vec![dry_leaf],
                                    water_region,
                                });
                        }
                        if !below_water
                            && node.contents & (contents::WATER | contents::SLIME) == 0
                            && let Some(bottom) = bottom
                        {
                            water_budget.polygon(winding.0.len(), control)?;
                            let mut reversed = winding.clone();
                            reversed.0.reverse();
                            pending.push((
                                on_node.unwrap_or(0),
                                reversed,
                                None,
                                face_plane ^ 1,
                                bottom,
                                true,
                                vertical_side.then_some((index, on_node)),
                            ));
                            if vertical_side {
                                continue;
                            }
                        }
                        groups
                            .entry((
                                on_node,
                                face_plane,
                                texinfo,
                                side.smoothing_groups,
                                map.overlays.membership(if below_water {
                                    None
                                } else {
                                    side.source_id
                                }),
                                below_water.then_some(side.texinfo),
                                water_region,
                            ))
                            .or_default()
                            .push(Face {
                                smoothing_source: None,
                                source_sides: if below_water {
                                    Arc::from([])
                                } else {
                                    source_sides.clone()
                                },
                                displacement: None,
                                plane: face_plane,
                                texinfo,
                                smoothing_groups: side.smoothing_groups,
                                winding,
                                node: on_node,
                                leaves: vec![index],
                                water_region,
                            });
                    }
                    None => {}
                }
            }
        }
    }
    let mut output = Vec::new();
    for (group, mut faces) in groups {
        let generated_water = group.5.is_some() || group.6.is_some();
        let mut i = 0;
        while i < faces.len() {
            let normal = map.planes.get(faces[i].plane).normal;
            let mut j = i + 1;
            while j < faces.len() {
                control.checkpoint()?;
                if generated_water {
                    water_budget.merge(
                        faces[i].winding.0.len(),
                        faces[j].winding.0.len(),
                        faces[i].leaves.len().saturating_add(faces[j].leaves.len()),
                        control,
                    )?;
                }
                if let Some(winding) = faces[i].winding.merge(&faces[j].winding, normal) {
                    let source_sides = merge_sources(
                        &faces[i].source_sides,
                        &faces[j].source_sides,
                        &mut provenance_work,
                    )?;
                    let other = faces.remove(j);
                    faces[i].source_sides = source_sides;
                    faces[i].winding = winding;
                    faces[i].leaves.extend(other.leaves);
                    faces[i].leaves.sort_unstable();
                    faces[i].leaves.dedup();
                    j = i + 1;
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
        for mut face in faces {
            if retention != SourceRetention::None
                && (face.smoothing_groups != 0 || retention == SourceRetention::All)
                && face.displacement.is_none()
            {
                if generated_water {
                    water_budget.clone_points(face.winding.0.len(), control)?;
                }
                *retained_points = retained_points
                    .checked_add(face.winding.0.len())
                    .filter(|&count| count <= MAX_SMOOTH_SOURCE_POINTS)
                    .ok_or(
                        "Brush smoothing sources exceed their four-million-point allocation budget",
                    )?;
                face.smoothing_source = Some(Arc::new(SmoothingSource {
                    plane: face.plane,
                    groups: face.smoothing_groups,
                    winding: face.winding.clone(),
                }));
            }
            subdivide(face, map, &mut output, control)?;
        }
    }
    Ok(output)
}

fn merge_sources(a: &Arc<[u32]>, b: &Arc<[u32]>, work: &mut usize) -> Result<Arc<[u32]>, String> {
    if Arc::ptr_eq(a, b) {
        return Ok(a.clone());
    }
    *work = work
        .checked_add(a.len())
        .and_then(|value| value.checked_add(b.len()))
        .ok_or("Face provenance work count overflow")?;
    if *work > MAX_PROVENANCE_WORK {
        return Err("Face provenance exceeded its four-million work-unit limit".into());
    }
    if a == b {
        return Ok(a.clone());
    }
    let mut sources = Vec::with_capacity(a.len() + b.len());
    let (mut left, mut right) = (0, 0);
    while left < a.len() || right < b.len() {
        if right == b.len() || (left < a.len() && a[left] < b[right]) {
            sources.push(a[left]);
            left += 1;
        } else if left == a.len() || b[right] < a[left] {
            sources.push(b[right]);
            right += 1;
        } else {
            sources.push(a[left]);
            left += 1;
            right += 1;
        }
    }
    Ok(sources.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_union_preserves_sorted_ids_shares_equal_scopes_and_bounds_work() {
        let a: Arc<[u32]> = vec![1, 2].into();
        let b: Arc<[u32]> = vec![2, 3].into();
        let mut work = 0;
        assert_eq!(&*merge_sources(&a, &b, &mut work).unwrap(), &[1, 2, 3]);
        assert_eq!(work, 4);
        assert!(Arc::ptr_eq(&a, &merge_sources(&a, &a, &mut work).unwrap()));
        work = MAX_PROVENANCE_WORK;
        assert!(merge_sources(&a, &b, &mut work).is_err());
    }
}
