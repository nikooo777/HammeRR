use glam::DMat3;
use source_formats::bsp::{contents, surf};
use source_formats::gamefs::GameFs;

use super::*;
use crate::asset_sources::BuildMaterialSources;
use crate::materials::{MaterialInfo, Materials, Requirements};

pub(crate) struct Shape {
    pub brushes: usize,
    pub sides: usize,
    pub geometry_work: usize,
}

pub(super) fn is_model(entity: &Entity) -> bool {
    classname(entity).is_some()
}

pub(super) fn classname(entity: &Entity) -> Option<&'static str> {
    match entity.classname() {
        Some(class) if class.eq_ignore_ascii_case("func_brush") => Some("func_brush"),
        Some(class) if class.eq_ignore_ascii_case("func_wall") => Some("func_wall"),
        _ => None,
    }
}

pub(crate) fn shape(entity: &Entity) -> Result<Option<Shape>, String> {
    let mut shape = Shape {
        brushes: 0,
        sides: 0,
        geometry_work: 0,
    };
    for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
        if solid.sides.len() > MAX_BRUSH_SIDES {
            return Err(format!(
                "Child brush model exceeds {MAX_BRUSH_SIDES} sides per brush"
            ));
        }
        if solid.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(
                "Child brush models require ordinary convex brushes without terrain".into(),
            );
        }
        shape.brushes += 1;
        shape.sides = shape
            .sides
            .checked_add(solid.sides.len())
            .filter(|sides| *sides <= MAX_SIDES)
            .ok_or("Child brush model exceeds the combined side limit")?;
        shape.geometry_work = shape
            .geometry_work
            .checked_add(solid.sides.len().pow(3))
            .filter(|work| *work <= MAX_GEOMETRY_WORK)
            .ok_or("Child brush model exceeds the geometry-work limit")?;
        if shape.brushes >= MAX_OBJECTS {
            return Err("Child brush model exceeds the combined object limit".into());
        }
    }
    if shape.brushes == 0 {
        return if entity.solids.is_empty() {
            Err("Child brush models require visible ordinary convex brushes".into())
        } else {
            Ok(None)
        };
    }
    Ok(Some(shape))
}

fn zero_angles(raw: &str) -> Result<(), String> {
    if raw.len() > crate::brush_model_pose::MAX_INITIAL_POSE_BYTES || !raw.is_ascii() {
        return Err("Child brush model angles require at most 128 ASCII bytes".into());
    }
    let mut numbers = raw.split_ascii_whitespace();
    for _ in 0..3 {
        let number = numbers
            .next()
            .ok_or("Child brush model angles require three zero components")?;
        if crate::brush_model_pose::component(number)? != 0.0 {
            return Err("Child brush model source angles must be zero".into());
        }
    }
    if numbers.next().is_some() {
        return Err("Child brush model angles require three zero components".into());
    }
    Ok(())
}

pub(super) fn admit(entity: &Entity) -> Result<(), String> {
    let classname = classname(entity)
        .ok_or("Child brush model requires canonical classname func_brush or func_wall")?;
    admit_fields(entity, classname, classname == "func_brush", false)?;
    if shape(entity)?.is_some() {
        crate::brush_model_pose::initial_rotation(classname, &entity.properties.0)?;
    }
    Ok(())
}

pub(crate) fn admit_stationary(entity: &Entity, rotation: bool) -> Result<(), String> {
    if entity.classname() != Some("func_brush") {
        return Err("Child brush model requires canonical classname func_brush".into());
    }
    admit_fields(entity, "func_brush", rotation, true)
}

