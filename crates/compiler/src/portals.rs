use std::collections::VecDeque;

use glam::DVec3;
use source_formats::bsp::contents;

use crate::map::MapEntity;
use crate::planes::PlaneSet;
use crate::tree::Tree;
use crate::winding::{Side, Winding};
use crate::{Control, Diagnostic, Report};

#[derive(Debug)]
pub struct Portal {
    pub leaves: [usize; 2],
    pub winding: Winding,
}

#[derive(Debug, Clone)]
pub struct Leak {
    pub entity: u32,
    pub points: Vec<DVec3>,
}

fn connect(
    tree: &Tree,
    planes: &PlaneSet,
    leaves: [usize; 2],
    winding: Winding,
    portals: &mut Vec<Portal>,
    control: &Control,
) -> Result<(), String> {
    let mut pending = vec![(leaves, winding)];
    while let Some((leaves, winding)) = pending.pop() {
        control.checkpoint()?;
        if winding.is_tiny() {
            continue;
        }
        let split = leaves
            .iter()
            .enumerate()
            .find_map(|(side, &index)| Some((side, tree.nodes.get(index)?.split?)));
        let Some((side, (plane, children))) = split else {
            portals.push(Portal { leaves, winding });
            continue;
        };
        let plane = planes.get(plane);
        let halves = match winding.classify(plane.normal, plane.dist) {
            Side::On => [Some(winding.clone()), Some(winding)],
            _ => {
                let (front, back) = winding.split(plane.normal, plane.dist);
                [front, back]
            }
        };
        for (child, half) in children.into_iter().zip(halves) {
            if let Some(half) = half {
                let mut next = leaves;
                next[side] = child;
                pending.push((next, half));
            }
        }
    }
    Ok(())
}

pub fn build(tree: &Tree, planes: &PlaneSet) -> Vec<Portal> {
    build_controlled(tree, planes, &Control::default()).expect("uncancelled portals")
}

pub fn build_controlled(
    tree: &Tree,
    planes: &PlaneSet,
    control: &Control,
) -> Result<Vec<Portal>, String> {
    let mut portals = Vec::new();
    for (index, node) in tree.nodes.iter().enumerate() {
        control.checkpoint()?;
        let Some((split, children)) = node.split else {
            continue;
        };
        let plane = planes.get(split);
        let mut winding = Some(Winding::base(plane.normal, plane.dist));
        for side in &node.volume.sides {
            let clip = planes.get(side.plane);
            winding = winding.and_then(|w| w.clip_back(clip.normal, clip.dist));
        }
        if let Some(winding) = winding {
            connect(tree, planes, children, winding, &mut portals, control)?;
        }
        debug_assert!(index < children[0] && index < children[1]);
    }
    let outside = tree.nodes.len();
    for side in &tree.nodes[0].volume.sides {
        connect(
            tree,
            planes,
            [0, outside],
            side.winding.clone(),
            &mut portals,
            control,
        )?;
    }
    Ok(portals)
}

pub fn flood(
    tree: &mut Tree,
    planes: &PlaneSet,
    portals: &[Portal],
    entities: &[MapEntity],
    report: &mut Report,
) -> Option<Leak> {
    flood_controlled(tree, planes, portals, entities, report, &Control::default())
        .expect("uncancelled flood")
}

pub fn flood_controlled(
    tree: &mut Tree,
    planes: &PlaneSet,
    portals: &[Portal],
    entities: &[MapEntity],
    report: &mut Report,
    control: &Control,
) -> Result<Option<Leak>, String> {
    control.checkpoint()?;
    let outside = tree.nodes.len();
    let mut edges = vec![Vec::new(); outside + 1];
    for (index, portal) in portals.iter().enumerate() {
        let [a, b] = portal.leaves;
        edges[a].push((b, index));
        edges[b].push((a, index));
    }
    let mut reached = vec![false; outside + 1];
    let mut previous: Vec<Option<(usize, usize)>> = vec![None; outside + 1];
    let mut seeds = vec![None; outside + 1];
    let mut pending = VecDeque::new();
    for entity in entities {
        control.checkpoint()?;
        let Some(origin) = entity.origin else {
            continue;
        };
        let leaf = tree.leaf_at(origin, planes);
        if tree.nodes[leaf].solid() {
            report.diagnostics.push(Diagnostic::warning(
                format!("{} is inside solid space", entity.classname),
                Some(format!("entity:{}", entity.id)),
                Some(origin),
            ));
        } else if !reached[leaf] {
            reached[leaf] = true;
            seeds[leaf] = Some((entity.id, origin));
            pending.push_back(leaf);
        }
    }
    if pending.is_empty() {
        report.diagnostics.push(Diagnostic::error(
            "no point entity in open space; add a player spawn inside the map",
            None,
            None,
        ));
        return Ok(None);
    }
    while let Some(leaf) = pending.pop_front() {
        control.checkpoint()?;
        if leaf == outside {
            let mut points = Vec::new();
            let mut cursor = outside;
            while let Some((parent, portal)) = previous[cursor] {
                points.push(portals[portal].winding.center());
                cursor = parent;
            }
            let (entity, origin) = seeds[cursor].unwrap();
            points.push(origin);
            points.reverse();
            report.diagnostics.push(Diagnostic::error(
                "map leaks to the outside",
                Some(format!("entity:{entity}")),
                Some(origin),
            ));
            return Ok(Some(Leak { entity, points }));
        }
        for &(next, portal) in &edges[leaf] {
            if reached[next] || tree.nodes.get(next).is_some_and(|node| node.solid()) {
                continue;
            }
            reached[next] = true;
            previous[next] = Some((leaf, portal));
            pending.push_back(next);
        }
    }
    for (index, node) in tree.nodes.iter_mut().enumerate() {
        if node.split.is_none() && !reached[index] {
            node.contents |= contents::SOLID;
        }
    }
    tree.prune();
    Ok(None)
}
