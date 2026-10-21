use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use source_formats::bsp::{contents, surf};
use source_formats::vmf::Entity;

use crate::brush_model_pose::{Pose, component, initial_offset, initial_rotation};
use crate::emit::Model;
use crate::map::{MAX_COMPILE_COORD, Map};
use crate::{Control, Diagnostic, PhysicsMode, Report};

pub const WATER_ANALOG_CLASSNAME: &str = "func_water_analog";
pub const WATER_ANALOG_DEFAULTS: [(&str, &str); 3] = [
    ("angles", "0 0 0"),
    ("StartPosition", "0"),
    ("spawnflags", "0"),
];

const GEOMETRY_EPSILON: f64 = 1e-6;
const MAX_FIELD_BYTES: usize = 128;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Carrier {
    pub entity: usize,
    pub brush: usize,
}

pub(crate) fn declared(entity: &Entity) -> bool {
    entity
        .properties
        .get_all("classname")
        .any(|value| value.eq_ignore_ascii_case(WATER_ANALOG_CLASSNAME))
}

fn diagnostic(id: u32, origin: Option<DVec3>, message: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::error(
        format!("Cannot compile {WATER_ANALOG_CLASSNAME}: {message}"),
        Some(format!("entity:{id}")),
        origin.filter(|point| point.is_finite()),
    )
}

pub(crate) fn error(map: &Map, carrier: Carrier, message: impl std::fmt::Display) -> Diagnostic {
    let entity = &map.entities[carrier.entity];
    diagnostic(entity.id, entity.origin, message)
}

fn zero(value: &str, count: usize) -> bool {
    if value.len() > MAX_FIELD_BYTES || !value.is_ascii() {
        return false;
    }
    let mut values = value.split_ascii_whitespace();
    for _ in 0..count {
        if !values
            .next()
            .is_some_and(|token| component(token).is_ok_and(|value| value == 0.0))
        {
            return false;
        }
    }
    values.next().is_none()
}

fn source(entity: &Entity, report: &mut Report) -> Result<(), String> {
    if entity.solids.len() != 1 || entity.solids[0].hidden {
        return Err("one visible cuboid brush with no hidden children is required".into());
    }
    if entity.solids[0].sides.len() != 6 {
        return Err("exactly six source sides are required for the stationary cuboid".into());
    }
    if entity.solids[0]
        .sides
        .iter()
        .any(|side| side.displacement.is_some())
    {
        return Err("terrain is unsupported in a stationary water volume".into());
    }
    if !entity.extra.is_empty()
        || entity
            .connections
            .as_ref()
            .is_some_and(|connections| !connections.is_empty())
    {
        return Err(
            "runtime blocks and outputs are unsupported for a stationary water volume".into(),
        );
    }
    let mut keys = BTreeSet::new();
    let mut pose = false;
    for (key, value) in entity.properties.iter() {
        let lower = key.to_ascii_lowercase();
        if !keys.insert(lower.clone()) {
            return Err(format!("duplicate {key} fields are unsupported"));
        }
        match lower.as_str() {
            "classname" if key == "classname" && value == WATER_ANALOG_CLASSNAME => {}
            "origin" => {
                initial_offset(&[(key.to_owned(), value.to_owned())])?;
                if value.len() > MAX_FIELD_BYTES || !value.is_ascii() {
                    return Err("origin requires at most 128 ASCII bytes".into());
                }
                let origin = entity
                    .origin()
                    .ok_or("an explicit finite origin is required")?;
                if !origin.is_finite()
                    || origin.abs().max_element() > MAX_COMPILE_COORD
                    || !origin.as_vec3().is_finite()
                {
                    return Err("origin must fit finite Source coordinates".into());
                }
            }
            "angles" | "angle" => {
                if pose || !zero(value, if lower == "angles" { 3 } else { 1 }) {
                    return Err("one absent or exact-zero angle/angles field is required".into());
                }
                pose = true;
            }
            "startposition" | "spawnflags" if zero(value, 1) => {}
            "targetname" | "hammerid" => {}
            _ => return Err(format!("unsupported stationary-volume field {key}")),
        }
    }
    if !keys.contains("origin") {
        return Err("an explicit finite origin is required".into());
    }
    let geometry = crate::map::origins::checked_geometry(&entity.solids[0], report)
        .ok_or("the cuboid geometry is invalid")?;
    let mut normals = BTreeSet::new();
    let mut corners = BTreeSet::new();
    for face in &geometry.faces {
        let axis = (0..3)
            .find(|&axis| {
                let mut normal = DVec3::ZERO;
                normal[axis] = face.plane.normal[axis].signum();
                face.plane.normal == normal
            })
            .ok_or("only axis-aligned cuboids are supported")?;
        normals.insert((axis, face.plane.normal[axis] > 0.0));
        for point in &face.vertices {
            let mut corner = [false; 3];
            for axis in 0..3 {
                if (point[axis] - geometry.mins[axis]).abs() <= GEOMETRY_EPSILON {
                    corner[axis] = false;
                } else if (point[axis] - geometry.maxs[axis]).abs() <= GEOMETRY_EPSILON {
                    corner[axis] = true;
                } else {
                    return Err("only complete axis-aligned cuboid corners are supported".into());
                }
            }
            corners.insert(corner);
        }
    }
    let size = geometry.maxs - geometry.mins;
    let volume = size.x * size.y * size.z;
    if geometry.faces.len() != 6
        || normals.len() != 6
        || corners.len() != 8
        || size.min_element() <= GEOMETRY_EPSILON
        || !(volume as f32).is_finite()
        || volume as f32 <= 0.0
    {
        return Err("one nondegenerate six-plane, eight-corner cuboid is required".into());
    }
    Ok(())
}

