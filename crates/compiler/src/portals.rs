use std::collections::VecDeque;

use glam::DVec3;
use source_formats::bsp::contents;

use crate::map::MapEntity;
use crate::planes::PlaneSet;
use crate::tree::Tree;
use crate::winding::{Side, Winding};
use crate::{Control, Diagnostic, Report};

mod budget;
use budget::Budget;
pub use budget::Limits;

#[cfg(test)]
mod budget_tests;

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
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    let mut pending = Vec::new();
    budget.pending(&mut pending, 1)?;
    pending.push((leaves, winding));
    while let Some((leaves, winding)) = pending.pop() {
        budget.visit(winding.0.len())?;
        if winding.is_tiny() {
            continue;
        }
        let split = leaves
            .iter()
            .enumerate()
            .find_map(|(side, &index)| Some((side, tree.nodes.get(index)?.split?)));
        let Some((side, (plane, children))) = split else {
            budget.output(portals)?;
            portals.push(Portal { leaves, winding });
            continue;
        };
        let plane = planes.get(plane);
        budget.visit(winding.0.len())?;
        let halves = match winding.classify(plane.normal, plane.dist) {
            Side::On => {
                budget.pending(&mut pending, 2)?;
                budget.allocate(winding.0.len())?;
                [Some(winding.clone()), Some(winding)]
            }
            Side::Front => {
                budget.pending(&mut pending, 1)?;
                [Some(winding), None]
            }
            Side::Back => {
                budget.pending(&mut pending, 1)?;
                [None, Some(winding)]
            }
            Side::Cross => {
                budget.pending(&mut pending, 2)?;
                budget.split(winding.0.len())?;
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
    build_limited(tree, planes, control, Limits::default())
}

pub fn build_limited(
    tree: &Tree,
    planes: &PlaneSet,
    control: &Control,
    limits: Limits,
) -> Result<Vec<Portal>, String> {
    let mut budget = Budget::new(limits, control);
    budget.visit(0)?;
    let root = tree
        .nodes
        .first()
        .ok_or("Cannot build portals for an empty BSP tree")?;
    if limits.pending == 0 {
        return Err("Portal construction exceeded its pending fragment budget".into());
    }
    let mut portals = Vec::new();
    for (index, node) in tree.nodes.iter().enumerate() {
        budget.visit(0)?;
        let Some((split, children)) = node.split else {
            continue;
        };
        let plane = planes.get(split);
        budget.allocate(4)?;
        let mut winding = Some(Winding::base(plane.normal, plane.dist));
        for side in &node.volume.sides {
            budget.visit(0)?;
            let clip = planes.get(side.plane);
            winding = match winding {
                Some(winding) => clip_back(winding, clip.normal, clip.dist, &mut budget)?,
                None => break,
            };
        }
        if let Some(winding) = winding {
            connect(tree, planes, children, winding, &mut portals, &mut budget)?;
        }
        debug_assert!(index < children[0] && index < children[1]);
    }
    let outside = tree.nodes.len();
    for side in &root.volume.sides {
        budget.allocate(side.winding.0.len())?;
        connect(
            tree,
            planes,
            [0, outside],
            side.winding.clone(),
            &mut portals,
            &mut budget,
        )?;
    }
    Ok(portals)
}

fn clip_back(
    winding: Winding,
    normal: DVec3,
    dist: f64,
    budget: &mut Budget<'_>,
) -> Result<Option<Winding>, String> {
    budget.visit(winding.0.len())?;
    match winding.classify(normal, dist) {
        Side::Back | Side::On => Ok(Some(winding)),
        Side::Front => Ok(None),
        Side::Cross => {
            budget.split(winding.0.len())?;
            Ok(winding.split(normal, dist).1)
        }
    }
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

pub fn flood_controlled<'a>(
    tree: &mut Tree,
    planes: &PlaneSet,
    portals: &[Portal],
    entities: impl IntoIterator<Item = &'a MapEntity>,
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
