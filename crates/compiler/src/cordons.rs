use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use map_core::{Document, compile_region, edit};
use source_formats::{
    bsp::contents,
    gamefs::GameFs,
    vmf::{Entity, Solid, Vmf},
};

use crate::{Control, Diagnostic, Report, map::origins, materials::Materials};

const MAX_OBJECTS: usize = 16_384;
const MAX_SIDES: usize = 65_536;
const MAX_SIDES_PER_BRUSH: usize = 128;
const SHELL_MATERIAL: &str = "tools/toolsnodraw";
const UNSUPPORTED: &[&str] = &[
    "info_overlay",
    "info_overlay_transition",
    "func_areaportal",
    "func_areaportalwindow",
    "func_occluder",
    "func_instance",
    "func_viscluster",
];

#[derive(Clone, Debug, serde::Serialize)]
pub struct RegionSummary {
    pub mins: [f64; 3],
    pub maxs: [f64; 3],
    pub shell_thickness: f64,
    pub included: Vec<String>,
    pub omitted: Vec<String>,
    pub generated_brushes: Vec<u32>,
}

pub(super) struct Prepared<'a> {
    pub vmf: Cow<'a, Vmf>,
    pub region: Option<RegionSummary>,
}

fn selected_brush(
    solid: &Solid,
    bounds: compile_region::Bounds,
    report: &mut Report,
    control: &Control,
) -> Result<bool, String> {
    control.checkpoint()?;
    if solid.hidden {
        return Ok(false);
    }
    if solid.sides.len() > MAX_SIDES_PER_BRUSH {
        return Err(format!(
            "Compile region supports at most {MAX_SIDES_PER_BRUSH} sides per source brush"
        ));
    }
    let geometry = origins::checked_geometry(solid, report)
        .ok_or("Compile region cannot determine bounds of an invalid source brush")?;
    Ok(bounds.intersects(geometry.mins, geometry.maxs))
}

fn check_materials(
    solid: &Solid,
    materials: &mut Materials<'_>,
    control: &Control,
) -> Result<(), String> {
    for side in &solid.sides {
        control.checkpoint()?;
        let index = materials.load(&side.material)?;
        if materials.get(index).contents & (contents::WATER | contents::SLIME) != 0 {
            return Err(format!(
                "Compile regions do not support retained water/slime brushes yet (solid:{})",
                solid.id
            ));
        }
    }
    Ok(())
}

fn origin(entity: &Entity) -> Result<Option<DVec3>, String> {
    let point = entity.origin();
    if entity.properties.get("origin").is_some()
        && !point.is_some_and(|point| {
            point.is_finite() && point.abs().max_element() <= crate::map::MAX_COMPILE_COORD
        })
    {
        return Err(format!(
            "Compile region requires a valid finite origin within Source bounds (entity:{})",
            entity.id
        ));
    }
    Ok(point)
}

