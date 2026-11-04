use std::collections::{BTreeSet, VecDeque};

use source_formats::bsp::{
    Area, AreaData, AreaPortal, MAX_AREA_PORTAL_VERTICES, MAX_AREA_PORTALS, MAX_AREAS,
    MAX_CLIP_PORTAL_VERTICES, contents,
};

use crate::Control;
use crate::map::Map;
use crate::portals::Portal;
use crate::tree::Tree;

pub(crate) mod windows;

const MAX_WORK: usize = 4_000_000;
const MAX_CLIP_POINTS: usize = 4096;
pub(crate) const MAX_PORTAL_BRUSHES: usize = 4096;
const PLANE_TOLERANCE: f64 = 0.02;
const NORMAL_TOLERANCE: f64 = 1e-8;

#[derive(Debug)]
pub(crate) struct PortalBrush {
    pub entity: usize,
    pub brushes: Vec<usize>,
}

pub(crate) struct Areas {
    pub data: AreaData,
    pub indices: Vec<i16>,
}

struct Budget<'a> {
    work: usize,
    control: &'a Control,
}

impl Budget<'_> {
    fn consume(&mut self, amount: usize) -> Result<(), String> {
        self.control.checkpoint()?;
        self.work = self
            .work
            .checked_add(amount)
            .ok_or("Areaportal work count overflow")?;
        if self.work > MAX_WORK {
            return Err("Areaportals exceeded their four-million work-unit limit".into());
        }
        Ok(())
    }
}

