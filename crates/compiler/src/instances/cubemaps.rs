use std::fmt::Write;

use super::*;

pub(super) fn is_helper(entity: &Entity) -> bool {
    entity
        .classname()
        .is_some_and(|class| class.eq_ignore_ascii_case("env_cubemap"))
}

pub(super) fn admit(entity: &Entity) -> Result<(), String> {
    if !entity.solids.is_empty()
        || !entity.extra.is_empty()
        || entity
            .connections
            .as_ref()
            .is_some_and(|pairs| !pairs.is_empty())
    {
        return Err("Child cubemap cannot have brushes, connections or opaque blocks".into());
    }
    let mut seen = BTreeSet::new();
    for (key, value) in entity.properties.iter() {
        field(key)?;
        field(value)?;
        let lower = key.to_ascii_lowercase();
        if !seen.insert(lower.clone()) {
            return Err(format!("Child cubemap repeats property {key}"));
        }
        if key != lower {
            return Err(format!("Child cubemap requires canonical property {lower}"));
        }
        match key {
            "classname" | "origin" | "cubemapsize" | "sides" => {}
            "angles" if vector(entity, key)? == DVec3::ZERO => {}
            "angle" if value.trim().parse::<f64>().ok() == Some(0.0) => {}
            "spawnflags" | "mindxlevel" | "maxdxlevel"
                if value.trim().parse::<u32>().ok() == Some(0) => {}
            "targetname" | "target" | "parentname" | "name" | "globalname" | "killtarget"
            | "lightingorigin"
                if value.trim().is_empty() => {}
            _ => return Err(format!("Child cubemap property {key} is unsupported")),
        }
    }
    if entity.properties.get("classname").is_none() {
        return Err("Child cubemap requires canonical property classname".into());
    }
    crate::map::Map::cubemap_size(entity)?;
    side_ids(entity)?;
    point(entity).map(|_| ())
}

pub(super) fn has_manual_bindings(entity: &Entity) -> bool {
    is_helper(entity)
        && entity
            .properties
            .get("sides")
            .is_some_and(|value| !value.trim().is_empty())
}

fn side_ids(entity: &Entity) -> Result<Vec<u32>, String> {
    field(entity.properties.get("sides").unwrap_or(""))?;
    let mut seen = BTreeSet::new();
    let mut sides = Vec::new();
    for id in crate::cubemap_assets::binding::manual_side_ids(entity) {
        let id = id?;
        if id == 0 || !seen.insert(id) {
            return Err("Child cubemap sides require positive distinct side IDs".into());
        }
        sides.push(id);
    }
    Ok(sides)
}

pub(super) fn charge_bindings(
    work: &mut usize,
    amount: usize,
    control: &Control,
) -> Result<(), String> {
    charge_side_bindings(work, amount, control, "cubemap")
}

pub(super) fn charge_side_bindings(
    work: &mut usize,
    amount: usize,
    control: &Control,
    helper: &str,
) -> Result<(), String> {
    control.checkpoint()?;
    *work = work
        .checked_add(amount)
        .filter(|value| *value <= MAX_GEOMETRY_WORK)
        .ok_or_else(|| {
            format!("Instance {helper} bindings exceed the shared geometry-work limit")
        })?;
    Ok(())
}

pub(super) fn remap_sides(
    original: &Entity,
    local_sides: &BTreeMap<u32, u32>,
    work: &mut usize,
    control: &Control,
) -> Result<String, String> {
    charge_bindings(
        work,
        original.properties.get("sides").map_or(0, str::len),
        control,
    )?;
    remap_side_ids(
        side_ids(original)?,
        local_sides,
        None,
        work,
        control,
        "cubemap",
    )
}

pub fn remap_collapsed_cubemap_sides(
    entity: &Entity,
    sides: &BTreeMap<u32, u32>,
    work: &mut usize,
    control: &Control,
) -> Result<Option<String>, String> {
    control.checkpoint()?;
    if !has_manual_bindings(entity) {
        return Ok(None);
    }
    remap_sides(entity, sides, work, control).map(Some)
}

pub(super) fn remap_side_ids(
    mut sides: Vec<u32>,
    local_sides: &BTreeMap<u32, u32>,
    displacement_sides: Option<&BTreeMap<u32, u32>>,
    work: &mut usize,
    control: &Control,
    helper: &str,
) -> Result<String, String> {
    let mut bytes = 0usize;
    for (index, id) in sides.iter_mut().enumerate() {
        charge_side_bindings(work, 1, control, helper)?;
        *id = local_sides
            .get(id)
            .or_else(|| displacement_sides.and_then(|sides| sides.get(id)))
            .copied()
            .filter(|id| *id != 0)
            .ok_or_else(|| {
                let target = if displacement_sides.is_some() {
                    "ordinary or displacement"
                } else {
                    "ordinary"
                };
                format!(
                    "{helper} sides target side:{id} must name a visible {target} world or func_detail side in the same child"
                )
            })?;
        bytes = bytes
            .checked_add(id.ilog10() as usize + 1 + usize::from(index != 0))
            .filter(|bytes| *bytes <= MAX_FIELD_BYTES)
            .ok_or_else(|| {
                format!("Child {helper} rewritten sides exceed {MAX_FIELD_BYTES} bytes")
            })?;
    }
    charge_side_bindings(work, bytes, control, helper)?;
    let mut rewritten = String::with_capacity(bytes);
    for (index, id) in sides.into_iter().enumerate() {
        control.checkpoint()?;
        if index != 0 {
            rewritten.push(' ');
        }
        write!(rewritten, "{id}").unwrap();
    }
    Ok(rewritten)
}

pub(super) fn point(entity: &Entity) -> Result<DVec3, String> {
    if entity.properties.get("origin").is_none() {
        return Err("Child cubemap requires canonical property origin".into());
    }
    let origin = vector(entity, "origin")?;
    if origin.abs().max_element() > crate::map::MAX_COMPILE_COORD {
        return Err("Child cubemap origin exceeds finite Source compile bounds".into());
    }
    Ok(origin)
}
