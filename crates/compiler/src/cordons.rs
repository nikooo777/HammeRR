use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use map_core::{BrushGeometry, Document, compile_region, edit};
use source_formats::{
    bsp::{contents, surf},
    gamefs::GameFs,
    vmf::{Entity, Solid, Vmf},
};

use crate::{Control, Diagnostic, Report, map::origins, materials::Materials};

const MAX_OBJECTS: usize = 16_384;
const MAX_SIDES: usize = 65_536;
const MAX_SIDES_PER_BRUSH: usize = 128;
const SHELL_MATERIAL: &str = "tools/toolsblack";
const BOUNDARY_LIGHTMAP_SCALE: u32 = 256;

#[derive(Clone, Debug, serde::Serialize)]
pub struct RegionSummary {
    pub mins: [f64; 3],
    pub maxs: [f64; 3],
    pub shell_thickness: f64,
    pub boundary_material: String,
    pub included: Vec<String>,
    pub omitted: Vec<String>,
    pub generated_brushes: Vec<u32>,
}

pub(super) fn is_global(classname: &str) -> bool {
    ["light_environment", "water_lod_control"]
        .iter()
        .any(|name| classname.eq_ignore_ascii_case(name))
}

pub(super) struct Prepared<'a> {
    pub vmf: Cow<'a, Vmf>,
    pub region: Option<RegionSummary>,
}

fn selected_geometry(
    solid: &Solid,
    bounds: compile_region::Bounds,
    report: &mut Report,
    control: &Control,
) -> Result<Option<BrushGeometry>, String> {
    control.checkpoint()?;
    if solid.hidden {
        return Ok(None);
    }
    if solid.sides.len() > MAX_SIDES_PER_BRUSH {
        return Err(format!(
            "Compile region supports at most {MAX_SIDES_PER_BRUSH} sides per source brush"
        ));
    }
    let geometry = origins::checked_geometry(solid, report)
        .ok_or("Compile region cannot determine bounds of an invalid source brush")?;
    let (mut mins, mut maxs) = (geometry.mins, geometry.maxs);
    let mut terrain = false;
    for face in &geometry.faces {
        let Some(info) = &solid.sides[face.side].displacement else {
            continue;
        };
        control.checkpoint()?;
        terrain = true;
        let (_, mesh) = crate::map::terrain::checked_mesh(face, info)?;
        for point in mesh.positions {
            mins = mins.min(point);
            maxs = maxs.max(point);
        }
    }
    if !bounds.intersects(mins, maxs) {
        return Ok(None);
    }
    if terrain && (!bounds.contains(mins) || !bounds.contains(maxs)) {
        return Err(format!(
            "Terrain crosses a compile-region boundary; include the complete base and displaced grid or disable the region (solid:{})",
            solid.id
        ));
    }
    Ok(Some(geometry))
}

fn check_liquid_volume(
    solid: &Solid,
    world: Option<(compile_region::Bounds, &BrushGeometry)>,
    materials: &mut Materials<'_>,
    control: &Control,
) -> Result<(), String> {
    if solid.sides.iter().any(|side| side.displacement.is_some()) {
        return Ok(());
    }
    let mut flags = 0;
    for side in &solid.sides {
        control.checkpoint()?;
        let index = materials.load(&side.material)?;
        flags |= materials.get(index).contents;
    }
    if flags & (contents::WATER | contents::SLIME) == 0 {
        return Ok(());
    }
    let Some((bounds, geometry)) = world.filter(|_| flags & contents::DETAIL == 0) else {
        return Err(format!(
            "Compile regions support liquids only in structural world brushes; detail/model liquids require a full-map build (solid:{})",
            solid.id
        ));
    };
    if !bounds.contains(geometry.mins) || !bounds.contains(geometry.maxs) {
        return Err(format!(
            "Liquid brush crosses a compile-region boundary; include the whole volume or disable the region (solid:{})",
            solid.id
        ));
    }
    Ok(())
}

fn selected_helper(
    entity: &Entity,
    bounds: compile_region::Bounds,
    report: &mut Report,
    control: &Control,
) -> Result<bool, String> {
    let classname = entity.classname().unwrap_or("brush helper");
    let (mut mins, mut maxs) = (DVec3::MAX, DVec3::MIN);
    let mut visible = false;
    for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
        control.checkpoint()?;
        visible = true;
        if solid.sides.len() > MAX_SIDES_PER_BRUSH {
            return Err(format!(
                "Compile region supports at most {MAX_SIDES_PER_BRUSH} sides per source brush (entity:{}, solid:{})",
                entity.id, solid.id
            ));
        }
        let geometry = origins::checked_geometry(solid, report).ok_or_else(|| {
            format!(
                "Compile region cannot determine the complete bounds of an invalid {classname} (entity:{}, solid:{})",
                entity.id, solid.id
            )
        })?;
        mins = mins.min(geometry.mins);
        maxs = maxs.max(geometry.maxs);
    }
    if !visible {
        return Ok(entity.solids.is_empty());
    }
    if !bounds.intersects(mins, maxs) {
        return Ok(false);
    }
    if !bounds.contains(mins) || !bounds.contains(maxs) {
        let message = format!(
            "{classname} crosses a compile-region boundary; include every visible brush in the helper or disable the region (entity:{})",
            entity.id
        );
        report.diagnostics.push(Diagnostic::error(
            message.clone(),
            Some(format!("entity:{}", entity.id)),
            Some((mins + maxs) * 0.5),
        ));
        return Err(message);
    }
    Ok(true)
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

