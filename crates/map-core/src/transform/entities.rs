use std::collections::BTreeSet;

use glam::{DMat3, DQuat, DVec3, EulerRot};
use source_formats::vmf::{Entity, format_number, format_vec3, parse_vec3};

use super::{Mapping, invalid};
use crate::EditError;

const FRAME_ROUNDOFF: f64 = 1e-10;
const COMMON_FIELDS: &[&str] = &[
    "classname",
    "origin",
    "angles",
    "angle",
    "hammerid",
    "targetname",
    "target",
    "parentname",
    "name",
    "globalname",
    "killtarget",
    "spawnflags",
    "mindxlevel",
    "maxdxlevel",
];
const MODEL_FIELDS: &[&str] = &[
    "model",
    "modelscale",
    "skin",
    "body",
    "solid",
    "renderamt",
    "rendercolor",
    "rendermode",
    "renderfx",
    "disablereceiveshadows",
    "disableshadows",
    "disableselfshadowing",
    "disablevertexlighting",
    "disableflashlight",
    "ignorenormals",
    "lightingorigin",
    "generatelightmaps",
    "lightmapresolutionx",
    "lightmapresolutiony",
    "fademindist",
    "fademaxdist",
    "fadescale",
    "screenspacefade",
    "mincpulevel",
    "maxcpulevel",
    "mingpulevel",
    "maxgpulevel",
    "defaultanim",
    "randomanimation",
    "minanimtime",
    "maxanimtime",
    "holdanimation",
    "setbodygroup",
    "disablebonefollowers",
    "explodedamage",
    "exploderadius",
    "health",
    "massscale",
    "inertiascale",
    "physdamagescale",
    "damagetoenablemotion",
    "forcetoenablemotion",
    "pressureoverride",
    "pressuredelay",
    "overridescript",
    "damagefilter",
    "startdisabled",
    "minhealthdmg",
    "damagetype",
    "nodamageforces",
    "puntsound",
    "physicsmode",
    "shadowcastdist",
    "performancemode",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Position,
    Light,
    Direction,
    Frame,
    Model,
    Instance,
}

fn kind(classname: &str) -> Option<Kind> {
    match classname {
        "info_null" | "info_lighting" | "env_cubemap" => Some(Kind::Position),
        "light" => Some(Kind::Light),
        "light_spot" | "light_environment" => Some(Kind::Direction),
        "info_target"
        | "info_landmark"
        | "info_player_start"
        | "info_player_deathmatch"
        | "info_player_teamspawn"
        | "info_player_terrorist"
        | "info_player_counterterrorist" => Some(Kind::Frame),
        "prop_static"
        | "prop_detail"
        | "detail_prop"
        | "prop_dynamic"
        | "prop_dynamic_override"
        | "prop_physics"
        | "prop_physics_override"
        | "prop_physics_multiplayer" => Some(Kind::Model),
        "func_instance" => Some(Kind::Instance),
        _ => None,
    }
}

fn allowed_field(kind: Kind, classname: &str, key: &str) -> bool {
    if kind == Kind::Instance {
        return matches!(
            key,
            "classname" | "origin" | "angles" | "file" | "fixup_style" | "targetname" | "name"
        ) || key.strip_prefix("replace").is_some_and(|suffix| {
            matches!(
                suffix,
                "01" | "02" | "03" | "04" | "05" | "06" | "07" | "08" | "09" | "10"
            )
        });
    }
    if COMMON_FIELDS.contains(&key) {
        return true;
    }
    match kind {
        Kind::Position => classname == "env_cubemap" && matches!(key, "cubemapsize" | "sides"),
        Kind::Light | Kind::Direction => {
            crate::light::value_kind(classname, key).is_some()
                || matches!(key, "style" | "defaultstyle" | "pattern")
                || (kind == Kind::Direction && key == "pitch")
        }
        Kind::Frame => {
            classname == "info_player_teamspawn"
                && matches!(
                    key,
                    "startdisabled"
                        | "teamnum"
                        | "controlpoint"
                        | "round_bluespawn"
                        | "round_redspawn"
                )
        }
        Kind::Model => MODEL_FIELDS.contains(&key),
        Kind::Instance => false,
    }
}

