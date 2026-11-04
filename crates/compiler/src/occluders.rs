use std::collections::BTreeSet;

use glam::DVec3;
use map_core::BrushGeometry;
use source_formats::bsp::{MAX_OCCLUDERS, OCCLUDER_INACTIVE, contents, surf};
use source_formats::vmf::{Entity, Solid};

use super::{Map, MapEntity, origins, support};
use crate::areas::Areas;
use crate::emit::stored_vertex;
use crate::portals::Portal;
use crate::tree::Tree;
use crate::winding::{Side, Winding};
use crate::{Control, Diagnostic, Report};

pub(crate) const MAX_BRUSHES_PER_OCCLUDER: usize = 16;
pub(crate) const MAX_POLYGONS_PER_OCCLUDER: usize = 256;
pub(crate) const MAX_VERTICES_PER_OCCLUDER: usize = 4096;
const MAX_AREA_WORK: usize = 4_000_000;

#[path = "occluders/csg.rs"]
mod csg;

#[derive(Debug)]
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

pub(super) struct Input<'a> {
    entity: usize,
    source: &'a Entity,
    geometries: Vec<(&'a Solid, BrushGeometry)>,
    polygons: Vec<Vec<Polygon>>,
    active_faces: Vec<Vec<bool>>,
    flags: i32,
}

fn property<'a>(entity: &'a Entity, name: &str) -> Option<&'a str> {
    entity
        .properties
        .iter()
        .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value))
}