#[cfg(test)]
pub(super) fn prepare<'a>(
    source: &'a Vmf,
    fs: &GameFs,
    report: &mut Report,
    control: &Control,
) -> Result<Prepared<'a>, String> {
    prepare_with_material_sources(source, fs, report, control, None)
}

pub(super) fn prepare_with_material_sources<'a>(
    source: &'a Vmf,
    fs: &GameFs,
    report: &mut Report,
    control: &Control,
    material_sources: Option<&crate::asset_sources::BuildMaterialSources>,
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
    let mut materials = match material_sources {
        Some(sources) => {
            Materials::with_build_sources(fs, crate::materials::Requirements::Compile, sources)
        }
        None => Materials::new(fs),
    };
    let shell_index = materials.load(SHELL_MATERIAL)?;
    let shell = materials.get(shell_index);
    if !shell.found
        || !shell.draws
        || shell.contents != contents::SOLID
        || shell.surface & (surf::SKY | surf::SKY2D | surf::WARP) != 0
    {
        return Err("Compile region sealing requires a mounted tools/toolsblack material with solid, rendering semantics".into());
    }
    let mut world = BTreeSet::new();
    let mut entities = BTreeMap::new();
    for solid in &source.world.solids {
        if let Some(geometry) = selected_geometry(solid, bounds, report, control)? {
            check_liquid_volume(solid, Some((bounds, &geometry)), &mut materials, control)?;
            world.insert(solid.id);
        }
    }
    for entity in source.entities.iter().filter(|entity| !entity.hidden) {
        control.checkpoint()?;
        let classname = entity.classname().unwrap_or("");
        if let Some(diagnostic) = crate::map::unresolved_instance_diagnostic(entity) {
            return Err(format!("{} (entity:{})", diagnostic.message, entity.id));
        }
        if classname.eq_ignore_ascii_case("info_overlay")
            || classname.eq_ignore_ascii_case("info_overlay_transition")
            || classname.eq_ignore_ascii_case("info_lighting")
        {
            entities.insert(entity.id, None);
            continue;
        }
        let point = origin(entity)?;
        if ["prop_detail", "detail_prop", "prop_detail_sprite"]
            .iter()
            .any(|name| classname.eq_ignore_ascii_case(name))
        {
            if bounds.contains(point.unwrap_or(DVec3::ZERO)) {
                entities.insert(entity.id, None);
            }
            continue;
        }
        if [
            "func_occluder",
            "func_areaportal",
            "func_areaportalwindow",
            "func_viscluster",
        ]
        .iter()
        .any(|name| classname.eq_ignore_ascii_case(name))
        {
            if selected_helper(entity, bounds, report, control)? {
                entities.insert(entity.id, None);
            }
            continue;
        }
        if classname.eq_ignore_ascii_case("func_detail") {
            let mut retained = BTreeSet::new();
            for solid in &entity.solids {
                if selected_geometry(solid, bounds, report, control)?.is_some() {
                    check_liquid_volume(solid, None, &mut materials, control)?;
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
                    check_liquid_volume(solid, None, &mut materials, control)?;
                }
                entities.insert(entity.id, None);
            }
        } else if is_global(classname) || point.is_none_or(|point| bounds.contains(point)) {
            entities.insert(entity.id, None);
        }
    }
    let mut summary = RegionSummary {
        mins: bounds.mins.to_array(),
        maxs: bounds.maxs.to_array(),
        shell_thickness: compile_region::SHELL_THICKNESS,
        boundary_material: SHELL_MATERIAL.into(),
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
                .map(|(mins, maxs)| {
                    let mut solid = edit::block_solid(tx, mins, maxs, SHELL_MATERIAL)?;
                    for side in &mut solid.sides {
                        side.lightmap_scale = BOUNDARY_LIGHTMAP_SCALE;
                    }
                    Ok(solid)
                })
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
    report.diagnostics.push(Diagnostic::warning(format!("Sealed compile region uses a black solid boundary, includes {} source objects and omits {}. Intersecting world/detail brushes and included brush models stay whole. References are unchanged and outside targets may be absent. Disable the region to check full-map leaks.", summary.included.len(), summary.omitted.len()), None, None));
    Ok(Prepared {
        vmf: Cow::Owned(vmf),
        region: Some(summary),
    })
}

#[cfg(test)]
mod tests;