pub(crate) fn admit_source(entity: &Entity, report: &mut Report) -> bool {
    match source(entity, report) {
        Ok(()) => true,
        Err(message) => {
            report
                .diagnostics
                .push(diagnostic(entity.id, entity.origin(), message));
            false
        }
    }
}

pub(crate) fn admit_carrier(map: &mut Map, entity: usize, report: &mut Report) {
    let owner = &map.entities[entity];
    let result: Result<Carrier, String> = (|| {
        if map.water_analog.is_some() {
            return Err("only one retained stationary water volume is supported".into());
        }
        let &[brush] = owner.brushes.as_slice() else {
            return Err("one compiled ordinary SOLID cuboid is required".into());
        };
        let geometry = &map.brushes[brush];
        if geometry.contents != contents::SOLID {
            return Err("the cuboid requires ordinary SOLID nodraw materials".into());
        }
        let native_mins = geometry.mins.as_vec3();
        let native_maxs = geometry.maxs.as_vec3();
        if !native_maxs.cmpgt(native_mins).all() {
            return Err("cuboid bounds collapse in native float coordinates".into());
        }
        let mut material_id = None;
        for side in geometry.sides.iter().filter(|side| !side.bevel) {
            let material = map.materials.get(side.material);
            if !material.found
                || material.contents != contents::SOLID
                || material.draws
                || material.hint
                || material.skip
                || material.surface != (surf::NODRAW | surf::NOLIGHT)
                || !material.unsupported_flags.is_empty()
            {
                return Err(format!(
                    "material {} must be a found ordinary SOLID nodraw material without unsupported compile flags",
                    material.name
                ));
            }
            if material_id.is_some_and(|previous| previous != side.material) {
                return Err("one uniform ordinary SOLID nodraw material is required".into());
            }
            material_id = Some(side.material);
        }
        Ok(Carrier { entity, brush })
    })();
    match result {
        Ok(carrier) => map.water_analog = Some(carrier),
        Err(message) => report
            .diagnostics
            .push(diagnostic(owner.id, owner.origin, message)),
    }
}

pub(crate) fn admit_mode(map: &Map, physics: PhysicsMode, report: &mut Report) {
    if let Some(carrier) = map.water_analog
        && physics != PhysicsMode::Engine
    {
        report
            .diagnostics
            .push(error(map, carrier, "CS:S engine physics is required"));
    }
}