impl Map<'_> {
    pub(super) fn add_occluder<'a>(
        &mut self,
        entity: &'a Entity,
        ordinal: usize,
        report: &mut Report,
        control: &Control,
    ) -> Result<Option<Input<'a>>, String> {
        let object = format!("entity:{}", entity.id);
        let solids = entity
            .solids
            .iter()
            .filter(|solid| !solid.hidden)
            .take(MAX_BRUSHES_PER_OCCLUDER + 1)
            .collect::<Vec<_>>();
        if solids.is_empty() && !entity.solids.is_empty() {
            return Ok(None);
        }
        if solids.is_empty() || solids.len() > MAX_BRUSHES_PER_OCCLUDER || ordinal >= MAX_OCCLUDERS
        {
            report.diagnostics.push(Diagnostic::error(
                "Occluders require 1–16 visible convex brushes per entity, with at most 1024 entities (the CS:S network index limit)",
                Some(object), entity.origin(),
            ));
            return Ok(None);
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
                return Ok(None);
            }
        };
        if property(entity, "parentname").is_some_and(|value| !value.trim().is_empty()) {
            report.diagnostics.push(Diagnostic::error(
                "Occluder polygons are fixed in world space; remove the parent and use Activate, Deactivate or Toggle to control visibility",
                Some(object), entity.origin(),
            ));
            return Ok(None);
        }
        if solids
            .iter()
            .try_fold(0usize, |count, solid| {
                count
                    .checked_add(solid.sides.len())
                    .filter(|&count| count <= MAX_POLYGONS_PER_OCCLUDER)
            })
            .is_none()
        {
            report.diagnostics.push(Diagnostic::error(
                "Occluder input exceeds HammeRR's combined 256-side per-entity budget",
                Some(object),
                entity.origin(),
            ));
            return Ok(None);
        }
        let mut geometries = Vec::with_capacity(solids.len());
        let mut face_count = 0usize;
        let mut vertex_count = 0usize;
        for solid in solids {
            control.checkpoint()?;
            if solid.sides.iter().any(|side| side.displacement.is_some())
                || self.origin_material(solid, control)?
            {
                report.diagnostics.push(Diagnostic::error(
                    "Occluders require ordinary convex brush geometry without terrain or origin materials",
                    Some(object),entity.origin(),
                ));
                return Ok(None);
            }
            let Some(geometry) = origins::checked_geometry(solid, report) else {
                return Ok(None);
            };
            let counts = face_count
                .checked_add(geometry.faces.len())
                .filter(|&count| count <= MAX_POLYGONS_PER_OCCLUDER)
                .zip(geometry.faces.iter().try_fold(vertex_count, |count, face| {
                    count
                        .checked_add(face.vertices.len())
                        .filter(|&count| count <= MAX_VERTICES_PER_OCCLUDER)
                }));
            let Some((faces, vertices)) = counts else {
                report.diagnostics.push(Diagnostic::error(
                    "Occluder geometry exceeds HammeRR's combined per-entity budget of 256 polygons or 4096 polygon vertices",
                    Some(object),Some(geometry.center()),
                ));
                return Ok(None);
            };
            face_count = faces;
            vertex_count = vertices;
            geometries.push((solid, geometry));
        }
        let mut brush_polygons = Vec::with_capacity(geometries.len());
        let mut active_faces = Vec::with_capacity(geometries.len());
        for (solid, geometry) in &geometries {
            let mut polygons = Vec::with_capacity(geometry.faces.len());
            let mut active = vec![false; geometry.faces.len()];
            for (index, face) in geometry.faces.iter().enumerate() {
                control.checkpoint()?;
                let side = &solid.sides[face.side];
                let material = self.materials.load(&side.material)?;
                let info = self.materials.get(material);
                if !info.found {
                    report.diagnostics.push(Diagnostic::error(
                    format!("Occluder material {} could not be loaded; mount its game content so its nodraw and trigger flags can be checked", side.material),
                    Some(object), Some(geometry.center()),
                ));
                    return Ok(None);
                }
                if info.hint || info.skip || !info.unsupported_flags.is_empty() {
                    report.diagnostics.push(Diagnostic::error(
                    "Occluder sides cannot use hint, skip or unsupported compile flags; use tools/toolsoccluder and tools/toolsnodraw",
                    Some(object), Some(geometry.center()),
                ));
                    return Ok(None);
                }
                if info.surface & surf::NODRAW != 0 && info.surface & surf::TRIGGER == 0 {
                    continue;
                }
                let Some(plane) =
                    support::find_occluder(&mut self.planes, geometry, face, control)?
                else {
                    report.diagnostics.push(Diagnostic::error(
                        "Cannot preserve the occluder's native plane support; simplify its brush",
                        Some(object),
                        Some(geometry.center()),
                    ));
                    return Ok(None);
                };
                if self.planes.len() > super::bevels::MAX_PLANES {
                    report.diagnostics.push(Diagnostic::error(
                        "Occluder exceeds the shared BSP plane limit; simplify map geometry",
                        Some(object),
                        Some(geometry.center()),
                    ));
                    return Ok(None);
                }
                let winding = Winding(face.vertices.iter().copied().map(stored_vertex).collect());
                if winding.is_tiny() || winding.normal().dot(self.planes.get(plane).normal) < 0.99 {
                    report.diagnostics.push(Diagnostic::error(
                    "Occluder face collapses after native vertex rounding; enlarge or simplify its brush",
                    Some(object), Some(geometry.center()),
                ));
                    return Ok(None);
                }
                polygons.push(Polygon { plane, winding });
                active[index] = true;
            }
            brush_polygons.push(polygons);
            active_faces.push(active);
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
            ("occludernumber".into(), ordinal.to_string()),
            ("StartActive".into(), if active { "1" } else { "0" }.into()),
        ]);
        for connection in entity.connections.iter().flatten() {
            properties.push((connection.output.clone(), connection.value.clone()));
        }
        let entity_index = self.entities.len();
        self.entities.push(MapEntity {
            id: entity.id,
            classname: "func_occluder".into(),
            properties,
            brushes: Vec::new(),
            origin: None,
        });
        Ok(Some(Input {
            entity: entity_index,
            source: entity,
            geometries,
            polygons: brush_polygons,
            active_faces,
            flags: if active { 0 } else { OCCLUDER_INACTIVE },
        }))
    }

    pub(super) fn finalize_occluders(
        &mut self,
        inputs: Vec<Input<'_>>,
        report: &mut Report,
        control: &Control,
    ) -> Result<(), String> {
        if inputs.is_empty() {
            return Ok(());
        }
        let brush_count = inputs.iter().try_fold(0usize, |count, input| {
            count
                .checked_add(input.geometries.len())
                .filter(|&count| count <= MAX_OCCLUDERS * MAX_BRUSHES_PER_OCCLUDER)
                .ok_or("Occluder source-brush count exceeds its bounded helper inputs")
        })?;
        let mut geometries = Vec::with_capacity(brush_count);
        let mut polygons = Vec::with_capacity(brush_count);
        let mut active_faces = Vec::with_capacity(brush_count);
        let mut helpers = Vec::with_capacity(inputs.len());
        for input in inputs {
            control.checkpoint()?;
            let brushes = geometries.len()..geometries.len() + input.geometries.len();
            helpers.push((input.entity, input.source, input.flags, brushes));
            geometries.extend(input.geometries);
            polygons.extend(input.polygons);
            active_faces.extend(input.active_faces);
        }
        let mut budget = csg::Budget::default();
        let boundary = match csg::boundary_grouped(
            &geometries,
            polygons,
            &active_faces,
            &self.planes,
            &mut budget,
            control,
        ) {
            Ok(boundary) => boundary,
            Err(failure) => {
                control.checkpoint()?;
                let mut reported = None;
                for brush in failure.brushes.into_iter().flatten() {
                    let Some((_, source, _, _)) = helpers
                        .iter()
                        .find(|(_, _, _, brushes)| brushes.contains(&brush))
                    else {
                        continue;
                    };
                    if reported == Some(source.id) {
                        continue;
                    }
                    report.diagnostics.push(Diagnostic::error(
                        failure.message.clone(),
                        Some(format!("entity:{}", source.id)),
                        source.origin(),
                    ));
                    reported = Some(source.id);
                }
                if reported.is_none() {
                    report
                        .diagnostics
                        .push(Diagnostic::error(failure.message, None, None));
                }
                return Ok(());
            }
        };
        if boundary.len() != brush_count {
            return Err("Occluder global boundary changed its source-brush grouping".into());
        }
        let mut boundary = boundary.into_iter();
        let mut definitions = Vec::with_capacity(helpers.len());
        for (entity, source, flags, brushes) in helpers {
            control.checkpoint()?;
            let groups = boundary
                .as_slice()
                .get(..brushes.len())
                .ok_or("Occluder global boundary lost a source brush")?;
            let count = groups.iter().try_fold(0usize, |count, group| {
                count
                    .checked_add(group.len())
                    .filter(|&count| count <= MAX_POLYGONS_PER_OCCLUDER)
            });
            let Some(count) = count else {
                report.diagnostics.push(Diagnostic::error(
                    "Occluder global union exceeds its 256 boundary-polygon per-helper budget; simplify or split the helper",
                    Some(format!("entity:{}", source.id)),
                    source.origin(),
                ));
                return Ok(());
            };
            if groups
                .iter()
                .flatten()
                .try_fold(0usize, |count, polygon| {
                    count
                        .checked_add(polygon.winding.0.len())
                        .filter(|&count| count <= MAX_VERTICES_PER_OCCLUDER)
                })
                .is_none()
            {
                report.diagnostics.push(Diagnostic::error(
                    "Occluder global union exceeds its 4096 boundary-vertex per-helper budget; simplify or split the helper",
                    Some(format!("entity:{}", source.id)),
                    source.origin(),
                ));
                return Ok(());
            }
            let mut polygons = Vec::with_capacity(count);
            polygons.extend(boundary.by_ref().take(brushes.len()).flatten());
            let empty_bound = DVec3::splat(f64::from(f32::MAX));
            let (mins, maxs) = polygons
                .iter()
                .flat_map(|polygon| polygon.winding.0.iter())
                .fold((empty_bound, -empty_bound), |(mins, maxs), &point| {
                    (mins.min(point), maxs.max(point))
                });
            definitions.push(Definition {
                entity,
                area: if polygons.is_empty() { -1 } else { 0 },
                polygons,
                mins,
                maxs,
                flags,
            });
        }
        control.checkpoint()?;
        self.occluders = definitions;
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
    let (world_mins, world_maxs) = tree.nodes[0].volume.bounds();
    for definition in &mut map.occluders {
        if definition.polygons.is_empty() {
            continue;
        }
        if definition.mins.cmplt(world_mins).any() || definition.maxs.cmpgt(world_maxs).any() {
            report.diagnostics.push(Diagnostic::error(
                "Occluder lies outside the world bounds; move it inside the sealed map",
                Some(format!("entity:{}", map.entities[definition.entity].id)),
                Some((definition.mins + definition.maxs) * 0.5),
            ));
            continue;
        }
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
            format!("Compiled {} fixed-world occluders with Activate/Deactivate/Toggle inputs; keep helpers within one visibility area", map.occluders.len()),
            None, None,
        ));
    }
    Ok(())
}
