use std::collections::BTreeMap;

use glam::DVec3;
use source_formats::bsp::TexInfo;

use crate::map::Map;
use crate::tree::Tree;
use crate::winding::{ON_EPSILON, Winding};

const MAX_LIGHTMAP_EXTENT: i32 = 32;

#[derive(Debug)]
pub struct Face {
    pub plane: usize,
    pub texinfo: usize,
    pub winding: Winding,
    pub node: Option<usize>,
    pub leaves: Vec<usize>,
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

fn subdivide(face: Face, map: &Map, output: &mut Vec<Face>) {
    let material = map
        .materials
        .get(map.texdata[map.texinfos.infos[face.texinfo].texdata as usize]);
    if !material.lit() {
        output.push(face);
        return;
    }
    let info = &map.texinfos.infos[face.texinfo];
    let mut pending = vec![face];
    while let Some(face) = pending.pop() {
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
            plane: face.plane,
            texinfo: face.texinfo,
            winding: front,
            node: face.node,
            leaves: face.leaves.clone(),
        });
        pending.push(Face {
            winding: back,
            ..face
        });
    }
}

pub fn build(map: &Map, tree: &Tree, entity: usize) -> Vec<Face> {
    let mut groups: BTreeMap<(Option<usize>, usize, usize), Vec<Face>> = BTreeMap::new();
    for &brush_index in &map.entities[entity].brushes {
        let brush = &map.brushes[brush_index];
        for side in brush.sides.iter().filter(|s| s.visible && !s.bevel) {
            let Some(winding) = &side.winding else {
                continue;
            };
            let mut pending = vec![(0, winding.clone(), None)];
            while let Some((index, winding, on_node)) = pending.pop() {
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
                            .entry((on_node, side.plane, side.texinfo))
                            .or_default()
                            .push(Face {
                                plane: side.plane,
                                texinfo: side.texinfo,
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
    for (_, mut faces) in groups {
        let mut i = 0;
        while i < faces.len() {
            let normal = map.planes.get(faces[i].plane).normal;
            let mut j = i + 1;
            while j < faces.len() {
                if let Some(winding) = faces[i].winding.merge(&faces[j].winding, normal) {
                    let other = faces.remove(j);
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
        for face in faces {
            subdivide(face, map, &mut output);
        }
    }
    output
}
