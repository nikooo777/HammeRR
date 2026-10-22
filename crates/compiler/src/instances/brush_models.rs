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
    entity
        .classname()
        .is_some_and(|class| class.eq_ignore_ascii_case("func_brush"))
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
    admit_stationary(entity, false)
}

pub(crate) fn admit_stationary(entity: &Entity, rotation: bool) -> Result<(), String> {
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
        "Solidity",
        "StartDisabled",
        "solidbsp",
        "invert_exclusion",
        "vrad_brush_cast_shadows",
        "InputFilter",
        "renderfx",
        "rendermode",
        "disableshadows",
        "disablereceiveshadows",
        "renderamt",
        "rendercolor",
        "excludednpc",
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
    let mut seen = 0_u32;
    for (key, value) in entity.properties.iter() {
        field(key)?;
        field(value)?;
        let index = fields
            .iter()
            .position(|field| field.eq_ignore_ascii_case(key))
            .ok_or_else(|| format!("Child brush model property {key} is unsupported"))?;
        let bit = 1 << index;
        if seen & bit != 0 {
            return Err(format!("Child brush model repeats property {key}"));
        }
        seen |= bit;
        if key != fields[index] {
            return Err(format!(
                "Child brush model requires canonical property {}",
                fields[index]
            ));
        }
        let neutral = match key {
            "classname" => value == "func_brush",
            "origin" => true,
            "angles" => {
                if !rotation {
                    zero_angles(value)?;
                }
                true
            }
            "spawnflags" => matches!(value.trim().parse::<u32>(), Ok(0 | 2)),
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
            "vrad_brush_cast_shadows" => matches!(value.trim().parse::<u32>(), Ok(0 | 1)),
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
    if entity.properties.get("classname") != Some("func_brush") {
        return Err("Child brush model requires canonical classname func_brush".into());
    }
    origin(entity).map(|_| ())
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
                "Child brush model solid {} local coordinates exceed finite Source compile bounds; move its explicit origin closer to the brushes",
                solid.id,
            ));
        }
    }
    Ok(())
}

pub(crate) fn ordinary(info: &MaterialInfo) -> bool {
    info.found
        && info.contents == contents::SOLID
        && info.surface & !(surf::NODRAW | surf::NOLIGHT | surf::BUMPLIGHT) == 0
        && !info.hint
        && !info.skip
        && info.unsupported_flags.is_empty()
}