fn standard_scalar(key: &str, value: &str) -> bool {
    match key {
        "teamnum" | "minhealthdmg" => value.trim().parse::<i32>().is_ok(),
        "startdisabled" | "nodamageforces" | "damagetype" => {
            matches!(value.trim().parse::<i32>(), Ok(0 | 1))
        }
        "physicsmode" => matches!(value.trim().parse::<i32>(), Ok(0..=3)),
        "pressuredelay" => value.trim().parse::<f32>().is_ok_and(f32::is_finite),
        _ => true,
    }
}

fn finite_angles(entity: &Entity) -> Result<(), EditError> {
    if let Some(value) = entity.properties.get("angles")
        && !parse_vec3(value)
            .is_some_and(|angles| angles.is_finite() && angles.as_vec3().is_finite())
    {
        return Err(invalid(format!(
            "Entity {} has invalid native angles",
            entity.id
        )));
    }
    for key in ["angle", "pitch"] {
        if let Some(value) = entity.properties.get(key)
            && !value
                .trim()
                .parse::<f64>()
                .is_ok_and(|value| value.is_finite() && (value as f32).is_finite())
        {
            return Err(invalid(format!(
                "Entity {} has invalid native {key}",
                entity.id
            )));
        }
    }
    Ok(())
}

fn validate(entity: &Entity) -> Result<Kind, EditError> {
    let classname = entity.classname().unwrap_or_default().to_ascii_lowercase();
    let kind = kind(&classname).ok_or_else(|| {
        invalid(format!(
            "Entity {} class {classname:?} has no supported point reflection/shear semantics",
            entity.id
        ))
    })?;
    if !entity.extra.is_empty() {
        return Err(invalid(format!(
            "Entity {} has opaque blocks; point reflection/shear cannot transform them safely",
            entity.id
        )));
    }
    let mut keys = BTreeSet::new();
    for (key, value) in entity.properties.iter() {
        let lower = key.to_ascii_lowercase();
        if !keys.insert(lower.clone()) {
            return Err(invalid(format!(
                "Entity {} repeats property {key}; resolve it before point reflection/shear",
                entity.id
            )));
        }
        if !allowed_field(kind, &classname, &lower) {
            return Err(invalid(format!(
                "Entity {} property {key} has no supported point reflection/shear semantics",
                entity.id
            )));
        }
        if !standard_scalar(&lower, value) {
            return Err(invalid(format!(
                "Entity {} property {key} needs a valid native scalar before point reflection/shear",
                entity.id
            )));
        }
    }
    if entity
        .properties
        .get("parentname")
        .is_some_and(|value| !value.is_empty())
    {
        return Err(invalid(format!(
            "Entity {} is parented; point reflection/shear requires an unparented world-space pose",
            entity.id
        )));
    }
    if kind == Kind::Direction
        && entity
            .properties
            .get("target")
            .is_some_and(|value| !value.is_empty())
    {
        return Err(invalid(format!(
            "Entity {} uses a light target; point reflection/shear requires an untargeted authored direction",
            entity.id
        )));
    }
    if matches!(kind, Kind::Frame | Kind::Model | Kind::Instance)
        && entity.properties.get("angle").is_some()
        && entity.properties.get("angles").is_some()
    {
        return Err(invalid(format!(
            "Entity {} has both angle and angles; resolve its Source frame before point reflection/shear",
            entity.id
        )));
    }
    if kind == Kind::Model {
        let model = entity
            .properties
            .get("model")
            .ok_or_else(|| invalid(format!("Entity {} needs an authored MDL model", entity.id)))?;
        let path = source_formats::gamefs::asset_relative(model.trim())
            .map_err(|error| invalid(format!("Entity {} model: {error}", entity.id)))?;
        if !path.starts_with("models/") || !path.ends_with(".mdl") {
            return Err(invalid(format!(
                "Entity {} point reflection/shear requires a models/*.mdl asset",
                entity.id
            )));
        }
        source_formats::model_scale::scale(entity)
            .map_err(|error| invalid(format!("Entity {}: {error}", entity.id)))?;
    }
    finite_angles(entity)?;
    if matches!(kind, Kind::Frame | Kind::Model | Kind::Instance)
        && let Some(angle) = entity
            .properties
            .get("angle")
            .and_then(|value| value.trim().parse::<f64>().ok())
        && angle < 0.0
        && angle != -1.0
        && angle != -2.0
    {
        return Err(invalid(format!(
            "Entity {} has an unsupported negative legacy angle; use explicit Source angles or the exact -1/-2 vertical directions before point reflection/shear",
            entity.id
        )));
    }
    Ok(kind)
}

