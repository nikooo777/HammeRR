use source_formats::bsp::LightingKind;

use super::*;

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Point,
    Spot,
    StaticProp,
    LightingOrigin,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Point | Self::Spot => "light",
            Self::StaticProp => "prop",
            Self::LightingOrigin => "lighting origin",
        }
    }
}

pub(super) fn kind(entity: &Entity) -> Option<Kind> {
    match entity.classname()? {
        "light" => Some(Kind::Point),
        "light_spot" => Some(Kind::Spot),
        "prop_static" => Some(Kind::StaticProp),
        "info_lighting" => Some(Kind::LightingOrigin),
        _ => None,
    }
}

pub(super) fn admit(entity: &Entity, kind: Kind) -> Result<(), String> {
    if matches!(kind, Kind::LightingOrigin) {
        return names::admit(entity);
    }
    if matches!(kind, Kind::StaticProp) {
        return props::admit(entity);
    }
    if !entity.solids.is_empty()
        || !entity.extra.is_empty()
        || entity
            .connections
            .as_ref()
            .is_some_and(|pairs| !pairs.is_empty())
    {
        return Err("Static child lights cannot have brushes, connections or opaque blocks".into());
    }
    let mut keys = BTreeSet::new();
    for (key, value) in entity.properties.iter() {
        field(key)?;
        field(value)?;
        let lower = key.to_ascii_lowercase();
        if !keys.insert(lower.clone()) {
            return Err(format!("Static child light repeats property {key}"));
        }
        match lower.as_str() {
            "classname" | "origin" | "angles" | "angle" => {
                if key != lower.as_str() {
                    return Err(format!(
                        "Static child light requires canonical property {lower}"
                    ));
                }
            }
            "pitch" if matches!(kind, Kind::Spot) => {
                if key != "pitch" {
                    return Err("Static child spotlight requires canonical property pitch".into());
                }
            }
            "_light"
            | "_lighthdr"
            | "_lightscalehdr"
            | "_constant_attn"
            | "_linear_attn"
            | "_quadratic_attn"
            | "_distance"
            | "_fifty_percent_distance"
            | "_zero_percent_distance"
            | "_hardfalloff" => {}
            "_inner_cone" | "_cone" | "_exponent" if matches!(kind, Kind::Spot) => {}
            "style" | "defaultstyle" | "spawnflags" => {
                if value.trim().parse::<f64>().ok() != Some(0.0) {
                    return Err(format!("Static child light requires {key} 0"));
                }
            }
            "targetname" | "target" | "parentname" | "name" | "globalname" | "killtarget"
            | "lightingorigin" | "pattern" => {
                if !value.is_empty() {
                    return Err(format!("Static child light requires empty {key}"));
                }
            }
            _ => return Err(format!("Static child light property {key} is unsupported")),
        }
    }
    if entity.properties.get("classname").is_none() {
        return Err("Static child light requires canonical property classname".into());
    }
    check_pose(entity, kind)?;
    let mapped = crate::map::MapEntity {
        id: entity.id,
        classname: entity.classname().unwrap().to_owned(),
        properties: entity
            .properties
            .iter()
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
        brushes: Vec::new(),
        origin: entity.origin(),
    };
    for lighting in [LightingKind::Ldr, LightingKind::Hdr] {
        let light = crate::lighting::PointLight::from_entity_kind(&mapped, -1, lighting)?;
        if matches!(kind, Kind::Spot) {
            light.with_spotlight(&mapped, &map_core::light::Targets::new())?;
        }
    }
    Ok(())
}

fn scalar(entity: &Entity, key: &str) -> Result<f64, String> {
    entity
        .properties
        .get(key)
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("Static child light {key} must be finite"))
}

pub(super) fn check_pose(entity: &Entity, kind: Kind) -> Result<(), String> {
    if matches!(kind, Kind::LightingOrigin) {
        return names::check_point(entity);
    }
    if matches!(kind, Kind::StaticProp) {
        return props::check_pose(entity).map(|_| ());
    }
    if entity.properties.get("origin").is_none() {
        return Err("Static child light requires canonical property origin".into());
    }
    let origin = vector(entity, "origin")?;
    if origin.abs().max_element() > crate::map::MAX_COMPILE_COORD {
        return Err("Static child light origin exceeds finite Source compile bounds".into());
    }
    let angles = vector(entity, "angles")?;
    if entity.properties.get("angle").is_some() && scalar(entity, "angle")? != 0.0 {
        return Err("Static child light legacy angle must be absent or zero".into());
    }
    if matches!(kind, Kind::Spot) {
        let pitch = scalar(entity, "pitch")?;
        if !(-90.0..=90.0).contains(&pitch) {
            return Err("Static child spotlight pitch must be between -90 and 90".into());
        }
        if pitch == 0.0 && angles.x != 0.0 {
            return Err("Static child spotlight zero pitch requires angles.x 0".into());
        }
    }
    Ok(())
}
