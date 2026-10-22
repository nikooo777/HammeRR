use source_formats::bsp::LightingKind;

use super::*;

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Point,
    Spot,
    Environment,
    StaticProp,
    LightingOrigin,
    PlayerSpawn,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Point | Self::Spot => "light",
            Self::Environment => "environment light",
            Self::StaticProp => "prop",
            Self::LightingOrigin => "lighting origin",
            Self::PlayerSpawn => "player spawn",
        }
    }
}

pub(super) fn kind(entity: &Entity) -> Option<Kind> {
    match entity.classname()? {
        "light" => Some(Kind::Point),
        "light_spot" => Some(Kind::Spot),
        "light_environment" => Some(Kind::Environment),
        "prop_static" => Some(Kind::StaticProp),
        "info_lighting" => Some(Kind::LightingOrigin),
        "info_player_terrorist" | "info_player_counterterrorist" => Some(Kind::PlayerSpawn),
        _ => None,
    }
}

pub(super) fn admit(entity: &Entity, kind: Kind) -> Result<(), String> {
    if matches!(kind, Kind::PlayerSpawn) {
        return spawns::admit(entity);
    }
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
            "pitch" if matches!(kind, Kind::Spot | Kind::Environment) => {
                if key != "pitch" {
                    let label = if matches!(kind, Kind::Environment) {
                        "environment light"
                    } else {
                        "spotlight"
                    };
                    return Err(format!(
                        "Static child {label} requires canonical property pitch"
                    ));
                }
            }
            "_light" | "_lighthdr" | "_lightscalehdr" => {}
            "_ambient" | "_ambienthdr" | "_ambientscalehdr" | "sunspreadangle"
                if matches!(kind, Kind::Environment) => {}
            "_constant_attn"
            | "_linear_attn"
            | "_quadratic_attn"
            | "_distance"
            | "_fifty_percent_distance"
            | "_zero_percent_distance"
            | "_hardfalloff"
                if !matches!(kind, Kind::Environment) => {}
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
    if matches!(kind, Kind::Environment) {
        return crate::lighting::validate_environment_light(&mapped);
    }
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
    if matches!(kind, Kind::PlayerSpawn) {
        return spawns::check_pose(entity);
    }
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
    if matches!(kind, Kind::Spot | Kind::Environment) {
        let label = if matches!(kind, Kind::Environment) {
            "environment light"
        } else {
            "spotlight"
        };
        let pitch = scalar(entity, "pitch")?;
        if !(-90.0..=90.0).contains(&pitch) {
            return Err(format!(
                "Static child {label} pitch must be between -90 and 90"
            ));
        }
        if pitch == 0.0 && angles.x != 0.0 {
            return Err(format!(
                "Static child {label} zero pitch requires angles.x 0"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment() -> Entity {
        let mut entity = Entity::new(1000, "light_environment");
        for (key, value) in [
            ("origin", "16 24 128"),
            ("angles", "0 0 0"),
            ("pitch", "0"),
            ("_light", "255 255 255 200"),
            ("_ambient", "255 255 255 20"),
            ("_lightHDR", "-1 -1 -1 1"),
            ("_lightscaleHDR", "1"),
            ("_ambientHDR", "-1 -1 -1 1"),
            ("_AmbientScaleHDR", "1"),
            ("SunSpreadAngle", "0"),
        ] {
            entity.properties.set(key, value);
        }
        entity
    }

    #[test]
    fn environment_admission_preserves_fgd_scalars_and_existing_optional_angles() {
        let mut entity = environment();
        let before = entity.clone();
        admit(&entity, Kind::Environment).unwrap();
        assert_eq!(entity, before);
        entity.properties.remove("angles");
        admit(&entity, Kind::Environment).unwrap();
        for (key, value) in [("_ambientscalehdr", "0.5"), ("SUNSPREADANGLE", "5")] {
            entity.properties.remove(key);
            entity.properties.set(key, value);
        }
        let before = entity.clone();
        admit(&entity, Kind::Environment).unwrap();
        assert_eq!(entity, before);
        entity
            .properties
            .0
            .push(("SunSpreadAngle".into(), "0".into()));
        assert!(
            admit(&entity, Kind::Environment)
                .unwrap_err()
                .contains("repeats property")
        );
    }

    #[test]
    fn environment_uses_both_root_channels_without_widening_point_light_fields() {
        for (key, value) in [
            ("_ambient", "invalid"),
            ("_ambientHDR", "invalid"),
            ("_AmbientScaleHDR", "-1"),
            ("_lightHDR", "invalid"),
            ("_lightscaleHDR", "-1"),
            ("SunSpreadAngle", "91"),
            ("_constant_attn", "0"),
            ("_linear_attn", "0"),
            ("_quadratic_attn", "0"),
            ("_distance", "0"),
            ("_inner_cone", "0"),
        ] {
            let mut entity = environment();
            entity.properties.set(key, value);
            assert!(admit(&entity, Kind::Environment).is_err(), "{key}={value}");
        }
        let mut point = Entity::new(1000, "light");
        point.properties.set("origin", "16 24 128");
        point.properties.set("_light", "255 255 255 200");
        point.properties.set("_constant_attn", "1");
        admit(&point, Kind::Point).unwrap();
        for (key, value) in [
            ("_ambient", "255 255 255 20"),
            ("_ambientHDR", "-1 -1 -1 1"),
            ("_AmbientScaleHDR", "1"),
            ("SunSpreadAngle", "0"),
        ] {
            point.properties.set(key, value);
            assert!(admit(&point, Kind::Point).is_err(), "{key}");
            point.properties.remove(key);
        }
    }
}
