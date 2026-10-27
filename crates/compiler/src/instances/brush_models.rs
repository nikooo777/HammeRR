use glam::DMat3;
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
    classname(entity).is_some()
}

pub(super) fn classname(entity: &Entity) -> Option<&'static str> {
    match entity.classname() {
        Some(class) if class.eq_ignore_ascii_case("func_brush") => Some("func_brush"),
        Some(class) if class.eq_ignore_ascii_case("func_wall") => Some("func_wall"),
        _ => None,
    }
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
    let classname = classname(entity)
        .ok_or("Child brush model requires canonical classname func_brush or func_wall")?;
    admit_fields(entity, classname, classname == "func_brush", false)?;
    if shape(entity)?.is_some() {
        crate::brush_model_pose::initial_rotation(classname, &entity.properties.0)?;
    }
    Ok(())
}

pub(crate) fn admit_stationary(entity: &Entity, rotation: bool) -> Result<(), String> {
    if entity.classname() != Some("func_brush") {
        return Err("Child brush model requires canonical classname func_brush".into());
    }
    admit_fields(entity, "func_brush", rotation, true)
}

fn admit_fields(
    entity: &Entity,
    classname: &str,
    rotation: bool,
    require_origin: bool,
) -> Result<(), String> {
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
        "vrad_brush_cast_shadows",
        "renderfx",
        "rendermode",
        "disableshadows",
        "disablereceiveshadows",
        "renderamt",
        "rendercolor",
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
    let brush_fields = [
        "Solidity",
        "StartDisabled",
        "solidbsp",
        "invert_exclusion",
        "InputFilter",
        "excludednpc",
    ];
    let class_fields = if classname == "func_brush" {
        brush_fields.as_slice()
    } else {
        &[]
    };
    let mut seen = 0_u32;
    for (key, value) in entity.properties.iter() {
        field(key)?;
        field(value)?;
        let (index, canonical) = fields
            .iter()
            .chain(class_fields)
            .enumerate()
            .find(|(_, field)| field.eq_ignore_ascii_case(key))
            .ok_or_else(|| format!("Child brush model property {key} is unsupported"))?;
        let bit = 1 << index;
        if seen & bit != 0 {
            return Err(format!("Child brush model repeats property {key}"));
        }
        seen |= bit;
        if key != *canonical {
            return Err(format!(
                "Child brush model requires canonical property {canonical}"
            ));
        }
        let neutral = match key {
            "classname" => value == classname,
            "origin" => true,
            "angles" => {
                if !rotation {
                    zero_angles(value)?;
                }
                true
            }
            "spawnflags" if classname == "func_brush" => {
                matches!(value.trim().parse::<u32>(), Ok(0 | 2))
            }
            "spawnflags" => value.trim().parse::<u32>() == Ok(0),
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
            "vrad_brush_cast_shadows" if classname == "func_brush" => {
                matches!(value.trim().parse::<u32>(), Ok(0 | 1))
            }
            "vrad_brush_cast_shadows" => value.trim().parse::<u32>() == Ok(0),
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
    if entity.properties.get("classname") != Some(classname) {
        return Err(format!(
            "Child brush model requires canonical classname {classname}"
        ));
    }
    if require_origin || entity.properties.get("origin").is_some() {
        origin(entity)?;
    }
    Ok(())
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

pub(super) fn check_source(entity: &Entity, control: &Control) -> Result<(), String> {
    for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
        control.checkpoint()?;
        checked_solid_controlled(solid, control)?;
    }
    Ok(())
}

pub(super) fn plan_origin(
    materials: &mut Materials<'_>,
    entity: &Entity,
    control: &Control,
    brush_texture_shadows: bool,
    sources: &BuildMaterialSources,
    checked_cutouts: &mut std::collections::BTreeSet<usize>,
) -> Result<crate::map::origins::OriginPlan, String> {
    let mut report = Report::default();
    let plan = crate::map::origins::prepare_origins(materials, entity, &mut report, control)?;
    if let Some(error) = report
        .diagnostics
        .iter()
        .find(|error| error.level == crate::Level::Error)
    {
        return Err(format!(
            "{}: {}",
            error.object.as_deref().unwrap_or("entity"),
            error.message
        ));
    }
    if plan.position.is_none() {
        return Err(
            "Child brush model requires an explicit origin or one visible resolved origin brush"
                .into(),
        );
    }
    let mut remaining = 0;
    for solid in entity
        .solids
        .iter()
        .filter(|solid| !solid.hidden && !plan.volumes.contains(&solid.id))
    {
        remaining += 1;
        validate_solid_materials(
            materials,
            solid,
            entity,
            control,
            brush_texture_shadows,
            sources,
            checked_cutouts,
        )
        .map_err(|error| format!("Child brush-model material: solid {}: {error}", solid.id))?;
    }
    if remaining == 0 {
        return Err(
            "An origin brush requires compiled brush geometry in the same runtime entity".into(),
        );
    }
    Ok(plan)
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
                "Child brush model solid {} local coordinates exceed finite Source compile bounds; move its effective origin closer to the brushes",
                solid.id,
            ));
        }
    }
    Ok(())
}

