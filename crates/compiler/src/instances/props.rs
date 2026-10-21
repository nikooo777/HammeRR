use super::*;

pub(super) fn admit(entity: &Entity) -> Result<(), String> {
    if !entity.solids.is_empty()
        || !entity.extra.is_empty()
        || entity
            .connections
            .as_ref()
            .is_some_and(|pairs| !pairs.is_empty())
    {
        return Err("Static child props cannot have brushes, connections or opaque blocks".into());
    }
    let mut keys = BTreeSet::new();
    for (key, value) in entity.properties.iter() {
        field(key)?;
        field(value)?;
        let lower = key.to_ascii_lowercase();
        if !keys.insert(lower.clone()) {
            return Err(format!("Static child prop repeats property {key}"));
        }
        if key != lower.as_str() {
            return Err(format!(
                "Static child prop requires canonical property {lower}"
            ));
        }
        match key {
            "classname" | "origin" | "angles" | "model" => {}
            "solid" => {
                if !matches!(value.parse::<i32>(), Ok(0 | 2 | 6)) {
                    return Err("Static child prop solid must be 0, 2 or 6".into());
                }
            }
            "body"
            | "skin"
            | "spawnflags"
            | "generatelightmaps"
            | "ignorenormals"
            | "disableshadows"
            | "screenspacefade"
            | "disablevertexlighting"
            | "disableselfshadowing"
            | "mindxlevel"
            | "maxdxlevel"
            | "rendermode"
            | "renderfx" => {
                if value.parse::<i32>().ok() != Some(0) {
                    return Err(format!("Static child prop requires {key} 0"));
                }
            }
            "angle" | "fademaxdist" => neutral(key, value, 0.0)?,
            "modelscale" | "fadescale" => neutral(key, value, 1.0)?,
            "fademindist" => neutral(key, value, -1.0)?,
            "renderamt" => neutral(key, value, 255.0)?,
            "rendercolor" => {
                if source_formats::render_color::rgb(Some(value))
                    .map_err(|error| error.to_string())?
                    != [255; 3]
                {
                    return Err("Static child prop requires white rendercolor".into());
                }
            }
            "targetname" | "target" | "parentname" | "name" | "globalname" | "killtarget"
            | "lightingorigin" => {
                if !value.is_empty() {
                    return Err(format!("Static child prop requires empty {key}"));
                }
            }
            _ => return Err(format!("Static child prop property {key} is unsupported")),
        }
    }
    if entity.properties.get("classname") != Some("prop_static") {
        return Err("Static child prop requires canonical classname prop_static".into());
    }
    let model = source_formats::gamefs::asset_relative(
        entity
            .properties
            .get("model")
            .ok_or("Static child prop requires model")?,
    )
    .map_err(|error| error.to_string())?;
    if !model.starts_with("models/") || !model.ends_with(".mdl") {
        return Err("Static child prop model must name a models/*.mdl asset".into());
    }
    source_formats::render_color::model_appearance(
        entity.properties.get("rendermode"),
        entity.properties.get("renderamt"),
        entity.properties.get("renderfx"),
    )?;
    check_pose(entity)?;
    Ok(())
}

fn neutral(key: &str, value: &str, expected: f64) -> Result<(), String> {
    if value.parse::<f64>().ok() != Some(expected) {
        return Err(format!("Static child prop requires {key} {expected}"));
    }
    Ok(())
}

pub(super) fn check_pose(entity: &Entity) -> Result<StaticPropPose, String> {
    if entity.properties.get("origin").is_none() {
        return Err("Static child prop requires canonical property origin".into());
    }
    let origin = vector(entity, "origin")?;
    if origin.abs().max_element() > crate::map::MAX_COMPILE_COORD {
        return Err("Static child prop origin exceeds finite Source compile bounds".into());
    }
    vector(entity, "angles")?;
    Ok(StaticPropPose {
        origin,
        angles: crate::static_props::angles(entity)?,
    })
}
