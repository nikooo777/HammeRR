use super::*;

pub(super) fn admit(entity: &Entity) -> Result<(), String> {
    if !entity.solids.is_empty()
        || !entity.extra.is_empty()
        || entity
            .connections
            .as_ref()
            .is_some_and(|pairs| !pairs.is_empty())
    {
        return Err(
            "Static child fog controllers cannot have brushes, connections or opaque blocks".into(),
        );
    }
    let mut keys = BTreeSet::new();
    for (key, value) in entity.properties.iter() {
        field(key)?;
        field(value)?;
        let lower = key.to_ascii_lowercase();
        if !keys.insert(lower.clone()) {
            return Err(format!(
                "Static child fog controller repeats property {key}"
            ));
        }
        match lower.as_str() {
            "classname" | "origin" | "angles" | "angle" => {
                if key != lower {
                    return Err(format!(
                        "Static child fog controller requires canonical property {lower}"
                    ));
                }
            }
            "fogcolor" | "fogcolor2" | "fogdir" | "fogstart" | "fogend" | "fogmaxdensity"
            | "fogenable" | "fogblend" | "fogradial" | "farz" | "hdrcolorscale"
            | "zoomfogscale" | "foglerptime" | "use_angles" => {}
            "spawnflags" | "mindxlevel" | "maxdxlevel"
                if value.trim().parse::<u32>().ok() == Some(0) => {}
            "targetname" | "target" | "parentname" | "name" | "globalname" | "killtarget"
            | "lightingorigin"
                if value.is_empty() => {}
            _ => {
                return Err(format!(
                    "Static child fog controller property {key} is unsupported"
                ));
            }
        }
    }
    if entity.classname() != Some("env_fog_controller") {
        return Err(
            "Static child fog controller requires canonical classname env_fog_controller".into(),
        );
    }
    map_core::fog::parse_static_ldr(entity)?;
    if map_core::fog::boolean_property(entity, "use_angles")? {
        return Err(
            "Static child fog controller requires use_angles 0; directed fog is unsupported".into(),
        );
    }
    if map_core::fog::number_property(entity, "foglerptime", Some(0.0))? != 0.0 {
        return Err(
            "Static child fog controller requires foglerptime 0; transitions are unsupported"
                .into(),
        );
    }
    if keys.contains("fogcolor2") {
        map_core::fog::color_property(entity, "fogcolor2")?;
    }
    if keys.contains("fogdir") {
        vector(entity, "fogdir")?;
    }
    check_pose(entity)
}

pub(super) fn check_pose(entity: &Entity) -> Result<(), String> {
    if entity.properties.get("origin").is_none() {
        return Err("Static child fog controller requires canonical property origin".into());
    }
    if vector(entity, "origin")?.abs().max_element() > crate::map::MAX_COMPILE_COORD {
        return Err(
            "Static child fog controller origin exceeds finite Source compile bounds".into(),
        );
    }
    vector(entity, "angles")?;
    if let Some(angle) = entity.properties.get("angle")
        && angle.trim().parse::<f64>().ok() != Some(0.0)
    {
        return Err("Static child fog controller legacy angle must be absent or zero".into());
    }
    Ok(())
}