pub(super) fn prepare<'a>(
    source: &'a Vmf,
    fs: &GameFs,
    report: &mut Report,
    control: &Control,
) -> Result<Prepared<'a>, String> {
    let Some(bounds) = compile_region::active(source).map_err(|error| error.to_string())? else {
        return Ok(Prepared {
            vmf: Cow::Borrowed(source),
            region: None,
        });
    };
    if source.object_ids().take(MAX_OBJECTS + 1).count() > MAX_OBJECTS
        || source.side_ids().take(MAX_SIDES + 1).count() > MAX_SIDES
    {
        return Err(format!(
            "Compile region preprocessing is limited to {MAX_OBJECTS} source objects and {MAX_SIDES} source sides"
        ));
    }
    source.validate_ids().map_err(|error| error.to_string())?;
    let mut materials = Materials::new(fs);
    let shell_index = materials.load(SHELL_MATERIAL)?;
    let shell = materials.get(shell_index);
    if !shell.found || shell.draws || shell.contents != contents::SOLID {
        return Err("Compile region sealing requires a mounted tools/toolsnodraw material with solid, non-rendering semantics".into());
    }
    let solids = source.world.solids.iter().chain(
        source
            .entities
            .iter()
            .filter(|entity| !entity.hidden)
            .flat_map(|entity| &entity.solids),
    );
    if solids
        .filter(|solid| !solid.hidden)
        .any(|solid| solid.sides.iter().any(|side| side.displacement.is_some()))
    {
        return Err("Compile regions do not support visible terrain yet; disable the region for a full-map build".into());
    }
    let mut world = BTreeSet::new();
    let mut entities = BTreeMap::new();
    for solid in &source.world.solids {
        if selected_brush(solid, bounds, report, control)? {
            check_materials(solid, &mut materials, control)?;
            world.insert(solid.id);
        }
    }
    for entity in source.entities.iter().filter(|entity| !entity.hidden) {
        control.checkpoint()?;
        let classname = entity.classname().unwrap_or("");
        if UNSUPPORTED
            .iter()
            .any(|name| classname.eq_ignore_ascii_case(name))
        {
            return Err(format!(
                "Compile regions do not support {classname} yet (entity:{}); disable the region for a full-map build",
                entity.id
            ));
        }
        let point = origin(entity)?;
        if classname.eq_ignore_ascii_case("func_detail") {
            let mut retained = BTreeSet::new();
            for solid in &entity.solids {
                if selected_brush(solid, bounds, report, control)? {
                    check_materials(solid, &mut materials, control)?;
                    retained.insert(solid.id);
                }
            }
            if !retained.is_empty() {
                entities.insert(entity.id, Some(retained));
            }
        } else if entity.solids.iter().any(|solid| !solid.hidden) {
            let plan = origins::prepare_origins(&mut materials, entity, report, control)?;
            if report.has_errors() {
                return Err("Compile region cannot resolve a brush-entity pivot".into());
            }
            if bounds.contains(plan.position.unwrap_or(DVec3::ZERO)) {
                for solid in entity
                    .solids
                    .iter()
                    .filter(|solid| !solid.hidden && !plan.volumes.contains(&solid.id))
                {
                    check_materials(solid, &mut materials, control)?;
                }
                entities.insert(entity.id, None);
            }
        } else if classname.eq_ignore_ascii_case("light_environment")
            || point.is_none_or(|point| bounds.contains(point))
        {
            entities.insert(entity.id, None);
        }
    }
    let mut summary = RegionSummary {
        mins: bounds.mins.to_array(),
        maxs: bounds.maxs.to_array(),
        shell_thickness: compile_region::SHELL_THICKNESS,
        included: Vec::new(),
        omitted: Vec::new(),
        generated_brushes: Vec::new(),
    };
    for solid in &source.world.solids {
        let list = if world.contains(&solid.id) {
            &mut summary.included
        } else {
            &mut summary.omitted
        };
        list.push(format!("solid:{}", solid.id));
    }
    for entity in &source.entities {
        let list = if entities.contains_key(&entity.id) {
            &mut summary.included
        } else {
            &mut summary.omitted
        };
        list.push(format!("entity:{}", entity.id));
    }
    let mut document = Document::try_new(source.clone()).map_err(|error| error.to_string())?;
    let shell = document
        .edit("compile region boundary", |tx| {
            bounds
                .shell()
                .into_iter()
                .map(|(mins, maxs)| edit::block_solid(tx, mins, maxs, SHELL_MATERIAL))
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| error.to_string())?;
    let mut vmf = document.into_vmf();
    vmf.world.solids.retain(|solid| world.contains(&solid.id));
    vmf.entities.retain_mut(|entity| {
        let Some(retained) = entities.get(&entity.id) else {
            return false;
        };
        if let Some(retained) = retained {
            entity.solids.retain(|solid| retained.contains(&solid.id));
        }
        true
    });
    summary.generated_brushes = shell.iter().map(|solid| solid.id).collect();
    vmf.world.solids.extend(shell);
    report.diagnostics.push(Diagnostic::warning(format!("Sealed compile region includes {} source objects and omits {}. Intersecting world/detail brushes and included brush models stay whole. References are unchanged and outside targets may be absent. Disable the region to check full-map leaks.", summary.included.len(), summary.omitted.len()), None, None));
    Ok(Prepared {
        vmf: Cow::Owned(vmf),
        region: Some(summary),
    })
}

#[cfg(test)]
mod tests;