pub(super) fn lower_initial_pose(
    entity: &mut Entity,
    rotation: DMat3,
    control: &Control,
) -> Result<(), String> {
    let pivot = origin(entity)?;
    let offset = crate::brush_model_pose::initial_offset(&entity.properties.0)?;
    let pose = crate::brush_model_pose::Pose {
        offset,
        rotation: Some(rotation),
    };
    let translation = offset - rotation * pivot;
    for solid in entity.solids.iter_mut().filter(|solid| !solid.hidden) {
        control.checkpoint()?;
        let geometry = checked_solid_geometry_controlled(solid, control)?;
        pose.raw_and_stored_bounds(geometry.mins - pivot, geometry.maxs - pivot)?;
        for side in &mut solid.sides {
            control.checkpoint()?;
            for point in &mut side.plane.0 {
                *point = pose.point(*point - pivot);
            }
            for axis in [&mut side.u_axis, &mut side.v_axis] {
                axis.axis = rotation * axis.axis;
                axis.offset -= translation.dot(axis.axis) / axis.scale;
            }
        }
    }
    entity
        .properties
        .set("origin", source_formats::vmf::format_vec3(offset));
    entity.properties.set("angles", "0 0 0");
    check_pose(entity, control)
}

pub(crate) fn ordinary(info: &MaterialInfo) -> bool {
    info.found
        && info.contents == contents::SOLID
        && info.surface & !(surf::NODRAW | surf::NOLIGHT | surf::BUMPLIGHT) == 0
        && !info.hint
        && !info.skip
        && info.unsupported_flags.is_empty()
}