fn admit_fields(
    entity: &Entity,
    classname: &str,
    rotation: bool,
    require_origin: bool,
) -> Result<(), String> {
    if shape(entity)?.is_none() {
        return Ok(());
    }
    if !entity.extra.is_empty()
        || entity
            .connections
            .as_ref()
            .is_some_and(|pairs| !pairs.is_empty())
    {
        return Err("Child brush models cannot have connections or opaque blocks".into());
    }
    let fields = [
        "classname",
        "origin",
        "angles",
        "spawnflags",
        "vrad_brush_cast_shadows",
        "renderfx",
        "rendermode",
        "disableshadows",
        "disablereceiveshadows",
        "renderamt",
        "rendercolor",
        "targetname",
        "target",
        "parentname",
        "name",
        "globalname",
        "killtarget",
        "lightingorigin",
        "mindxlevel",
        "maxdxlevel",
    ];
    let brush_fields = [
        "Solidity",
        "StartDisabled",
        "solidbsp",
        "invert_exclusion",
        "InputFilter",
        "excludednpc",
    ];
    let class_fields = if classname == "func_brush" {
        brush_fields.as_slice()
    } else {
        &[]
    };
    let mut seen = 0_u32;
    for (key, value) in entity.properties.iter() {
        field(key)?;
        field(value)?;
        let (index, canonical) = fields
            .iter()
            .chain(class_fields)
            .enumerate()
            .find(|(_, field)| field.eq_ignore_ascii_case(key))
            .ok_or_else(|| format!("Child brush model property {key} is unsupported"))?;
        let bit = 1 << index;
        if seen & bit != 0 {
            return Err(format!("Child brush model repeats property {key}"));
        }
        seen |= bit;
        if key != *canonical {
            return Err(format!(
                "Child brush model requires canonical property {canonical}"
            ));
        }
        let neutral = match key {
            "classname" => value == classname,
            "origin" => true,
            "angles" => {
                if !rotation {
                    zero_angles(value)?;
                }
                true
            }
            "spawnflags" if classname == "func_brush" => {
                matches!(value.trim().parse::<u32>(), Ok(0 | 2))
            }
            "spawnflags" => value.trim().parse::<u32>() == Ok(0),
            "Solidity"
            | "StartDisabled"
            | "solidbsp"
            | "invert_exclusion"
            | "InputFilter"
            | "renderfx"
            | "rendermode"
            | "disableshadows"
            | "disablereceiveshadows"
            | "mindxlevel"
            | "maxdxlevel" => value.trim().parse::<u32>() == Ok(0),
            "vrad_brush_cast_shadows" if classname == "func_brush" => {
                matches!(value.trim().parse::<u32>(), Ok(0 | 1))
            }
            "vrad_brush_cast_shadows" => value.trim().parse::<u32>() == Ok(0),
            "renderamt" => value.trim().parse::<u32>() == Ok(255),
            "rendercolor" => vector(entity, "rendercolor")? == DVec3::splat(255.0),
            "excludednpc" | "targetname" | "target" | "parentname" | "name" | "globalname"
            | "killtarget" | "lightingorigin" => value.is_empty(),
            _ => unreachable!(),
        };
        if !neutral {
            return Err(format!("Child brush model property {key} must be neutral"));
        }
    }
    if entity.properties.get("classname") != Some(classname) {
        return Err(format!(
            "Child brush model requires canonical classname {classname}"
        ));
    }
    if require_origin || entity.properties.get("origin").is_some() {
        origin(entity)?;
    }
    Ok(())
}

pub(crate) fn origin(entity: &Entity) -> Result<DVec3, String> {
    if entity.properties.get("origin").is_none() {
        return Err("Child brush model requires explicit canonical origin".into());
    }
    let point = vector(entity, "origin")?;
    if point.abs().max_element() > crate::map::MAX_COMPILE_COORD {
        return Err("Child brush model origin exceeds finite Source compile bounds".into());
    }
    Ok(point)
}

pub(super) fn check_source(entity: &Entity, control: &Control) -> Result<(), String> {
    for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
        control.checkpoint()?;
        checked_solid_controlled(solid, control)?;
    }
    Ok(())
}

pub(super) fn plan_origin(
    materials: &mut Materials<'_>,
    entity: &Entity,
    control: &Control,
    brush_texture_shadows: bool,
    sources: &BuildMaterialSources,
    checked_cutouts: &mut std::collections::BTreeSet<usize>,
) -> Result<crate::map::origins::OriginPlan, String> {
    let mut report = Report::default();
    let plan = crate::map::origins::prepare_origins(materials, entity, &mut report, control)?;
    if let Some(error) = report
        .diagnostics
        .iter()
        .find(|error| error.level == crate::Level::Error)
    {
        return Err(format!(
            "{}: {}",
            error.object.as_deref().unwrap_or("entity"),
            error.message
        ));
    }
    if plan.position.is_none() {
        return Err(
            "Child brush model requires an explicit origin or one visible resolved origin brush"
                .into(),
        );
    }
    let mut remaining = 0;
    for solid in entity
        .solids
        .iter()
        .filter(|solid| !solid.hidden && !plan.volumes.contains(&solid.id))
    {
        remaining += 1;
        validate_solid_materials(
            materials,
            solid,
            entity,
            control,
            brush_texture_shadows,
            sources,
            checked_cutouts,
        )
        .map_err(|error| format!("Child brush-model material: solid {}: {error}", solid.id))?;
    }
    if remaining == 0 {
        return Err(
            "An origin brush requires compiled brush geometry in the same runtime entity".into(),
        );
    }
    Ok(plan)
}

