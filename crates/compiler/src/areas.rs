use std::collections::{BTreeSet, VecDeque};

use glam::{DVec2, DVec3};
use source_formats::bsp::{
    Area, AreaData, AreaPortal, MAX_AREAS, MAX_CLIP_PORTAL_VERTICES, contents,
};

use crate::Control;
use crate::map::Map;
use crate::portals::Portal;
use crate::tree::Tree;
use crate::winding::Winding;

pub(crate) mod windows;

const MAX_WORK: usize = 4_000_000;
const MAX_CLIP_POINTS: usize = 4096;
const PLANE_TOLERANCE: f64 = 0.02;

#[derive(Debug)]
pub(crate) struct PortalBrush {
    pub entity: usize,
    pub brush: usize,
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

fn hull(
    points: &[DVec3],
    normal: DVec3,
    dist: f64,
    budget: &mut Budget<'_>,
) -> Result<Winding, String> {
    if points.len() < 3 || points.len() > MAX_CLIP_POINTS {
        return Err("Areaportal clip geometry needs 3–4096 input points".into());
    }
    budget.consume(points.len() * (points.len().ilog2() as usize + 1))?;
    let right = normal.any_orthonormal_vector();
    let up = normal.cross(right);
    let mut projected = points
        .iter()
        .map(|&point| {
            let point = point - normal * (normal.dot(point) - dist);
            (DVec2::new(right.dot(point), up.dot(point)), point)
        })
        .collect::<Vec<_>>();
    projected.sort_by(|a, b| a.0.x.total_cmp(&b.0.x).then(a.0.y.total_cmp(&b.0.y)));
    projected.dedup_by(|a, b| a.0.distance_squared(b.0) < 1e-10);
    let mut perimeter: Vec<(DVec2, DVec3)> = Vec::new();
    for pass in 0..2 {
        let begin = if pass == 0 {
            0
        } else {
            perimeter.len().saturating_sub(1)
        };
        for step in (if pass == 0 { 0 } else { 1 })..projected.len() {
            let index = if pass == 0 {
                step
            } else {
                projected.len() - 1 - step
            };
            let point = projected[index];
            while perimeter.len() >= begin + 2 {
                budget.consume(1)?;
                let a = perimeter[perimeter.len() - 2].0;
                let b = perimeter[perimeter.len() - 1].0;
                if (b - a).perp_dot(point.0 - b) > 1e-8 {
                    break;
                }
                perimeter.pop();
            }
            perimeter.push(point);
        }
    }
    perimeter.pop();
    if perimeter.len() < 3 {
        return Err("Areaportal clip polygon is degenerate".into());
    }
    let mut winding = Winding(perimeter.into_iter().map(|(_, point)| point).collect());
    if winding.normal().dot(normal) < 0.0 {
        winding = winding.reversed();
    }
    Ok(winding)
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
    let mut owners = vec![None; tree.nodes.len()];
    let mut brush_owners = vec![None; map.brushes.len()];
    let mut portal_nodes = vec![Vec::new(); map.area_portals.len()];
    for (owner, portal) in map.area_portals.iter().enumerate() {
        brush_owners[portal.brush] = Some(owner);
    }
    for (node_index, node) in tree.nodes.iter().enumerate() {
        budget.consume(node.brushes.len() + 1)?;
        if node.split.is_some() || node.solid() || node.contents & contents::AREAPORTAL == 0 {
            continue;
        }
        for &brush in &node.brushes {
            if let Some(owner) = brush_owners[brush] {
                if owners[node_index].is_some_and(|existing| existing != owner) {
                    return Err("Areaportal brush volumes overlap".into());
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
    let mut targets = vec![BTreeSet::new(); map.area_portals.len()];
    for (node, owner) in owners.iter().enumerate() {
        let Some(owner) = owner else { continue };
        for &(other, _) in &adjacency[node] {
            budget.consume(1)?;
            if let Some(other_owner) = owners[other] {
                if *owner != other_owner {
                    return Err("Touching areaportal brush volumes are not supported yet".into());
                }
            } else if leaves[other] > 0 {
                targets[*owner].insert(leaves[other]);
            }
        }
    }
    let mut pairs = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        let entity = map.entities[map.area_portals[index].entity].id;
        let areas = target.iter().copied().collect::<Vec<_>>();
        let &[a, b] = areas.as_slice() else {
            return Err(format!(
                "entity:{entity}: areaportal must separate exactly two distinct areas; found {}. Seal the opening around the portal brush",
                areas.len()
            ));
        };
        pairs.push([a, b]);
    }
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
    for (index, portal) in map.area_portals.iter().enumerate() {
        let [a, b] = pairs[index];
        let mut points = Vec::new();
        for &node in &portal_nodes[index] {
            budget.consume(1)?;
            for &(other, edge) in &adjacency[node] {
                budget.consume(1)?;
                if owners[other].is_none() && leaves[other] == b {
                    if points.len() + portals[edge].winding.0.len() > MAX_CLIP_POINTS {
                        return Err(
                            "Areaportal clip geometry exceeds its bounded point limit".into()
                        );
                    }
                    points.extend_from_slice(&portals[edge].winding.0);
                }
            }
        }
        let brush = &map.brushes[portal.brush];
        let side = brush
            .sides
            .iter()
            .find(|side| {
                let plane = map.planes.get(side.plane);
                points
                    .iter()
                    .all(|&point| plane.distance(point).abs() <= PLANE_TOLERANCE)
            })
            .ok_or_else(|| {
                format!(
                    "entity:{}: areaportal needs one planar interface between its two areas",
                    map.entities[portal.entity].id
                )
            })?;
        let plane = map.planes.get(side.plane & !1);
        let polygon = hull(&points, -plane.normal, -plane.dist, &mut budget)?;
        if vertices.len() + 2 * polygon.0.len() > MAX_CLIP_PORTAL_VERTICES {
            return Err("Areaportal clip vertices exceed the native limit".into());
        }
        for (owner, other, plane, winding) in [
            (a, b, side.plane ^ 1, polygon.clone()),
            (b, a, side.plane, polygon.clone()),
        ] {
            let first = u16::try_from(vertices.len())
                .map_err(|_| "Areaportal clip offset exceeds the native unsigned-short limit")?;
            let count = u16::try_from(winding.0.len())
                .map_err(|_| "Areaportal clip count exceeds the native unsigned-short limit")?;
            vertices.extend(winding.0.iter().map(|point| point.as_vec3().to_array()));
            by_area[owner as usize].push(AreaPortal {
                portal_key: (index + 1) as u16,
                other_area: other as u16,
                first_clip_vert: first,
                clip_vert_count: count,
                plane: plane as i32,
            });
        }
        map.entities[portal.entity]
            .properties
            .push(("portalnumber".into(), (index + 1).to_string()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_hull_projects_fragments_and_enforces_geometry_work_and_cancellation() {
        let control = Control::default();
        let mut budget = Budget {
            work: 0,
            control: &control,
        };
        let points = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(8.0, 8.0, 0.0),
            DVec3::new(0.0, 8.0, 0.0),
            DVec3::new(8.0, 0.0, 0.0),
            DVec3::new(4.0, 4.0, 0.0),
            DVec3::ZERO,
        ];
        let polygon = hull(&points, DVec3::Z, 16.0, &mut budget).unwrap();
        assert_eq!(polygon.0.len(), 4);
        assert_eq!(polygon.area(), 64.0);
        assert_eq!(polygon.normal(), DVec3::Z);
        assert!(polygon.0.iter().all(|point| point.z == 16.0));
        assert!(hull(&[DVec3::ZERO; 3], DVec3::Z, 0.0, &mut budget).is_err());
        budget.work = MAX_WORK;
        assert!(hull(&points, DVec3::Z, 16.0, &mut budget).is_err());
        control.cancel().unwrap();
        assert!(hull(&points, DVec3::Z, 16.0, &mut budget).is_err());
    }
}
