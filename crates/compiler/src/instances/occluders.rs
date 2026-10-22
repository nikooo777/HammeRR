use super::*;
use crate::map::occluders::{
    MAX_BRUSHES_PER_OCCLUDER, MAX_POLYGONS_PER_OCCLUDER, MAX_VERTICES_PER_OCCLUDER,
};

pub(super) struct Shape {
    pub brushes: usize,
    pub sides: usize,
}

pub(super) fn is_helper(entity: &Entity) -> bool {
    entity
        .classname()
        .is_some_and(|class| class.eq_ignore_ascii_case("func_occluder"))
}

pub(super) fn shape(entity: &Entity) -> Result<Option<Shape>, String> {
    let mut shape = Shape {
        brushes: 0,
        sides: 0,
    };
    for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
        shape.brushes += 1;
        shape.sides = shape
            .sides
            .checked_add(solid.sides.len())
            .ok_or("Child occluder side count overflow")?;
        if shape.brushes > MAX_BRUSHES_PER_OCCLUDER {
            return Err("Child occluders require 1–16 visible convex brushes per entity".into());
        }
        if shape.sides > MAX_POLYGONS_PER_OCCLUDER {
            return Err(
                "Child occluder input exceeds the combined 256-side per-entity budget".into(),
            );
        }
        if solid.sides.iter().any(|side| side.displacement.is_some()) {
            return Err("Child occluders require ordinary convex geometry without terrain".into());
        }
    }
    if shape.brushes == 0 {
        return if entity.solids.is_empty() {
            Err("Child occluders require 1–16 visible convex brushes per entity".into())
        } else {
            Ok(None)
        };
    }
    Ok(Some(shape))
}

pub(super) fn admit(entity: &Entity) -> Result<(), String> {
    if shape(entity)?.is_none() {
        return Ok(());
    }
    if !entity.extra.is_empty()
        || entity
            .connections
            .as_ref()
            .is_some_and(|pairs| !pairs.is_empty())
    {
        return Err("Child occluders cannot have opaque blocks or connections".into());
    }
    let fields = [
        "classname",
        "StartActive",
        "origin",
        "angles",
        "angle",
        "spawnflags",
        "mindxlevel",
        "maxdxlevel",
        "targetname",
        "target",
        "parentname",
        "name",
        "globalname",
        "killtarget",
        "lightingorigin",
    ];
    let mut seen = 0_u16;
    for (key, value) in entity.properties.iter() {
        field(key)?;
        field(value)?;
        let index = fields
            .iter()
            .position(|field| field.eq_ignore_ascii_case(key))
            .ok_or_else(|| format!("Child occluder property {key} is unsupported"))?;
        let bit = 1 << index;
        if seen & bit != 0 {
            return Err(format!("Child occluder repeats property {key}"));
        }
        seen |= bit;
        if key != fields[index] {
            return Err(format!(
                "Child occluder requires canonical property {}",
                fields[index]
            ));
        }
        match key {
            "classname" | "origin" => {}
            "StartActive" => {
                if !matches!(value.trim(), "0" | "1") {
                    return Err("Child occluder StartActive must be 0 or 1".into());
                }
            }
            "angles" if vector(entity, "angles")? == DVec3::ZERO => {}
            "angle" if value.trim().parse::<f64>().ok() == Some(0.0) => {}
            "spawnflags" | "mindxlevel" | "maxdxlevel"
                if value.trim().parse::<u32>().ok() == Some(0) => {}
            "targetname" | "target" | "parentname" | "name" | "globalname" | "killtarget"
            | "lightingorigin" => {
                if !value.is_empty() {
                    return Err(format!("Child occluder requires exactly empty {key}"));
                }
            }
            _ => return Err(format!("Child occluder property {key} must be neutral")),
        }
    }
    if entity.properties.get("classname").is_none() {
        return Err("Child occluder requires canonical property classname".into());
    }
    check_origin(entity)
}

fn check_origin(entity: &Entity) -> Result<(), String> {
    if entity.properties.get("origin").is_some()
        && vector(entity, "origin")?.abs().max_element() > crate::map::MAX_COMPILE_COORD
    {
        return Err("Child occluder origin exceeds finite Source compile bounds".into());
    }
    Ok(())
}

pub(super) fn check_pose(entity: &Entity, control: &Control) -> Result<(), String> {
    check_origin(entity)?;
    let mut polygons = 0;
    let mut vertices = 0;
    for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
        control.checkpoint()?;
        let geometry = checked_solid_geometry_controlled(solid, control)?;
        polygons += geometry.faces.len();
        vertices += geometry
            .faces
            .iter()
            .map(|face| face.vertices.len())
            .sum::<usize>();
        if polygons > MAX_POLYGONS_PER_OCCLUDER || vertices > MAX_VERTICES_PER_OCCLUDER {
            return Err("Child occluder geometry exceeds the combined 256-polygon or 4096-point per-entity budget".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn helper() -> Entity {
        let mut entity = Entity::new(1000, "func_occluder");
        entity
            .solids
            .push(crate::test_common::room().world.solids.remove(0));
        entity
    }

    #[test]
    fn installed_state_spelling_and_unnamed_runtime_envelope_are_preserved() {
        let original = helper();
        assert!(admit(&original).is_ok());
        assert!(original.properties.get("StartActive").is_none());
        assert!(original.properties.get("origin").is_none());
        assert!(original.properties.get("angles").is_none());
        for state in ["0", "1", " 1 "] {
            let mut entity = original.clone();
            entity.properties.set("StartActive", state);
            assert!(admit(&entity).is_ok(), "{state}");
            assert!(
                entity
                    .properties
                    .iter()
                    .any(|(key, value)| key == "StartActive" && value == state)
            );
        }
        for state in ["", "01", "+1", "1.0", "2"] {
            let mut entity = original.clone();
            entity.properties.set("StartActive", state);
            assert!(admit(&entity).unwrap_err().contains("must be 0 or 1"));
        }
        let mut entity = original.clone();
        entity.properties.set("startactive", "1");
        assert!(
            admit(&entity)
                .unwrap_err()
                .contains("canonical property StartActive")
        );
        let mut entity = original.clone();
        entity.properties.0.extend([
            ("StartActive".into(), "1".into()),
            ("startactive".into(), "0".into()),
        ]);
        assert!(admit(&entity).unwrap_err().contains("repeats property"));
        for key in [
            "targetname",
            "target",
            "parentname",
            "name",
            "globalname",
            "killtarget",
            "lightingorigin",
        ] {
            let mut entity = original.clone();
            entity.properties.set(key, "");
            assert!(admit(&entity).is_ok(), "{key}");
            for value in [" ", "\t", "named"] {
                entity.properties.set(key, value);
                assert!(
                    admit(&entity)
                        .unwrap_err()
                        .contains(&format!("requires exactly empty {key}")),
                    "{key}={value:?}"
                );
            }
        }
        for (key, value) in [
            ("angles", "0 90 0"),
            ("angle", "90"),
            ("spawnflags", "1"),
            ("mindxlevel", "80"),
            ("maxdxlevel", "90"),
            ("origin", "16385 0 0"),
            ("origin", "NaN 0 0"),
            ("model", "*1"),
            ("occludernumber", "0"),
            ("hammerid", "1"),
            ("replace01", ""),
        ] {
            let mut entity = original.clone();
            entity.properties.set(key, value);
            assert!(admit(&entity).is_err(), "{key}={value}");
        }
    }
}