pub(super) fn validate_materials<'a>(
    entities: impl Iterator<Item = &'a Entity>,
    fs: &GameFs,
    sources: &BuildMaterialSources,
    control: &Control,
) -> Result<(), Diagnostic> {
    let mut materials = Materials::with_build_sources(fs, Requirements::GeometrySemantics, sources);
    for entity in entities {
        for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
            let result: Result<(), String> = (|| {
                for side in &solid.sides {
                    control.checkpoint()?;
                    let index = materials.load(&side.material)?;
                    if !ordinary(materials.get(index)) {
                        return Err(format!(
                            "{} requires ordinary SOLID contents without origin, liquid or special surface semantics",
                            side.material,
                        ));
                    }
                }
                Ok(())
            })();
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

#[cfg(test)]
mod tests {
    use source_formats::gamefs::{Mount, MountedPath};

    use super::*;

    fn model() -> Entity {
        let mut entity = Entity::new(1000, "func_brush");
        entity.properties.set("origin", "0 0 0");
        entity
            .solids
            .push(crate::test_common::room().world.solids.remove(0));
        entity
    }

    #[test]
    fn native_absence_and_installed_neutral_defaults_preserve_authored_values() {
        let mut entity = model();
        let before = entity.clone();
        admit(&entity).unwrap();
        assert_eq!(entity, before);
        for (key, value) in [
            ("Solidity", "0"),
            ("StartDisabled", "0"),
            ("InputFilter", "0"),
            ("solidbsp", "0"),
            ("invert_exclusion", "0"),
            ("vrad_brush_cast_shadows", "0"),
            ("renderfx", "0"),
            ("rendermode", "0"),
            ("disableshadows", "0"),
            ("disablereceiveshadows", "0"),
            ("renderamt", "255"),
            ("rendercolor", "255 255 255"),
            ("globalname", ""),
            ("excludednpc", ""),
            ("angles", "-0 +0.0 0e-300"),
        ] {
            entity.properties.set(key, value);
        }
        for flags in ["0", "2"] {
            entity.properties.set("spawnflags", flags);
            let before = entity.clone();
            admit(&entity).unwrap();
            assert_eq!(entity, before);
        }
        for (key, value) in [
            ("spawnflags", "1"),
            ("Solidity", "2"),
            ("StartDisabled", "1"),
            ("InputFilter", "8"),
            ("renderamt", "254"),
            ("rendercolor", "255 255 0"),
            ("vrad_brush_cast_shadows", "2"),
            ("parentname", "parent"),
            ("targetname", " "),
            ("excludednpc", "npc_test"),
            ("_minlight", "0"),
            ("angle", "0"),
            ("angles#later", "0 0 0"),
            ("origin ", "0 0 0"),
        ] {
            let mut changed = entity.clone();
            changed.properties.set(key, value);
            assert!(admit(&changed).is_err(), "{key}={value}");
        }
        let mut alias = model();
        alias.properties.set("solidity", "0");
        assert!(
            admit(&alias)
                .unwrap_err()
                .contains("canonical property Solidity")
        );
        entity.properties.0.push(("solidity".into(), "0".into()));
        assert!(admit(&entity).unwrap_err().contains("repeats property"));
    }

    #[test]
    fn stationary_global_request_shares_neutral_fields_but_keeps_child_source_zero() {
        let mut entity = model();
        entity.properties.set("vrad_brush_cast_shadows", "1");
        let original = entity.clone();
        admit(&entity).unwrap();
        assert_eq!(entity, original);
        entity.properties.set("angles", "15 90 -30");
        assert!(admit(&entity).is_err());
        admit_stationary(&entity, true).unwrap();
        for (key, value) in [
            ("targetname", "moving"),
            ("parentname", "parent"),
            ("StartDisabled", "1"),
            ("Solidity", "1"),
        ] {
            let mut invalid = entity.clone();
            invalid.properties.set(key, value);
            assert!(admit_stationary(&invalid, true).is_err(), "{key}");
        }
    }

    #[test]
    fn source_zero_angles_and_explicit_pivots_cannot_be_manufactured_by_parsing() {
        for angles in [
            "0 90 0",
            "360 0 0",
            "1e-4000 0 0",
            "1e-50 0 0",
            "NaN 0 0",
            "0 0",
            "0 0 0 trailing",
            "(0 0 0)",
        ] {
            let mut entity = model();
            entity.properties.set("angles", angles);
            assert!(admit(&entity).is_err(), "{angles}");
        }
        let mut entity = model();
        entity.properties.remove("origin");
        assert!(
            admit(&entity)
                .unwrap_err()
                .contains("explicit canonical origin")
        );
        for origin in ["NaN 0 0", "1 2", "16385 0 0"] {
            entity.properties.set("origin", origin);
            assert!(admit(&entity).is_err(), "{origin}");
        }
        entity.properties.set("origin", "0 0 0");
        check_pose(&entity, &Control::default()).unwrap();
        let bounds = BrushGeometry::from_solid(&entity.solids[0]).unwrap();
        let pivot = if bounds.mins.x < 0.0 {
            16_384.0
        } else {
            -16_384.0
        };
        entity.properties.set("origin", format!("{pivot} 0 0"));
        admit(&entity).unwrap();
        assert!(
            check_pose(&entity, &Control::default())
                .unwrap_err()
                .contains("local coordinates")
        );
    }

    #[test]
    fn hidden_brushes_are_omitted_and_a_truly_empty_model_is_not_admitted() {
        let mut entity = model();
        entity.solids[0].hidden = true;
        entity.properties.remove("origin");
        entity.solids[0].sides[0].plane.0 = [DVec3::ZERO; 3];
        assert!(shape(&entity).unwrap().is_none());
        admit(&entity).unwrap();
        entity.solids.clear();
        assert!(admit(&entity).unwrap_err().contains("visible ordinary"));
    }

    #[test]
    fn material_gate_uses_resolved_ordinary_semantics_without_texture_metrics() {
        let directory = tempfile::tempdir().unwrap();
        let materials = directory.path().join("materials");
        std::fs::create_dir(&materials).unwrap();
        let mut game = crate::test_common::empty_game();
        game.mounts.push(MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.path().to_owned()),
        });
        for (index, (shader, parameters, accepted)) in [
            ("LightmappedGeneric", "$basetexture missing/image", true),
            ("LightmappedGeneric", "%compilenodraw 1", true),
            ("LightmappedGeneric", "$bumpmap missing/normal", true),
            ("UnlitGeneric", "", true),
            ("LightmappedGeneric", "%compileorigin 1", false),
            ("LightmappedGeneric", "%compilewater 1", false),
            ("LightmappedGeneric", "%compiletrigger 1", false),
            ("LightmappedGeneric", "%compilesky 1", false),
            ("LightmappedGeneric", "%compileskip 1", false),
            ("LightmappedGeneric", "%compiledetail 1", false),
            ("LightmappedGeneric", "$alphatest 1", false),
            ("LightmappedGeneric", "%compileunknown 1", false),
        ]
        .into_iter()
        .enumerate()
        {
            let name = format!("case{index}");
            std::fs::write(
                materials.join(format!("{name}.vmt")),
                format!("{shader} {{ {parameters} }}"),
            )
            .unwrap();
            let patch = format!("patch{index}");
            std::fs::write(
                materials.join(format!("{patch}.vmt")),
                format!("Patch {{ include materials/{name}.vmt }}"),
            )
            .unwrap();
            let mut entity = model();
            for side in &mut entity.solids[0].sides {
                side.material.clone_from(&patch);
            }
            let sources = BuildMaterialSources::default();
            let result = validate_materials(
                std::iter::once(&entity),
                &game,
                &sources,
                &Control::default(),
            );
            assert_eq!(
                result.is_ok(),
                accepted,
                "{shader} {parameters}: {result:?}"
            );
        }
        let mut entity = model();
        for side in &mut entity.solids[0].sides {
            side.material = "missing/material".into();
        }
        let error = validate_materials(
            std::iter::once(&entity),
            &game,
            &BuildMaterialSources::default(),
            &Control::default(),
        )
        .unwrap_err();
        assert!(
            error
                .message
                .contains("geometry semantics cannot be determined")
        );
        assert_eq!(error.object, Some(format!("solid:{}", entity.solids[0].id)));
        let control = Control::default();
        control.cancel().unwrap();
        assert!(
            validate_materials(
                std::iter::once(&entity),
                &game,
                &BuildMaterialSources::default(),
                &control
            )
            .unwrap_err()
            .message
            .contains("cancelled")
        );
    }
}
