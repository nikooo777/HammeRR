use std::collections::BTreeSet;

use glam::DVec3;
use source_formats::bsp::{MAX_OCCLUDERS, OCCLUDER_INACTIVE, contents, surf};
use source_formats::vmf::Entity;

use super::{Map, MapEntity, origins, support};
use crate::areas::Areas;
use crate::emit::stored_vertex;
use crate::portals::Portal;
use crate::tree::Tree;
use crate::winding::{Side, Winding};
use crate::{Control, Diagnostic, Report};

const MAX_POLYGONS_PER_OCCLUDER: usize = 256;
const MAX_VERTICES_PER_OCCLUDER: usize = 4096;
const MAX_AREA_WORK: usize = 4_000_000;

pub(crate) struct Polygon {
    pub plane: usize,
    pub winding: Winding,
}

pub(crate) struct Definition {
    pub entity: usize,
    pub polygons: Vec<Polygon>,
    pub mins: DVec3,
    pub maxs: DVec3,
    pub area: i32,
    pub flags: i32,
}

fn property<'a>(entity: &'a Entity, name: &str) -> Option<&'a str> {
    entity
        .properties
        .iter()
        .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value))
}

impl Map<'_> {
    pub(super) fn add_occluder(
        &mut self,
        entity: &Entity,
        report: &mut Report,
        control: &Control,
    ) -> Result<(), String> {
        let object = format!("entity:{}", entity.id);
        let solids = entity
            .solids
            .iter()
            .filter(|solid| !solid.hidden)
            .collect::<Vec<_>>();
        if solids.is_empty() && !entity.solids.is_empty() {
            return Ok(());
        }
        if solids.len() != 1 || self.occluders.len() >= MAX_OCCLUDERS {
            report.diagnostics.push(Diagnostic::error(
                "Occluders currently require one visible convex brush per entity, with at most 1024 entities (the CS:S network index limit)",
                Some(object), entity.origin(),
            ));
            return Ok(());
        }
        let active = match property(entity, "StartActive").map(str::trim) {
            None | Some("1") => true,
            Some("0") => false,
            _ => {
                report.diagnostics.push(Diagnostic::error(
                    "Occluder Start Active must be 0 or 1",
                    Some(object),
                    entity.origin(),
                ));
                return Ok(());
            }
        };
        if property(entity, "parentname").is_some_and(|value| !value.trim().is_empty()) {
            report.diagnostics.push(Diagnostic::error(
                "Occluder polygons are fixed in world space; remove the parent and use Activate, Deactivate or Toggle to control visibility",
                Some(object), entity.origin(),
            ));
            return Ok(());
        }
        let solid = solids[0];
        if solid.sides.len() > MAX_POLYGONS_PER_OCCLUDER {
            report.diagnostics.push(Diagnostic::error(
                "Occluder input exceeds HammeRR's 256-side per-entity budget",
                Some(object),
                entity.origin(),
            ));
            return Ok(());
        }
        if solid.sides.iter().any(|side| side.displacement.is_some())
            || self.origin_material(solid, control)?
        {
            report.diagnostics.push(Diagnostic::error(
                "Occluders require ordinary convex brush geometry without terrain or origin materials",
                Some(object), entity.origin(),
            ));
            return Ok(());
        }
        let Some(geometry) = origins::checked_geometry(solid, report) else {
            return Ok(());
        };
        if geometry.faces.len() > MAX_POLYGONS_PER_OCCLUDER
            || geometry
                .faces
                .iter()
                .map(|face| face.vertices.len())
                .sum::<usize>()
                > MAX_VERTICES_PER_OCCLUDER
        {
            report.diagnostics.push(Diagnostic::error(
                "Occluder geometry exceeds HammeRR's per-entity budget of 256 polygons or 4096 polygon vertices",
                Some(object), Some(geometry.center()),
            ));
            return Ok(());
        }
        let mut polygons = Vec::new();
        for face in &geometry.faces {
            control.checkpoint()?;
            let side = &solid.sides[face.side];
            let material = self.materials.load(&side.material);
            let info = self.materials.get(material);
            if !info.found {
                report.diagnostics.push(Diagnostic::error(
                    format!("Occluder material {} could not be loaded; mount its game content so its nodraw and trigger flags can be checked", side.material),
                    Some(object), Some(geometry.center()),
                ));
                return Ok(());
            }
            if info.hint || info.skip || !info.unsupported_flags.is_empty() {
                report.diagnostics.push(Diagnostic::error(
                    "Occluder sides cannot use hint, skip or unsupported compile flags; use tools/toolsoccluder and tools/toolsnodraw",
                    Some(object), Some(geometry.center()),
                ));
                return Ok(());
            }
            if info.surface & surf::NODRAW != 0 && info.surface & surf::TRIGGER == 0 {
                continue;
            }
            let Some(plane) = support::find_authored(&mut self.planes, &geometry, face, control)?
            else {
                report.diagnostics.push(Diagnostic::error(
                    "Cannot preserve the occluder's native plane support; simplify its brush",
                    Some(object),
                    Some(geometry.center()),
                ));
                return Ok(());
            };
            if self.planes.len() > super::bevels::MAX_PLANES {
                report.diagnostics.push(Diagnostic::error(
                    "Occluder exceeds the shared BSP plane limit; simplify map geometry",
                    Some(object),
                    Some(geometry.center()),
                ));
                return Ok(());
            }
            let winding = Winding(face.vertices.iter().copied().map(stored_vertex).collect());
            if winding.is_tiny() || winding.normal().dot(self.planes.get(plane).normal) < 0.99 {
                report.diagnostics.push(Diagnostic::error(
                    "Occluder face collapses after native vertex rounding; enlarge or simplify its brush",
                    Some(object), Some(geometry.center()),
                ));
                return Ok(());
            }
            polygons.push(Polygon { plane, winding });
        }
        if polygons.is_empty() {
            report.diagnostics.push(Diagnostic::error(
                "Occluder has no active faces; apply tools/toolsoccluder to at least one side",
                Some(object),
                Some(geometry.center()),
            ));
            return Ok(());
        }
        let mut properties = entity
            .properties
            .iter()
            .filter(|(key, _)| {
                ![
                    "model",
                    "occludernumber",
                    "classname",
                    "hammerid",
                    "StartActive",
                ]
                .iter()
                .any(|name| key.eq_ignore_ascii_case(name))
            })
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect::<Vec<_>>();
        properties.extend([
            ("classname".into(), "func_occluder".into()),
            ("hammerid".into(), entity.id.to_string()),
            ("occludernumber".into(), self.occluders.len().to_string()),
            ("StartActive".into(), if active { "1" } else { "0" }.into()),
        ]);
        for connection in entity.connections.iter().flatten() {
            properties.push((connection.output.clone(), connection.value.clone()));
        }
        let (mins, maxs) = polygons
            .iter()
            .flat_map(|polygon| polygon.winding.0.iter())
            .fold((DVec3::MAX, DVec3::MIN), |(mins, maxs), &point| {
                (mins.min(point), maxs.max(point))
            });
        self.occluders.push(Definition {
            entity: self.entities.len(),
            polygons,
            mins,
            maxs,
            area: 0,
            flags: if active { 0 } else { OCCLUDER_INACTIVE },
        });
        self.entities.push(MapEntity {
            id: entity.id,
            classname: "func_occluder".into(),
            properties,
            brushes: Vec::new(),
            origin: None,
        });
        Ok(())
    }
}