fn validate_solid_materials(
    materials: &mut Materials<'_>,
    solid: &Solid,
    entity: &Entity,
    control: &Control,
    brush_texture_shadows: bool,
    sources: &BuildMaterialSources,
    checked_cutouts: &mut std::collections::BTreeSet<usize>,
) -> Result<(), String> {
    let brush_texture_shadows = brush_texture_shadows && entity.classname() == Some("func_brush");
    for side in &solid.sides {
        control.checkpoint()?;
        let index = materials.load(&side.material)?;
        let info = materials.get(index);
        if !ordinary(info) {
            if brush_texture_shadows {
                crate::brush_shadow_casters::request(entity)?;
            }
            if brush_texture_shadows && crate::brush_shadow_casters::binary_cutout(info) {
                if checked_cutouts.insert(index) {
                    crate::brush_shadow_casters::validate_material(
                        &side.material.trim().replace('\\', "/").to_ascii_lowercase(),
                        sources,
                        control,
                        true,
                    )?;
                }
                continue;
            }
            return Err(format!(
                "{} requires ordinary SOLID contents without origin, liquid or special surface semantics",
                side.material,
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_materials<'a>(
    entities: impl Iterator<Item = &'a Entity>,
    fs: &GameFs,
    sources: &BuildMaterialSources,
    control: &Control,
    brush_texture_shadows: bool,
) -> Result<(), Diagnostic> {
    let mut materials = Materials::with_build_sources(fs, Requirements::GeometrySemantics, sources);
    let mut checked_cutouts = std::collections::BTreeSet::new();
    for entity in entities {
        for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
            let result = validate_solid_materials(
                &mut materials,
                solid,
                entity,
                control,
                brush_texture_shadows,
                sources,
                &mut checked_cutouts,
            );
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
    fn stationary_global_request_shares_neutral_fields_and_admitted_initial_angles() {
        let mut entity = model();
        entity.properties.set("vrad_brush_cast_shadows", "1");
        let original = entity.clone();
        admit(&entity).unwrap();
        assert_eq!(entity, original);
        entity.properties.set("angles", "15 90 -30");
        admit(&entity).unwrap();
        assert!(admit_stationary(&entity, false).is_err());
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
    fn walls_share_only_neutral_fields_and_keep_zero_source_angles_and_no_global_request() {
        let mut entity = model();
        entity.properties.set("classname", "func_wall");
        let original = entity.clone();
        admit(&entity).unwrap();
        assert_eq!(entity, original);
        for (key, value) in [
            ("spawnflags", "0"),
            ("vrad_brush_cast_shadows", "0"),
            ("renderfx", "0"),
            ("rendermode", "0"),
            ("disableshadows", "0"),
            ("disablereceiveshadows", "0"),
            ("renderamt", "255"),
            ("rendercolor", "255 255 255"),
            ("mindxlevel", "0"),
            ("maxdxlevel", "0"),
            ("targetname", ""),
            ("target", ""),
            ("parentname", ""),
            ("name", ""),
            ("globalname", ""),
            ("killtarget", ""),
            ("lightingorigin", ""),
            ("angles", "-0 +0.0 0e-300"),
        ] {
            entity.properties.set(key, value);
        }
        let original = entity.clone();
        admit(&entity).unwrap();
        assert_eq!(entity, original);
        for rotation in [false, true] {
            assert!(
                admit_stationary(&entity, rotation)
                    .unwrap_err()
                    .contains("canonical classname func_brush")
            );
        }
        for (key, value) in [
            ("classname", "FUNC_WALL"),
            ("angles", "0 1 0"),
            ("angles", "1e-4000 0 0"),
            ("angles", "0 0"),
            ("angle", "0"),
            ("Angles", "0 0 0"),
            ("spawnflags", "2"),
            ("vrad_brush_cast_shadows", "1"),
            ("Solidity", "0"),
            ("StartDisabled", "0"),
            ("solidbsp", "0"),
            ("invert_exclusion", "0"),
            ("InputFilter", "0"),
            ("excludednpc", ""),
            ("_minlight", "0"),
            ("renderfx", "1"),
            ("rendermode", "1"),
            ("disableshadows", "1"),
            ("disablereceiveshadows", "1"),
            ("renderamt", "254"),
            ("rendercolor", "255 255 0"),
            ("mindxlevel", "1"),
            ("maxdxlevel", "1"),
            ("targetname", " "),
            ("parentname", "parent"),
            ("origin", "NaN 0 0"),
            ("origin", "16385 0 0"),
        ] {
            let mut invalid = entity.clone();
            if key == "Angles" {
                invalid.properties.remove("angles");
                invalid.properties.0.push((key.into(), value.into()));
            } else {
                invalid.properties.set(key, value);
            }
            assert!(admit(&invalid).is_err(), "{key}={value}");
        }
        entity.properties.0.push(("angles".into(), "0 0 0".into()));
        assert!(admit(&entity).unwrap_err().contains("repeats property"));
        entity = original;
        entity.connections = Some(vec![source_formats::vmf::Connection {
            output: "OnUser1".into(),
            value: "other,Kill,,0,-1".into(),
        }]);
        assert!(admit(&entity).unwrap_err().contains("connections"));
    }

    #[test]
    fn source_angles_and_explicit_pivots_cannot_be_manufactured_by_parsing() {
        for angles in [
            "360.0000001 0 0",
            "0 -361 0",
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
        admit(&entity).unwrap();
        assert!(
            admit_stationary(&entity, true)
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
    fn initial_pose_preserves_nonunit_texture_rows_and_uses_the_native_pivot() {
        let mut entity = model();
        let pivot = DVec3::new(0.1, -0.2, 0.3);
        entity.properties.set("origin", "0.1 -0.2 0.3");
        entity.properties.set("angles", "20 35 10");
        for side in &mut entity.solids[0].sides {
            side.u_axis.axis = DVec3::new(2.0, 3.0, 5.0);
            side.v_axis.axis = DVec3::new(-1.0, 4.0, 2.0);
            side.u_axis.scale = 0.5;
            side.v_axis.scale = 2.0;
            side.u_axis.offset = 7.0;
            side.v_axis.offset = -11.0;
        }
        let before = entity.clone();
        let native_pivot = DVec3::new(
            0.10000000149011612,
            -0.20000000298023224,
            0.30000001192092896,
        );
        let expected_rotation = DMat3::from_cols_array(&[
            0.7697511313200572,
            0.5389855446957562,
            -0.3420201433256687,
            -0.5162121193659353,
            0.8407726623973335,
            0.16317591116653482,
            0.37551064385876115,
            0.050950100827363065,
            0.9254165783983234,
        ]);
        let rotation =
            crate::brush_model_pose::initial_rotation("func_brush", &entity.properties.0)
                .unwrap()
                .unwrap();
        lower_initial_pose(&mut entity, rotation, &Control::default()).unwrap();
        assert_eq!(entity.origin(), Some(native_pivot));
        assert_eq!(entity.properties.get("angles"), Some("0 0 0"));
        for (actual, original) in entity.solids[0].sides.iter().zip(&before.solids[0].sides) {
            assert_eq!(actual.id, original.id);
            assert_eq!(actual.material, original.material);
            assert_eq!(actual.lightmap_scale, original.lightmap_scale);
            for (point, old) in actual.plane.0.iter().zip(original.plane.0) {
                let expected = expected_rotation * (old - pivot) + native_pivot;
                assert!(point.abs_diff_eq(expected, 1e-12));
                for (axis, source) in [
                    (actual.u_axis, original.u_axis),
                    (actual.v_axis, original.v_axis),
                ] {
                    assert_eq!(axis.scale, source.scale);
                    assert!(
                        axis.axis
                            .abs_diff_eq(expected_rotation * source.axis, 1e-12)
                    );
                    let diffuse = expected.dot(axis.axis) / axis.scale + axis.offset;
                    let source_diffuse = old.dot(source.axis) / source.scale + source.offset;
                    assert!((diffuse - source_diffuse).abs() < 1e-10);
                    let lightmap = (expected.dot(axis.axis) + axis.offset * axis.scale)
                        / f64::from(actual.lightmap_scale);
                    let source_lightmap = (old.dot(source.axis) + source.offset * source.scale)
                        / f64::from(original.lightmap_scale);
                    assert!((lightmap - source_lightmap).abs() < 1e-11);
                }
            }
        }
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
                false,
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
            false,
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
                &control,
                false,
            )
            .unwrap_err()
            .message
            .contains("cancelled")
        );
    }
}
