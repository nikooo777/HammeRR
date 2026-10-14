use std::collections::BTreeMap;

use glam::DVec3;
use map_core::checks::{Location, Report, SOURCE_COORDINATE_LIMIT, Severity};
use map_core::{BrushGeometry, ObjectId};
use source_formats::bsp::{contents, surf};
use source_formats::gamefs::GameFs;
use source_formats::vmf::{Solid, Vmf};
use source_formats::vmt::{CompileSemantics, Material};

const HULL_MINS: DVec3 = DVec3::new(-16.0, -16.0, 0.0);
const HULL_MAXS: DVec3 = DVec3::new(16.0, 16.0, 72.0);
const CONTACT_TOLERANCE: f64 = 0.01;
const AXIS_TOLERANCE: f64 = 1e-12;
const MAX_BRUSH_SIDES: usize = 256;
const MAX_BLOCKER_NAMES: usize = 4;
#[path = "spawns/terrain.rs"]
mod terrain;
const PLAYER_BLOCKING_CONTENTS: i32 =
    contents::SOLID | contents::WINDOW | contents::PLAYERCLIP | contents::GRATE;
const TEAMS: [(&str, &str); 2] = [
    ("info_player_terrorist", "Terrorist"),
    ("info_player_counterterrorist", "Counter-Terrorist"),
];

pub(super) const SCOPE: &str = "Spawn clearance checks the 32 × 32 × 72 standing hull at the authored origin against visible world/func_detail brushes and world terrain sheets with known player-blocking materials and player collision enabled. Touching within 0.01 units is allowed; terrain parent volumes do not block. Trigger, hint, origin and areaportal brushes are excluded. Other brush entities, models, removed terrain triangles, compiler LOD snapping, runtime spawn placement, ground support, player counts and objectives are unchecked.";

struct Spawn {
    location: Location,
    label: &'static str,
    mins: DVec3,
    maxs: DVec3,
    blockers: Vec<String>,
    blocker_count: usize,
}

struct Projection {
    axis: DVec3,
    min: f64,
    max: f64,
}

struct Materials<'a> {
    game: &'a GameFs,
    cache: BTreeMap<String, Result<CompileSemantics, String>>,
}

impl Materials<'_> {
    fn semantics(&mut self, name: &str) -> Result<&CompileSemantics, String> {
        let key = super::material_path(name.trim());
        let material = self.cache.entry(key.clone()).or_insert_with(|| {
            let material = Material::load(self.game, &key)
                .map_err(|error| format!("Material {name:?} could not be read: {error}"))?
                .ok_or_else(|| format!("Material {name:?} was not found"))?;
            let semantics = material.compile_semantics();
            if !semantics.unsupported_flags.is_empty() {
                return Err(format!(
                    "Material {name:?} has unsupported compile flags {}",
                    semantics.unsupported_flags.join(", ")
                ));
            }
            Ok(semantics)
        });
        material.as_ref().map_err(Clone::clone)
    }

    fn blocks_player(&mut self, solid: &Solid, geometry: &BrushGeometry) -> Result<bool, String> {
        let mut brush_contents = 0;
        let mut solid_sides = false;
        for face in &geometry.faces {
            let semantics = self.semantics(&solid.sides[face.side].material)?;
            if semantics.hint
                || semantics.contents & (contents::ORIGIN | contents::AREAPORTAL) != 0
                || semantics.surface & surf::TRIGGER != 0
            {
                return Ok(false);
            }
            brush_contents |= semantics.contents;
            solid_sides |= semantics.contents == contents::SOLID;
        }
        if solid_sides && brush_contents & !(contents::SOLID | contents::DETAIL) != 0 {
            brush_contents &= !contents::SOLID;
            if brush_contents & (contents::WINDOW | contents::TRANSLUCENT) != 0 {
                brush_contents |= contents::WINDOW;
            }
        }
        Ok(brush_contents & PLAYER_BLOCKING_CONTENTS != 0)
    }
}

fn ambiguous(id: u32, counts: &BTreeMap<u32, usize>) -> bool {
    id == 0 || counts.get(&id) != Some(&1)
}