fn consume(work: &mut usize, amount: usize, control: &Control) -> Result<(), String> {
    control.checkpoint()?;
    *work = work
        .checked_add(amount)
        .ok_or("Occluder area work count overflow")?;
    if *work > MAX_AREA_WORK {
        return Err(
            "Occluder area assignment exceeds HammeRR's four-million work-unit budget".into(),
        );
    }
    Ok(())
}

pub(crate) fn assign_areas(
    map: &mut Map<'_>,
    tree: &Tree,
    portals: &[Portal],
    areas: &Areas,
    report: &mut Report,
    control: &Control,
) -> Result<(), String> {
    if map.occluders.is_empty() {
        return Ok(());
    }
    let mut work = 0;
    let mut neighbors = vec![Vec::new(); tree.nodes.len()];
    for portal in portals {
        consume(&mut work, 1, control)?;
        let [a, b] = portal.leaves;
        if a < neighbors.len() && b < neighbors.len() {
            neighbors[a].push(b);
            neighbors[b].push(a);
        }
    }
    for definition in &mut map.occluders {
        let mut leaves = BTreeSet::new();
        for polygon in &definition.polygons {
            let mut pending = vec![(0, polygon.winding.clone())];
            while let Some((index, winding)) = pending.pop() {
                consume(&mut work, winding.0.len() + 1, control)?;
                let Some((plane, children)) = tree.nodes[index].split else {
                    leaves.insert(index);
                    continue;
                };
                let plane = map.planes.get(plane);
                let halves = match winding.classify(plane.normal, plane.dist) {
                    Side::On => [Some(winding.clone()), Some(winding)],
                    _ => {
                        let (front, back) = winding.split(plane.normal, plane.dist);
                        [front, back]
                    }
                };
                for (child, half) in children.into_iter().zip(halves) {
                    if let Some(half) = half {
                        pending.push((child, half));
                    }
                }
            }
        }
        let mut found = BTreeSet::new();
        for cross_portals in [false, true] {
            for &leaf in &leaves {
                for other in std::iter::once(leaf).chain(neighbors[leaf].iter().copied()) {
                    consume(&mut work, 1, control)?;
                    if !cross_portals && tree.nodes[other].contents & contents::AREAPORTAL != 0 {
                        continue;
                    }
                    let area = areas.indices[other];
                    if area > 0 {
                        found.insert(area);
                    }
                }
            }
            if !found.is_empty() {
                break;
            }
        }
        if found.len() != 1 {
            let entity = &map.entities[definition.entity];
            report.diagnostics.push(Diagnostic::error(
                if found.is_empty() {
                    "Occluder has no adjacent open visibility area; move it inside the sealed map".into()
                } else {
                    format!("Occluder spans {} visibility areas; split it into separate func_occluder entities within each area", found.len())
                },
                Some(format!("entity:{}", entity.id)), Some((definition.mins + definition.maxs) * 0.5),
            ));
            continue;
        }
        definition.area = i32::from(*found.first().unwrap());
    }
    if !report.has_errors() {
        report.diagnostics.push(Diagnostic::info(
            format!("Compiled {} fixed-world occluders; CS:S renderer culling and input transitions still need runtime validation", map.occluders.len()),
            None, None,
        ));
    }
    Ok(())
}
