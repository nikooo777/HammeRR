use super::*;

pub(super) fn admit(entity: &Entity) -> Result<(), String> {
    if !entity.solids.is_empty()
        || !entity.extra.is_empty()
        || entity
            .connections
            .as_ref()
            .is_some_and(|pairs| !pairs.is_empty())
    {
        return Err("Child player spawns cannot have brushes, connections or opaque blocks".into());
    }
    let mut keys = BTreeSet::new();
    for (key, value) in entity.properties.iter() {
        field(key)?;
        field(value)?;
        let lower = key.to_ascii_lowercase();
        if !keys.insert(lower.clone()) {
            return Err(format!("Child player spawn repeats property {key}"));
        }
        if key != lower.as_str() {
            return Err(format!(
                "Child player spawn requires canonical property {lower}"
            ));
        }
        match key {
            "classname" | "origin" | "angles" | "angle" => {}
            "spawnflags" if value.parse::<i32>().ok() == Some(0) => {}
            "spawnflags" => return Err("Child player spawn requires integer spawnflags 0".into()),
            "targetname" | "target" | "parentname" | "name" | "globalname" | "killtarget" => {
                if !value.is_empty() {
                    return Err(format!("Child player spawn requires empty {key}"));
                }
            }
            _ => return Err(format!("Child player spawn property {key} is unsupported")),
        }
    }
    if !matches!(
        entity.classname(),
        Some("info_player_terrorist" | "info_player_counterterrorist")
    ) {
        return Err(
            "Child player spawn requires a canonical terrorist or counterterrorist classname"
                .into(),
        );
    }
    check_pose(entity)
}

pub(super) fn check_pose(entity: &Entity) -> Result<(), String> {
    if entity.properties.get("origin").is_none() {
        return Err("Child player spawn requires canonical property origin".into());
    }
    let origin = vector(entity, "origin")?;
    if origin.abs().max_element() > crate::map::MAX_COMPILE_COORD {
        return Err("Child player spawn origin exceeds finite Source compile bounds".into());
    }
    vector(entity, "angles")?;
    if let Some(angle) = entity.properties.get("angle")
        && angle.parse::<f64>().ok() != Some(0.0)
    {
        return Err("Child player spawn legacy angle must be absent or zero".into());
    }
    Ok(())
}