pub(super) fn check(
    vmf: &Vmf,
    game: Option<&GameFs>,
    counts: &BTreeMap<u32, usize>,
    report: &mut Report,
) {
    let mut spawns = Vec::new();
    let mut visible = [false; TEAMS.len()];
    for (index, entity) in vmf.entities.iter().enumerate().filter(|(_, e)| !e.hidden) {
        let Some(team) = TEAMS.iter().position(|(classname, _)| {
            entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case(classname))
        }) else {
            continue;
        };
        visible[team] = true;
        let location = Location {
            object: Some(ObjectId::Entity(entity.id)),
            position: entity.origin().filter(|point| point.is_finite()),
            path: format!("entity[{index}]"),
            ambiguous: ambiguous(entity.id, counts),
            property: Some("origin".into()),
            ..Default::default()
        };
        let origin = entity.origin().filter(|point| {
            point.is_finite() && point.abs().max_element() <= SOURCE_COORDINATE_LIMIT
        });
        if entity.properties.get_all("origin").count() != 1 || origin.is_none() {
            report.add(
                Severity::Warning,
                "spawn_origin",
                &location,
                format!("{} spawn needs one explicit origin with three finite coordinates within ±{SOURCE_COORDINATE_LIMIT} units. Its standing-hull clearance was not checked.", TEAMS[team].1),
                None,
            );
            continue;
        }
        if entity.properties.get_all("classname").count() != 1
            || !entity.solids.is_empty()
            || entity
                .properties
                .get_all("parentname")
                .any(|name| !name.trim().is_empty())
        {
            report.add(
                Severity::Warning,
                "spawn_clearance_unchecked",
                &location,
                "Spawn clearance requires a point spawn with one classname and no runtime parent. Inspect this entity and test its placement in CS:S.",
                None,
            );
            continue;
        }
        let origin = origin.unwrap();
        spawns.push(Spawn {
            location,
            label: TEAMS[team].1,
            mins: origin + HULL_MINS,
            maxs: origin + HULL_MAXS,
            blockers: Vec::new(),
            blocker_count: 0,
        });
    }
    for (team, (classname, label)) in TEAMS.iter().enumerate() {
        if !visible[team] {
            report.add(
                Severity::Warning,
                "missing_team_spawn",
                &Location::default(),
                format!(
                    "No visible {label} spawn ({classname}). Add a spawn before testing that team."
                ),
                None,
            );
        }
    }
    if !visible.iter().any(|present| *present) {
        return;
    }
    report.add(
        Severity::Info,
        "spawn_clearance_scope",
        &Location::default(),
        SCOPE,
        None,
    );
    if spawns.is_empty() {
        return;
    }
    let Some(game) = game else {
        report.add(
            Severity::Warning,
            "spawn_clearance_unchecked",
            &Location::default(),
            "Game content is unavailable. Spawn hull intersections were not checked because player-blocking material contents cannot be resolved.",
            None,
        );
        return;
    };
    let mut materials = Materials {
        game,
        cache: BTreeMap::new(),
    };
    let mut terrain_budget = terrain::Budget::default();
    for (index, solid) in vmf
        .world
        .solids
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.hidden)
    {
        inspect_brush(
            solid,
            Location {
                object: Some(ObjectId::Solid(solid.id)),
                ambiguous: ambiguous(solid.id, counts),
                path: format!("world/solid[{index}]"),
                ..Default::default()
            },
            &mut materials,
            &mut spawns,
            report,
            &mut terrain_budget,
        );
    }
    for (index, entity) in vmf.entities.iter().enumerate().filter(|(_, e)| !e.hidden) {
        if !entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("func_detail"))
        {
            continue;
        }
        if entity.properties.get_all("classname").count() != 1 {
            report.add(Severity::Warning, "spawn_clearance_unchecked", &Location {
                object: Some(ObjectId::Entity(entity.id)),
                ambiguous: ambiguous(entity.id, counts),
                path: format!("entity[{index}]"),
                ..Default::default()
            }, "Brush entity has repeated classnames; its static spawn-blocking collision was not checked.", None);
            continue;
        }
        for (brush_index, solid) in entity.solids.iter().enumerate().filter(|(_, s)| !s.hidden) {
            inspect_brush(
                solid,
                Location {
                    object: Some(ObjectId::Entity(entity.id)),
                    ambiguous: ambiguous(entity.id, counts) || ambiguous(solid.id, counts),
                    path: format!("entity[{index}]/solid[{brush_index}]"),
                    ..Default::default()
                },
                &mut materials,
                &mut spawns,
                report,
                &mut terrain_budget,
            );
        }
    }
    for spawn in spawns
        .into_iter()
        .filter(|spawn| !spawn.blockers.is_empty())
    {
        let names = spawn.blockers.join(", ");
        let extra = spawn.blocker_count.saturating_sub(MAX_BLOCKER_NAMES);
        let suffix = if extra == 0 {
            String::new()
        } else {
            format!(" and {extra} more")
        };
        report.add(
            Severity::Warning,
            "spawn_clearance",
            &spawn.location,
            format!("{} spawn's 32 × 32 × 72 standing hull intersects {names}{suffix}. Move the spawn or inspect the blocking geometry. This checks the authored origin; runtime placement still needs testing in CS:S.", spawn.label),
            None,
        );
    }
}