pub(crate) fn build(
    map: &mut Map<'_>,
    tree: &Tree,
    portals: &[Portal],
    control: &Control,
) -> Result<Areas, String> {
    let mut budget = Budget { work: 0, control };
    let mut leaves = vec![0; tree.nodes.len()];
    if map.area_portals.is_empty() {
        for (index, node) in tree.nodes.iter().enumerate() {
            budget.consume(1)?;
            if node.split.is_some() || !node.solid() {
                leaves[index] = 1;
            }
        }
        return Ok(Areas {
            data: AreaData {
                areas: vec![Area::default(); 2],
                portals: vec![AreaPortal::default()],
                clip_vertices: Vec::new(),
            },
            indices: leaves,
        });
    }
    let mut owners: Vec<Option<usize>> = vec![None; tree.nodes.len()];
    let mut brush_owners = vec![None; map.brushes.len()];
    let mut portal_nodes = vec![Vec::new(); map.area_portals.len()];
    for (owner, portal) in map.area_portals.iter().enumerate() {
        for &brush in &portal.brushes {
            budget.consume(1)?;
            brush_owners[brush] = Some(owner);
        }
    }
    for (node_index, node) in tree.nodes.iter().enumerate() {
        budget.consume(node.brushes.len() + 1)?;
        if node.split.is_some() || node.solid() || node.contents & contents::AREAPORTAL == 0 {
            continue;
        }
        for &brush in &node.brushes {
            if let Some(owner) = brush_owners[brush] {
                if let Some(existing) = owners[node_index]
                    && existing != owner
                {
                    return Err(format!(
                        "entity:{}: areaportal volume overlaps entity:{}",
                        map.entities[map.area_portals[owner].entity].id,
                        map.entities[map.area_portals[existing].entity].id,
                    ));
                }
                owners[node_index] = Some(owner);
            }
        }
        let owner = owners[node_index].ok_or("Areaportal leaf has no authored brush")?;
        portal_nodes[owner].push(node_index);
    }
    let mut adjacency = vec![Vec::new(); tree.nodes.len()];
    for (index, portal) in portals.iter().enumerate() {
        budget.consume(1)?;
        let [a, b] = portal.leaves;
        if a >= tree.nodes.len()
            || b >= tree.nodes.len()
            || tree.nodes[a].solid()
            || tree.nodes[b].solid()
        {
            continue;
        }
        adjacency[a].push((b, index));
        adjacency[b].push((a, index));
    }
    let mut count = 1;
    for start in tree.preorder() {
        budget.consume(1)?;
        if tree.nodes[start].split.is_some()
            || tree.nodes[start].solid()
            || owners[start].is_some()
            || leaves[start] != 0
        {
            continue;
        }
        if count >= MAX_AREAS {
            return Err("Visibility areas exceed Source's 255 open-area limit".into());
        }
        leaves[start] = count as i16;
        let mut pending = VecDeque::from([start]);
        while let Some(node) = pending.pop_front() {
            for &(other, _) in &adjacency[node] {
                budget.consume(1)?;
                if owners[other].is_none() && leaves[other] == 0 {
                    leaves[other] = count as i16;
                    pending.push_back(other);
                }
            }
        }
        count += 1;
    }
    let mut visited = vec![false; tree.nodes.len()];
    let mut pairs = vec![None; map.area_portals.len()];
    for (index, nodes) in portal_nodes.iter().enumerate() {
        let entity = map.entities[map.area_portals[index].entity].id;
        for &start in nodes {
            budget.consume(1)?;
            if visited[start] {
                continue;
            }
            visited[start] = true;
            let mut target = BTreeSet::new();
            let mut pending = VecDeque::from([start]);
            while let Some(node) = pending.pop_front() {
                for &(other, _) in &adjacency[node] {
                    budget.consume(1)?;
                    if owners[other] == Some(index) {
                        if !visited[other] {
                            visited[other] = true;
                            pending.push_back(other);
                        }
                    } else if owners[other].is_none() && leaves[other] > 0 {
                        target.insert(leaves[other]);
                    }
                }
            }
            let areas = target.into_iter().collect::<Vec<_>>();
            let &[a, b] = areas.as_slice() else {
                return Err(format!(
                    "entity:{entity}: every connected areaportal volume must separate exactly two distinct open areas; found {}. Seal the opening around each volume; serial touching portals are unsupported",
                    areas.len()
                ));
            };
            if pairs[index].is_some_and(|pair| pair != [a, b]) {
                return Err(format!(
                    "entity:{entity}: all areaportal volumes under one controller must separate the same two open areas"
                ));
            }
            pairs[index] = Some([a, b]);
        }
        if pairs[index].is_none() {
            return Err(format!(
                "entity:{entity}: areaportal has no open volume separating two areas"
            ));
        }
    }
    let pairs = pairs.into_iter().flatten().collect::<Vec<_>>();
    for (node, owner) in owners.iter().enumerate() {
        if let Some(owner) = owner {
            leaves[node] = pairs[*owner][0];
        }
    }
    for node in tree.preorder().into_iter().rev() {
        budget.consume(1)?;
        if let Some((_, [a, b])) = tree.nodes[node].split {
            leaves[node] = if leaves[a] == leaves[b] {
                leaves[a]
            } else {
                -1
            };
        }
    }
    let mut by_area = vec![Vec::new(); count];
    let mut vertices = Vec::new();
    let mut record_count = 1;
    for (index, portal) in map.area_portals.iter().enumerate() {
        let [a, b] = pairs[index];
        let entity = map.entities[portal.entity].id;
        let mut interface = None;
        let mut point_count = 0;
        for &node in &portal_nodes[index] {
            budget.consume(1)?;
            for &(other, edge) in &adjacency[node] {
                budget.consume(1)?;
                if owners[other].is_some() || leaves[other] != b {
                    continue;
                }
                let fragment = &portals[edge];
                let winding = &fragment.winding;
                point_count += winding.0.len();
                budget.consume(winding.0.len())?;
                if point_count > MAX_CLIP_POINTS {
                    return Err(format!(
                        "entity:{entity}: areaportal clip geometry exceeds its {MAX_CLIP_POINTS}-point input limit"
                    ));
                }
                if !(3..=MAX_AREA_PORTAL_VERTICES).contains(&winding.0.len()) {
                    return Err(format!(
                        "entity:{entity}: areaportal clip fragments require 3–{MAX_AREA_PORTAL_VERTICES} vertices; simplify the interface"
                    ));
                }
                let normal = if fragment.leaves[0] == other {
                    winding.normal()
                } else {
                    -winding.normal()
                };
                let mut side_plane = None;
                for &brush in &tree.nodes[node].brushes {
                    budget.consume(1)?;
                    if brush_owners[brush] != Some(index) {
                        continue;
                    }
                    for side in &map.brushes[brush].sides {
                        budget.consume(winding.0.len() + 1)?;
                        let plane = map.planes.get(side.plane);
                        if !side.bevel
                            && plane.normal.dot(normal) >= 1.0 - NORMAL_TOLERANCE
                            && winding.0.iter().all(|&point| {
                                plane.distance(point).abs() <= PLANE_TOLERANCE
                                    && plane.distance(point.as_vec3().as_dvec3()).abs()
                                        <= PLANE_TOLERANCE
                            })
                        {
                            side_plane = Some(side.plane);
                            break;
                        }
                    }
                    if side_plane.is_some() {
                        break;
                    }
                }
                let plane = side_plane.ok_or_else(|| {
                    format!(
                        "entity:{entity}: areaportal clip fragment has no matching outward authored face"
                    )
                })?;
                if interface.is_some_and(|existing| existing != plane) {
                    return Err(format!(
                        "entity:{entity}: all areaportal clip fragments under one controller must share one plane and orientation"
                    ));
                }
                interface = Some(plane);
                record_count += 2;
                if record_count > MAX_AREA_PORTALS {
                    return Err(format!(
                        "Areaportal fragments exceed Source's {MAX_AREA_PORTALS}-directed-record limit"
                    ));
                }
                if vertices.len() + 2 * winding.0.len() > MAX_CLIP_PORTAL_VERTICES {
                    return Err("Areaportal clip vertices exceed the native limit".into());
                }
                for (owner, other, plane) in [(a, b, plane ^ 1), (b, a, plane)] {
                    let first = u16::try_from(vertices.len()).map_err(
                        |_| "Areaportal clip offset exceeds the native unsigned-short limit",
                    )?;
                    vertices.extend(winding.0.iter().map(|point| point.as_vec3().to_array()));
                    by_area[owner as usize].push(AreaPortal {
                        portal_key: (index + 1) as u16,
                        other_area: other as u16,
                        first_clip_vert: first,
                        clip_vert_count: winding.0.len() as u16,
                        plane: plane as i32,
                    });
                }
            }
        }
        interface.ok_or_else(|| {
            format!("entity:{entity}: areaportal has no clip interface between its two areas")
        })?;
        map.entities[portal.entity]
            .properties
            .push(("portalnumber".into(), (index + 1).to_string()));
    }
    for (node, owner) in owners.iter().enumerate() {
        let Some(owner) = owner else { continue };
        for &(other, _) in &adjacency[node] {
            budget.consume(1)?;
            if let Some(other_owner) = owners[other]
                && *owner != other_owner
                && pairs[*owner] != pairs[other_owner]
            {
                return Err(format!(
                    "entity:{}: touching areaportal controllers must separate the same two areas (touches entity:{})",
                    map.entities[map.area_portals[*owner].entity].id,
                    map.entities[map.area_portals[other_owner].entity].id,
                ));
            }
        }
    }
    let mut data = AreaData {
        areas: Vec::with_capacity(count),
        portals: vec![AreaPortal::default()],
        clip_vertices: vertices,
    };
    for portals in by_area {
        data.areas.push(Area {
            first_portal: data.portals.len() as i32,
            portal_count: portals.len() as i32,
        });
        data.portals.extend(portals);
    }
    Ok(Areas {
        data,
        indices: leaves,
    })
}