pub(super) fn check_pose(entity: &Entity, control: &Control) -> Result<(), String> {
    let pivot = origin(entity)?;
    for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
        control.checkpoint()?;
        let geometry = checked_solid_geometry_controlled(solid, control)?;
        let mins = geometry.mins - pivot;
        let maxs = geometry.maxs - pivot;
        if !mins.is_finite()
            || !maxs.is_finite()
            || mins.abs().max(maxs.abs()).max_element() > crate::map::MAX_COMPILE_COORD
        {
            return Err(format!(
                "Child brush model solid {} local coordinates exceed finite Source compile bounds; move its effective origin closer to the brushes",
                solid.id,
            ));
        }
    }
    Ok(())
}

pub(super) fn lower_initial_pose(
    entity: &mut Entity,
    rotation: DMat3,
    control: &Control,
) -> Result<(), String> {
    let pivot = origin(entity)?;
    let offset = crate::brush_model_pose::initial_offset(&entity.properties.0)?;
    let pose = crate::brush_model_pose::Pose {
        offset,
        rotation: Some(rotation),
    };
    let translation = offset - rotation * pivot;
    for solid in entity.solids.iter_mut().filter(|solid| !solid.hidden) {
        control.checkpoint()?;
        let geometry = checked_solid_geometry_controlled(solid, control)?;
        pose.raw_and_stored_bounds(geometry.mins - pivot, geometry.maxs - pivot)?;
        for side in &mut solid.sides {
            control.checkpoint()?;
            for point in &mut side.plane.0 {
                *point = pose.point(*point - pivot);
            }
            for axis in [&mut side.u_axis, &mut side.v_axis] {
                axis.axis = rotation * axis.axis;
                axis.offset -= translation.dot(axis.axis) / axis.scale;
            }
        }
    }
    entity
        .properties
        .set("origin", source_formats::vmf::format_vec3(offset));
    entity.properties.set("angles", "0 0 0");
    check_pose(entity, control)
}

pub(crate) fn ordinary(info: &MaterialInfo) -> bool {
    info.found
        && info.contents == contents::SOLID
        && info.surface & !(surf::NODRAW | surf::NOLIGHT | surf::BUMPLIGHT) == 0
        && !info.hint
        && !info.skip
        && info.unsupported_flags.is_empty()
}

fn validate_solid_materials(
    materials: &mut Materials<'_>,
    solid: &Solid,
    entity: &Entity,
    control: &Control,
    brush_texture_shadows: bool,
    sources: &BuildMaterialSources,
    checked_cutouts: &mut std::collections::BTreeSet<usize>,
) -> Result<(), String> {
    let brush_texture_shadows = brush_texture_shadows && entity.classname() == Some("func_brush");
    for side in &solid.sides {
        control.checkpoint()?;
        let index = materials.load(&side.material)?;
        let info = materials.get(index);
        if !ordinary(info) {
            if brush_texture_shadows {
                crate::brush_shadow_casters::request(entity)?;
            }
            if brush_texture_shadows && crate::brush_shadow_casters::binary_cutout(info) {
                if checked_cutouts.insert(index) {
                    crate::brush_shadow_casters::validate_material(
                        &side.material.trim().replace('\\', "/").to_ascii_lowercase(),
                        sources,
                        control,
                        true,
                    )?;
                }
                continue;
            }
            return Err(format!(
                "{} requires ordinary SOLID contents without origin, liquid or special surface semantics",
                side.material,
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_materials<'a>(
    entities: impl Iterator<Item = &'a Entity>,
    fs: &GameFs,
    sources: &BuildMaterialSources,
    control: &Control,
    brush_texture_shadows: bool,
) -> Result<(), Diagnostic> {
    let mut materials = Materials::with_build_sources(fs, Requirements::GeometrySemantics, sources);
    let mut checked_cutouts = std::collections::BTreeSet::new();
    for entity in entities {
        for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
            let result = validate_solid_materials(
                &mut materials,
                solid,
                entity,
                control,
                brush_texture_shadows,
                sources,
                &mut checked_cutouts,
            );
            if let Err(message) = result {
                return Err(Diagnostic::error(
                    format!("Child brush-model material: {message}"),
                    Some(format!("solid:{}", solid.id)),
                    entity.origin(),
                ));
            }
        }
    }
    Ok(())
}
