use std::collections::BTreeMap;
use std::sync::Arc;

use glam::DVec3;
use source_formats::bsp::TexInfo;

use crate::Control;
use crate::map::Map;
use crate::tree::Tree;
use crate::winding::{ON_EPSILON, Winding};

const MAX_LIGHTMAP_EXTENT: i32 = 32;
const MAX_PROVENANCE_WORK: usize = 4_000_000;
const MAX_SMOOTH_SOURCE_POINTS: usize = 4_000_000;

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
    pub displacement: Option<usize>,
}

pub fn lightmap_bounds(winding: &Winding, info: &TexInfo) -> ([i32; 2], [i32; 2]) {
    let mut mins = [0; 2];
    let mut size = [0; 2];
    for axis in 0..2 {
        let v = info.lightmap_vecs[axis];
        let direction = DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64);
        let (lo, hi) =
            winding
                .0
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &point| {
                    let value = direction.dot(point) + v[3] as f64;
                    (lo.min(value), hi.max(value))
                });
        mins[axis] = lo.floor() as i32;
        size[axis] = hi.ceil() as i32 - mins[axis];
    }
    (mins, size)
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
    if !material.lit() {
        output.push(face);
        return Ok(());
    }
    let info = &map.texinfos.infos[face.texinfo];
    let mut pending = vec![face];
    while let Some(face) = pending.pop() {
        control.checkpoint()?;
        let (mins, size) = lightmap_bounds(&face.winding, info);
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
            displacement: face.displacement,
        });
        pending.push(Face {
            winding: back,
            ..face
        });
    }
    Ok(())
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
    build_with_smoothing(map, tree, entity, control, false)
}

pub fn build_with_smoothing(
    map: &Map,
    tree: &Tree,
    entity: usize,
    control: &Control,
    retain_unassigned: bool,
) -> Result<Vec<Face>, String> {
    control.checkpoint()?;
    type Group = (Option<usize>, usize, usize, u32, Arc<[usize]>);
    let mut groups: BTreeMap<Group, Vec<Face>> = BTreeMap::new();
    let mut provenance_work = 0;
    for &brush_index in &map.entities[entity].brushes {
        control.checkpoint()?;
        let brush = &map.brushes[brush_index];
        for side in brush.sides.iter().filter(|s| s.visible && !s.bevel) {
            let Some(winding) = &side.winding else {
                continue;
            };
            let source_sides: Arc<[u32]> = side.source_id.into_iter().collect::<Vec<_>>().into();
            let mut pending = vec![(0, winding.clone(), None)];
            while let Some((index, winding, on_node)) = pending.pop() {
                control.checkpoint()?;
                let node = &tree.nodes[index];
                match node.split {
                    Some((plane, children)) if side.plane & !1 == plane => {
                        let front = side.plane & 1 == 0;
                        let attached = if entity != 0 || brush.structural() {
                            Some(index)
                        } else {
                            on_node
                        };
                        pending.push((children[usize::from(!front)], winding, attached));
                    }
                    Some((plane, children)) => {
                        let plane = map.planes.get(plane);
                        let (front, back) = winding.split(plane.normal, plane.dist);
                        for (child, half) in children.into_iter().zip([front, back]) {
                            if let Some(half) = half {
                                pending.push((child, half, on_node));
                            }
                        }
                    }
                    None if !node.solid() && winding.area() > ON_EPSILON * ON_EPSILON => {
                        groups
                            .entry((
                                on_node,
                                side.plane,
                                side.texinfo,
                                side.smoothing_groups,
                                map.overlays.membership(side.source_id),
                            ))
                            .or_default()
                            .push(Face {
                                smoothing_source: None,
                                source_sides: source_sides.clone(),
                                displacement: None,
                                plane: side.plane,
                                texinfo: side.texinfo,
                                smoothing_groups: side.smoothing_groups,
                                winding,
                                node: on_node,
                                leaves: vec![index],
                            });
                    }
                    None => {}
                }
            }
        }
    }
    let mut output = Vec::new();
    let mut retained_points = 0_usize;
    for (_, mut faces) in groups {
        let mut i = 0;
        while i < faces.len() {
            let normal = map.planes.get(faces[i].plane).normal;
            let mut j = i + 1;
            while j < faces.len() {
                control.checkpoint()?;
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
            if entity == 0
                && (face.smoothing_groups != 0 || retain_unassigned)
                && face.displacement.is_none()
            {
                retained_points = retained_points
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