fn checked_point(point: DVec3, id: u32) -> Result<DVec3, EditError> {
    if !point.is_finite() || point.abs().max_element() > crate::primitive::MAX_COORDINATE {
        return Err(invalid(format!(
            "Entity {id} point reflection/shear exceeds finite ±65536 editor bounds"
        )));
    }
    Ok(point)
}

fn angles_from_frame(frame: DMat3) -> Result<DVec3, EditError> {
    let orientation = DQuat::from_mat3(&frame).normalize();
    let (yaw, pitch, roll) = orientation.to_euler(EulerRot::ZYX);
    let angles = DVec3::new(pitch, yaw, roll).map(|value| {
        let degrees = value.to_degrees();
        if degrees.abs() < 1e-9 { 0.0 } else { degrees }
    });
    let rebuilt = source_frame(angles);
    if !angles.is_finite() || !rebuilt.abs_diff_eq(frame, FRAME_ROUNDOFF) {
        return Err(invalid(
            "Point transform cannot encode its Source orientation faithfully",
        ));
    }
    Ok(angles)
}

fn source_frame(angles: DVec3) -> DMat3 {
    let angles = angles.map(|value| value.rem_euclid(360.0).to_radians());
    DMat3::from_quat(DQuat::from_euler(
        EulerRot::ZYX,
        angles.y,
        angles.x,
        angles.z,
    ))
}

fn direction(mapping: &Mapping, entity: &mut Entity, origin: DVec3) -> Result<(), EditError> {
    let direction = crate::light::direction(
        entity.properties.get("angles"),
        entity.properties.get("angle"),
        entity.properties.get("pitch"),
        origin,
        None,
    )
    .map_err(|error| invalid(format!("Entity {}: {error}", entity.id)))?;
    let direction = (mapping.orientation_linear * direction)
        .try_normalize()
        .filter(|direction| direction.is_finite())
        .ok_or_else(|| invalid("Point transform produces an invalid light direction"))?;
    let roll = entity
        .properties
        .get("angles")
        .and_then(parse_vec3)
        .map_or(0.0, |angles| angles.z);
    let yaw = direction.y.atan2(direction.x).to_degrees();
    let pitch = direction
        .z
        .atan2(direction.x.hypot(direction.y))
        .to_degrees();
    entity
        .properties
        .set("angles", format_vec3(DVec3::new(0.0, yaw, roll)));
    entity.properties.remove("angle");
    entity.properties.set("pitch", format_number(pitch));
    Ok(())
}

pub(super) fn deform(
    mapping: &Mapping,
    entity: &mut Entity,
    rotate: bool,
) -> Result<(), EditError> {
    let kind = validate(entity)?;
    let origin = crate::edit::checked_entity_origin(entity)?
        .ok_or_else(|| invalid(format!("Entity {} has no origin to transform", entity.id)))?;
    let origin = checked_point(origin, entity.id)?;
    let transformed = checked_point(mapping.point(origin)?, entity.id)?;
    match kind {
        Kind::Position | Kind::Light => {
            if rotate {
                mapping.orientation(entity)?;
            }
        }
        Kind::Direction => direction(mapping, entity, origin)?,
        Kind::Frame | Kind::Model | Kind::Instance => {
            if !mapping.rigid_orientation {
                return Err(invalid(format!(
                    "Entity {} needs a proper rigid Source frame; odd reflection and nonzero shear cannot be encoded as angles or intrinsic model/instance deformation",
                    entity.id
                )));
            }
            let angles = crate::query::point_entity_angles(entity).ok_or_else(|| {
                invalid(format!("Entity {} has an invalid Source frame", entity.id))
            })?;
            let angles = angles_from_frame(mapping.orientation_linear * source_frame(angles))?;
            entity.properties.set("angles", format_vec3(angles));
            entity.properties.remove("angle");
        }
    }
    entity.properties.set("origin", format_vec3(transformed));
    Ok(())
}