fn touches(a: (DVec3, DVec3), b: (DVec3, DVec3)) -> bool {
    a.0.cmple(b.1 + DVec3::splat(GEOMETRY_EPSILON)).all()
        && b.0.cmple(a.1 + DVec3::splat(GEOMETRY_EPSILON)).all()
}

fn conservative_bounds(pose: Pose, mins: DVec3, maxs: DVec3) -> Result<(DVec3, DVec3), String> {
    let raw = pose.raw_and_stored_bounds(mins, maxs)?;
    let native_pose = Pose {
        offset: pose.offset.as_vec3().as_dvec3(),
        rotation: pose.rotation,
    };
    let native = native_pose.raw_and_stored_bounds(
        mins.min(mins.as_vec3().as_dvec3()),
        maxs.max(maxs.as_vec3().as_dvec3()),
    )?;
    Ok((raw.0.min(native.0), raw.1.max(native.1)))
}

pub(crate) fn admit_world(
    map: &Map,
    tree: &crate::tree::Tree,
    report: &mut Report,
    control: &Control,
) -> Result<(), String> {
    let Some(carrier) = map.water_analog else {
        return Ok(());
    };
    let brush = &map.brushes[carrier.brush];
    let Some(origin) = map.entities[carrier.entity].origin else {
        report.diagnostics.push(error(
            map,
            carrier,
            "the admitted carrier lost its explicit origin",
        ));
        return Ok(());
    };
    let bounds = match conservative_bounds(
        Pose {
            offset: origin,
            rotation: None,
        },
        brush.mins,
        brush.maxs,
    ) {
        Ok(bounds) => bounds,
        Err(message) => {
            report.diagnostics.push(error(map, carrier, message));
            return Ok(());
        }
    };
    let mut poses = BTreeMap::<usize, Pose>::new();
    for (index, other) in map.brushes.iter().enumerate() {
        control.checkpoint()?;
        if index == carrier.brush {
            continue;
        }
        let other_bounds = if other.entity == 0 {
            conservative_bounds(
                Pose {
                    offset: DVec3::ZERO,
                    rotation: None,
                },
                other.mins,
                other.maxs,
            )
        } else {
            let entity = &map.entities[other.entity];
            let pose = if let Some(pose) = poses.get(&other.entity) {
                Ok(*pose)
            } else {
                initial_rotation(&entity.classname, &entity.properties).and_then(|rotation| {
                    let offset = if rotation.is_some() {
                        initial_offset(&entity.properties)?
                    } else {
                        entity.origin.unwrap_or(DVec3::ZERO)
                    };
                    let pose = Pose { offset, rotation };
                    poses.insert(other.entity, pose);
                    Ok(pose)
                })
            };
            pose.and_then(|pose| conservative_bounds(pose, other.mins, other.maxs))
        };
        let other_bounds = match other_bounds {
            Ok(bounds) => bounds,
            Err(message) => {
                report.diagnostics.push(error(
                    map,
                    carrier,
                    format!(
                        "cannot establish initial bounds for entity:{}: {message}",
                        map.entities[other.entity].id
                    ),
                ));
                return Ok(());
            }
        };
        if touches(bounds, other_bounds) {
            report.diagnostics.push(error(map, carrier, format!("conservative bounds touch or overlap solid:{}; keep the stationary water cuboid disjoint from other initial brush geometry", other.id)));
            return Ok(());
        }
    }
    for terrain in &map.terrains {
        control.checkpoint()?;
        let mut bounds_other = (DVec3::MAX, DVec3::MIN);
        for &point in &terrain.mesh.positions {
            control.checkpoint()?;
            bounds_other = (bounds_other.0.min(point), bounds_other.1.max(point));
        }
        for point in terrain.native_positions()? {
            control.checkpoint()?;
            let point = DVec3::from_array(point.map(f64::from));
            bounds_other = (bounds_other.0.min(point), bounds_other.1.max(point));
        }
        if touches(bounds, bounds_other) {
            report.diagnostics.push(error(
                map,
                carrier,
                format!(
                    "conservative terrain bounds touch or overlap solid:{}",
                    terrain.solid
                ),
            ));
            return Ok(());
        }
    }
    let center = (bounds.0 + bounds.1) * 0.5;
    let extent = (bounds.1 - bounds.0) * 0.5;
    let mut pending = vec![0];
    while let Some(index) = pending.pop() {
        control.checkpoint()?;
        let node = &tree.nodes[index];
        if let Some((plane, children)) = node.split {
            let plane = map.planes.get(plane);
            let distance = plane.distance(center);
            let radius = plane.normal.abs().dot(extent);
            if distance + radius >= -GEOMETRY_EPSILON {
                pending.push(children[0]);
            }
            if distance - radius <= GEOMETRY_EPSILON {
                pending.push(children[1]);
            }
        } else if node.contents != contents::EMPTY {
            report.diagnostics.push(error(map, carrier, "the complete cuboid must fit dry open world space; conservative world-leaf bounds touch unsupported contents"));
            return Ok(());
        }
    }
    Ok(())
}