fn inspect_brush(
    solid: &Solid,
    mut location: Location,
    materials: &mut Materials<'_>,
    spawns: &mut [Spawn],
    report: &mut Report,
    terrain_budget: &mut terrain::Budget,
) {
    if solid.sides.iter().any(|side| side.displacement.is_some()) {
        terrain::inspect(solid, location, materials, spawns, report, terrain_budget);
        return;
    }
    let geometry = match checked_geometry(solid) {
        Ok(geometry) => geometry,
        Err(reason) => {
            report.add(
                Severity::Warning,
                "spawn_clearance_unchecked",
                &location,
                format!(
                    "Brush {} was excluded from spawn clearance: {reason}.",
                    solid.id
                ),
                None,
            );
            return;
        }
    };
    if !spawns
        .iter()
        .any(|spawn| bounds_overlap(&geometry, spawn.mins, spawn.maxs))
    {
        return;
    }
    location.position = Some(geometry.center());
    match materials.blocks_player(solid, &geometry) {
        Ok(false) => return,
        Ok(true) => {}
        Err(reason) => {
            report.add(Severity::Warning, "spawn_clearance_unchecked", &location, format!("Brush {} near a spawn was excluded from clearance: {reason}. Its collision is uncertain.", solid.id), None);
            return;
        }
    }
    let projections = project_axes(&geometry, &separating_axes(&geometry));
    for spawn in spawns {
        if bounds_overlap(&geometry, spawn.mins, spawn.maxs)
            && intersects_box(&projections, spawn.mins, spawn.maxs)
        {
            spawn.blocker_count += 1;
            if spawn.blockers.len() < MAX_BLOCKER_NAMES {
                spawn
                    .blockers
                    .push(format!("brush {} ({})", solid.id, location.path));
            }
        }
    }
}

fn checked_geometry(solid: &Solid) -> Result<BrushGeometry, &'static str> {
    if solid.sides.len() > MAX_BRUSH_SIDES {
        return Err("more than 256 sides");
    }
    let geometry = BrushGeometry::from_solid(solid).map_err(|_| "invalid brush geometry")?;
    if !geometry.mins.is_finite()
        || !geometry.maxs.is_finite()
        || geometry.mins.abs().max(geometry.maxs.abs()).max_element() > SOURCE_COORDINATE_LIMIT
        || geometry.faces.iter().any(|face| {
            !face.plane.normal.is_finite()
                || !face.plane.dist.is_finite()
                || face
                    .vertices
                    .iter()
                    .any(|point| !closed_corner(*point, &geometry))
        })
    {
        return Err("brush is not closed, finite and within Source coordinate bounds");
    }
    Ok(geometry)
}

fn closed_corner(point: DVec3, geometry: &BrushGeometry) -> bool {
    if !point.is_finite() {
        return false;
    }
    let normals = geometry
        .faces
        .iter()
        .filter(|face| face.plane.distance_to(point).abs() < map_core::plane::ON_EPSILON * 4.0)
        .map(|face| face.plane.normal)
        .collect::<Vec<_>>();
    for (index, first) in normals.iter().enumerate() {
        for (next, second) in normals.iter().enumerate().skip(index + 1) {
            if normals
                .iter()
                .skip(next + 1)
                .any(|third| first.cross(*second).dot(*third).abs() > 1e-8)
            {
                return true;
            }
        }
    }
    false
}

fn bounds_overlap(geometry: &BrushGeometry, mins: DVec3, maxs: DVec3) -> bool {
    geometry
        .mins
        .cmplt(maxs - DVec3::splat(CONTACT_TOLERANCE))
        .all()
        && geometry
            .maxs
            .cmpgt(mins + DVec3::splat(CONTACT_TOLERANCE))
            .all()
}

fn add_axis(axes: &mut Vec<DVec3>, axis: DVec3) {
    if axis.length_squared() <= AXIS_TOLERANCE * AXIS_TOLERANCE {
        return;
    }
    let axis = axis.normalize();
    if !axes.iter().any(|other| {
        (*other - axis).abs().max_element() <= AXIS_TOLERANCE
            || (*other + axis).abs().max_element() <= AXIS_TOLERANCE
    }) {
        axes.push(axis);
    }
}

fn separating_axes(geometry: &BrushGeometry) -> Vec<DVec3> {
    let mut axes = vec![DVec3::X, DVec3::Y, DVec3::Z];
    for face in &geometry.faces {
        add_axis(&mut axes, face.plane.normal);
        for (index, point) in face.vertices.iter().enumerate() {
            let edge = face.vertices[(index + 1) % face.vertices.len()] - *point;
            for box_axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                add_axis(&mut axes, edge.cross(box_axis));
            }
        }
    }
    axes
}

fn project_axes(geometry: &BrushGeometry, axes: &[DVec3]) -> Vec<Projection> {
    axes.iter()
        .map(|axis| {
            let (min, max) = geometry
                .faces
                .iter()
                .flat_map(|face| &face.vertices)
                .map(|point| point.dot(*axis))
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), value| {
                    (lo.min(value), hi.max(value))
                });
            Projection {
                axis: *axis,
                min,
                max,
            }
        })
        .collect()
}

fn intersects_box(projections: &[Projection], mins: DVec3, maxs: DVec3) -> bool {
    let center = (mins + maxs) * 0.5;
    let half_size = (maxs - mins) * 0.5;
    projections.iter().all(|projection| {
        let middle = center.dot(projection.axis);
        let radius = half_size.dot(projection.axis.abs());
        projection.max > middle - radius + CONTACT_TOLERANCE
            && projection.min < middle + radius - CONTACT_TOLERANCE
    })
}

#[cfg(test)]
mod tests;
