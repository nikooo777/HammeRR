use super::*;

pub(super) fn is_helper(entity: &Entity) -> bool {
    entity
        .classname()
        .is_some_and(|class| class.eq_ignore_ascii_case("func_viscluster"))
}

pub(super) fn admit(entity: &Entity) -> Result<(), String> {
    let Some(solid) = crate::visclusters::source_solid(entity)? else {
        return Ok(());
    };
    if !entity.extra.is_empty()
        || entity
            .connections
            .as_ref()
            .is_some_and(|pairs| !pairs.is_empty())
    {
        return Err("Child viscluster cannot have opaque blocks or connections".into());
    }
    let mut seen = BTreeSet::new();
    for (key, value) in entity.properties.iter() {
        field(key)?;
        field(value)?;
        let lower = key.to_ascii_lowercase();
        if !seen.insert(lower.clone()) {
            return Err(format!("Child viscluster repeats property {key}"));
        }
        if key != lower {
            return Err(format!(
                "Child viscluster requires canonical property {lower}"
            ));
        }
        match key {
            "classname" | "origin" | "angles" => {}
            "angle" if value.trim().parse::<f64>().ok() == Some(0.0) => {}
            "spawnflags" | "mindxlevel" | "maxdxlevel"
                if value.trim().parse::<u32>().ok() == Some(0) => {}
            "targetname" | "target" | "parentname" | "name" | "globalname" | "killtarget"
            | "lightingorigin"
                if value.trim().is_empty() => {}
            _ => return Err(format!("Child viscluster property {key} is unsupported")),
        }
    }
    if entity.properties.get("classname").is_none() {
        return Err("Child viscluster requires canonical property classname".into());
    }
    crate::visclusters::source_rules(entity, solid)?;
    check_pose(entity)
}

pub(super) fn check_pose(entity: &Entity) -> Result<(), String> {
    if entity.properties.get("origin").is_some()
        && vector(entity, "origin")?.abs().max_element() > crate::map::MAX_COMPILE_COORD
    {
        return Err("Child viscluster origin exceeds finite Source compile bounds".into());
    }
    Ok(())
}