pub(crate) fn model_index(map: &Map, models: &[Model]) -> Result<Option<usize>, String> {
    let Some(carrier) = map.water_analog else {
        return Ok(None);
    };
    models
        .iter()
        .position(|model| model.entity == carrier.entity)
        .filter(|&index| index != 0)
        .map(Some)
        .ok_or_else(|| "stationary water volume is missing its inline model".into())
}

pub(crate) fn admit_models(map: &Map, models: &[Model], report: &mut Report) {
    let Some(carrier) = map.water_analog else {
        return;
    };
    let result = model_index(map, models).and_then(|index| {
        let model = &models[index.ok_or("stationary water volume has no model")?];
        if !model.faces.is_empty() { return Err("stationary water volume must have no rendered faces".into()); }
        let mut occupied = Vec::new();
        for index in model.tree.preorder() {
            let node = &model.tree.nodes[index];
            if node.split.is_some() { continue; }
            if node.contents == contents::SOLID && node.brushes == [carrier.brush] {
                occupied.push(index);
            } else if node.contents != contents::EMPTY || !node.brushes.is_empty() {
                return Err("stationary water volume requires uniquely owned ordinary solid interior and empty exterior leaves".into());
            }
        }
        if occupied.len() != 1 { return Err("stationary water volume requires exactly one occupied inline leaf".into()); }
        if model.tree.brush_leaves(&map.brushes[carrier.brush], &map.planes) != occupied {
            return Err("stationary water brush references must belong only to its occupied inline leaf".into());
        }
        Ok(())
    });
    if let Err(message) = result {
        report.diagnostics.push(error(map, carrier, message));
    }
}

pub(crate) fn leaf_contents(map: &Map, entity: usize, contents: i32) -> i32 {
    if map
        .water_analog
        .is_some_and(|carrier| carrier.entity == entity)
        && contents == contents::SOLID
    {
        contents::WATER
    } else {
        contents
    }
}

pub(crate) fn brush_contents(map: &Map, brush: usize, contents: i32) -> i32 {
    if map
        .water_analog
        .is_some_and(|carrier| carrier.brush == brush)
    {
        contents::WATER
    } else {
        contents
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_offset_rounding_cannot_turn_a_source_gap_into_unchecked_contact() {
        let carrier = (
            DVec3::new(8160.0, -32.0, 32.0),
            DVec3::new(8224.0, 32.0, 96.0),
        );
        let pose = Pose {
            offset: DVec3::new(8224.0002, 0.0, 64.0),
            rotation: None,
        };
        let mins = DVec3::new(0.0, -8.0, -8.0);
        let maxs = DVec3::new(16.0, 8.0, 8.0);
        assert!(!touches(carrier, pose.bounds(mins, maxs).unwrap()));
        assert!(touches(
            carrier,
            conservative_bounds(pose, mins, maxs).unwrap()
        ));
    }
}
