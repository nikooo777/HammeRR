use std::{collections::BTreeSet, io};

use source_formats::{
    bsp::contents,
    gamefs::GameFs,
    keyvalues,
    vmf::{Entity, Vmf},
    vmt,
};

use crate::{
    Control, Diagnostic,
    asset_sources::BuildMaterialSources,
    brush_model_pose::Pose,
    instances,
    materials::{MaterialInfo, Materials, Requirements},
};

const KEY: &str = "vrad_brush_cast_shadows";

pub(crate) fn enabled(properties: &[(String, String)]) -> bool {
    properties
        .iter()
        .any(|(key, value)| key == KEY && value.trim() == "1")
}

pub(crate) fn request(entity: &Entity) -> Result<bool, String> {
    let mut fields = entity.properties.iter().filter(|(key, _)| {
        key.trim_end_matches(' ')
            .split('#')
            .next()
            .unwrap_or(key)
            .eq_ignore_ascii_case(KEY)
    });
    let Some((key, value)) = fields.next() else {
        return Ok(false);
    };
    if fields.next().is_some() || key != KEY {
        return Err(
            "requires one canonical vrad_brush_cast_shadows field with literal 0 or 1".into(),
        );
    }
    if value.trim().parse::<u32>() == Ok(0) {
        return Ok(false);
    }
    if value.trim() != "1" {
        return Err(
            "requires one canonical vrad_brush_cast_shadows field with literal 0 or 1".into(),
        );
    }
    Ok(true)
}

fn diagnostic(entity: &Entity, message: String) -> Diagnostic {
    Diagnostic::error(
        format!("Brush global shadows: {message}"),
        Some(format!("entity:{}", entity.id)),
        entity.origin(),
    )
}

pub(crate) fn requests<'a>(vmf: &'a Vmf, control: &Control) -> Result<Vec<&'a Entity>, Diagnostic> {
    let mut requests = Vec::new();
    let mut objects = 0usize;
    let mut sides = 0usize;
    let mut work = 0usize;
    for entity in vmf.entities.iter().filter(|entity| !entity.hidden) {
        if !request(entity).map_err(|error| diagnostic(entity, error))? {
            continue;
        }
        let checked = (|| {
            control.checkpoint()?;
            let shape = instances::brush_models::shape(entity)?
                .ok_or("requested model requires visible ordinary brushes")?;
            objects = objects
                .checked_add(shape.brushes + 1)
                .filter(|count| *count <= instances::MAX_OBJECTS)
                .ok_or("requested models exceed the aggregate object limit")?;
            sides = sides
                .checked_add(shape.sides)
                .filter(|count| *count <= instances::MAX_SIDES)
                .ok_or("requested models exceed the aggregate side limit")?;
            work = work
                .checked_add(shape.geometry_work)
                .filter(|count| *count <= instances::MAX_GEOMETRY_WORK)
                .ok_or("requested models exceed the aggregate geometry-work limit")?;
            instances::brush_models::admit_stationary(entity, true)
        })();
        checked.map_err(|error| diagnostic(entity, error))?;
        requests.push(entity);
    }
    Ok(requests)
}

fn geometry(entity: &Entity, control: &Control) -> Result<(), String> {
    let pivot = instances::brush_models::origin(entity)?;
    let properties = &entity.properties.0;
    let rotation = crate::brush_model_pose::initial_rotation("func_brush", properties)?;
    let pose = Pose {
        offset: if rotation.is_some() {
            crate::brush_model_pose::initial_offset(properties)?
        } else {
            pivot
        },
        rotation,
    };
    for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
        control.checkpoint()?;
        let geometry = instances::checked_solid_geometry_controlled(solid, control)?;
        let mins = geometry.mins - pivot;
        let maxs = geometry.maxs - pivot;
        if mins.abs().max(maxs.abs()).max_element() > crate::map::MAX_COMPILE_COORD {
            return Err(format!(
                "solid:{} local coordinates exceed finite Source compile bounds",
                solid.id
            ));
        }
        pose.raw_and_stored_bounds(mins, maxs)?;
    }
    Ok(())
}

fn opaque_raw(path: &str, bytes: &[u8]) -> io::Result<()> {
    let invalid =
        |message: &str| io::Error::new(io::ErrorKind::InvalidData, format!("{path}: {message}"));
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("material must be UTF-8"))?;
    let nodes =
        keyvalues::parse_static_material_bounded(text, 3, 4096).map_err(io::Error::other)?;
    let [root] = nodes.as_slice() else {
        return Err(invalid("requires one material root"));
    };
    let children = root
        .children()
        .ok_or_else(|| invalid("requires a shader block"))?;
    let patch = root.key.eq_ignore_ascii_case("patch");
    if !patch
        && !["lightmappedgeneric", "unlitgeneric"]
            .iter()
            .any(|shader| root.key.eq_ignore_ascii_case(shader))
    {
        return Err(invalid(
            "requires LightmappedGeneric, UnlitGeneric or Patch",
        ));
    }
    fn scalars(nodes: &[keyvalues::Node]) -> bool {
        let mut seen = BTreeSet::new();
        nodes
            .iter()
            .all(|node| node.as_str().is_some() && seen.insert(node.key.to_ascii_lowercase()))
    }
    if patch {
        let mut seen = BTreeSet::new();
        for node in children {
            let key = node.key.to_ascii_lowercase();
            if !seen.insert(key.clone())
                || match key.as_str() {
                    "include" => node.as_str().is_none(),
                    "insert" | "replace" => node.children().is_none_or(|nodes| !scalars(nodes)),
                    _ => true,
                }
            {
                return Err(invalid(
                    "requires an unambiguous scalar Patch include/edit chain",
                ));
            }
        }
    } else if !scalars(children) {
        return Err(invalid(
            "conditional/animated blocks and duplicate material fields are unsupported",
        ));
    }
    let parsed = vmt::Material::parse(text).map_err(io::Error::other)?;
    let mut raw_params = children
        .iter()
        .filter_map(|node| node.as_str().map(|value| (&node.key, value)));
    let params_agree = parsed.params.iter().all(|(key, value)| {
        raw_params.next().is_some_and(|(raw_key, raw_value)| {
            raw_key.eq_ignore_ascii_case(key) && raw_value == value.as_str()
        })
    }) && raw_params.next().is_none();
    if parsed.shader != root.key.to_ascii_lowercase()
        || !params_agree
        || !children
            .iter()
            .filter(|node| node.children().is_some())
            .eq(parsed.blocks.iter())
    {
        return Err(invalid("raw and resolved material parsers disagree"));
    }
    Ok(())
}

pub(crate) fn validate_material(
    name: &str,
    sources: &BuildMaterialSources,
    control: &Control,
    cutout: bool,
) -> Result<(), String> {
    let material = vmt::Material::load_with_reader(&vmt::material_path(name), &mut |path| {
        control.checkpoint().map_err(io::Error::other)?;
        let asset = sources
            .retained(path)?
            .ok_or_else(|| io::Error::other("original material dependency was not retained"))?;
        opaque_raw(path, asset.bytes())?;
        Ok(Some(asset.bytes().to_vec()))
    })
    .map_err(|error| error.to_string())?
    .ok_or("material is unavailable")?;
    if cutout {
        crate::lighting::validate_brush_cutout_material(&material)
    } else {
        opaque_parameters(name, &material)
    }
}

pub(crate) fn binary_cutout(info: &MaterialInfo) -> bool {
    info.found
        && info.draws
        && info.contents == (contents::WINDOW | contents::TRANSLUCENT)
        && info.surface == 0
        && !info.hint
        && !info.skip
        && info.unsupported_flags.is_empty()
}

fn opaque_parameters(name: &str, material: &vmt::Material) -> Result<(), String> {
    for (key, neutral) in [
        ("$alphatest", 0.0),
        ("$translucent", 0.0),
        ("$additive", 0.0),
        ("$vertexalpha", 0.0),
        ("$vertexalphatest", 0.0),
        ("$allowalphatocoverage", 0.0),
        ("$distancealpha", 0.0),
        ("$distancealphafromdetail", 0.0),
        ("$glow", 0.0),
        ("$wireframe", 0.0),
        ("$decal", 0.0),
        ("$nodraw", 0.0),
        ("$depthblend", 0.0),
        ("$softedges", 0.0),
        ("$outline", 0.0),
        ("$ignorez", 0.0),
        ("$znearer", 0.0),
        ("$alpha", 1.0),
    ] {
        if let Some(value) = material.get(key) {
            let value = value
                .trim()
                .parse::<f64>()
                .map_err(|_| format!("cannot classify {key}"))?;
            if !value.is_finite() || value != neutral {
                return Err(format!(
                    "{name} requires opaque material; {key} is unsupported"
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate(
    requests: &[&Entity],
    fs: &GameFs,
    sources: &BuildMaterialSources,
    control: &Control,
    brush_texture_shadows: bool,
) -> Result<(), Diagnostic> {
    let mut materials = Materials::with_build_sources(fs, Requirements::GeometrySemantics, sources);
    let mut checked = BTreeSet::new();
    for &entity in requests {
        geometry(entity, control).map_err(|error| diagnostic(entity, error))?;
        for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
            for side in &solid.sides {
                let result = (|| {
                    control.checkpoint()?;
                    let index = materials.load(&side.material)?;
                    let info = materials.get(index);
                    let cutout = brush_texture_shadows && binary_cutout(info);
                    if !instances::brush_models::ordinary(info) && !cutout {
                        return Err(format!(
                            "{} requires ordinary SOLID contents, or a supported binary cutout with brush texture shadows enabled, without origin, liquid or special surface semantics",
                            side.material
                        ));
                    }
                    if checked.insert(index) {
                        validate_material(
                            &side.material.trim().replace('\\', "/").to_ascii_lowercase(),
                            sources,
                            control,
                            cutout,
                        )?;
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    return Err(Diagnostic::error(
                        format!("Brush global shadows: {error}"),
                        Some(format!("solid:{}", solid.id)),
                        entity.origin(),
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_preserve_zero_and_refuse_aliases_ambiguity_and_nonbinary_values() {
        let mut entity = Entity::new(7, "func_brush");
        assert!(!request(&entity).unwrap());
        entity.properties.set(KEY, "0");
        assert!(!request(&entity).unwrap());
        entity.properties.set(KEY, "1");
        assert!(request(&entity).unwrap());
        for invalid in ["2", "1.0", "", "nan"] {
            entity.properties.set(KEY, invalid);
            assert!(request(&entity).is_err());
        }
        entity.properties.set(KEY, "1");
        entity
            .properties
            .0
            .push(("Vrad_brush_cast_shadows".into(), "0".into()));
        assert!(request(&entity).is_err());
    }

    #[test]
    fn requested_source_requires_explicit_canonical_class_even_without_receivers() {
        let mut vmf = crate::test_common::room();
        vmf.entities.clear();
        let mut entity = Entity::new(700, "func_brush");
        entity.properties.set(KEY, "1");
        entity.properties.set("origin", "0 0 0");
        entity.solids.push(vmf.world.solids[0].clone());
        entity.properties.remove("classname");
        vmf.entities.push(entity);
        assert!(
            requests(&vmf, &Control::default())
                .unwrap_err()
                .message
                .contains("classname")
        );
        vmf.entities[0].properties.set("classname", "func_wall");
        assert!(requests(&vmf, &Control::default()).is_err());
        vmf.entities[0].properties.set("classname", "func_brush");
        assert_eq!(requests(&vmf, &Control::default()).unwrap().len(), 1);
    }

    #[test]
    fn effective_material_flags_refuse_partial_coverage_even_on_compile_nodraw() {
        for key in [
            "$alphatest",
            "$translucent",
            "$additive",
            "$vertexalphatest",
            "$allowalphatocoverage",
            "$wireframe",
            "$decal",
            "$nodraw",
            "$depthblend",
            "$softedges",
            "$outline",
            "$ignorez",
            "$znearer",
        ] {
            let material = vmt::Material::parse(&format!(
                "LightmappedGeneric {{ %compilenodraw 1 {key} 1 }}"
            ))
            .unwrap();
            assert!(
                opaque_parameters("test/hidden", &material).is_err(),
                "{key}"
            );
        }
        let material =
            vmt::Material::parse("UnlitGeneric { %compilenodraw 1 $alpha 1.0 $alphatest 0.0 }")
                .unwrap();
        opaque_parameters("test/hidden", &material).unwrap();
    }

    #[test]
    fn aggregate_source_work_refuses_before_hull_evaluation_and_observes_cancellation() {
        let mut vmf = crate::test_common::room();
        vmf.entities.clear();
        let mut entity = Entity::new(700, "func_brush");
        entity.properties.set(KEY, "1");
        entity.properties.set("origin", "0 0 0");
        let mut solid = vmf.world.solids[0].clone();
        solid
            .sides
            .resize(instances::MAX_BRUSH_SIDES, solid.sides[0].clone());
        entity.solids.push(solid);
        let count = instances::MAX_GEOMETRY_WORK / instances::MAX_BRUSH_SIDES.pow(3) + 1;
        vmf.entities = vec![entity; count];
        let control = Control::default();
        let failure = requests(&vmf, &control).unwrap_err();
        assert!(failure.message.contains("aggregate geometry-work"));
        control.cancel().unwrap();
        assert!(
            requests(&vmf, &control)
                .unwrap_err()
                .message
                .contains("cancelled")
        );
    }

    #[test]
    fn opaque_raw_proof_retains_scalar_patches_but_refuses_hidden_and_ambiguous_syntax() {
        for raw in [
            r#"LightmappedGeneric { "$basetexture" "x" }"#,
            r#"UnlitGeneric { "%compilenodraw" "1" }"#,
            r#"Patch { include "materials/x.vmt" replace { "$basetexture" "y" } }"#,
        ] {
            opaque_raw("materials/x.vmt", raw.as_bytes()).unwrap();
        }
        for raw in [
            r#"LightmappedGeneric { "$alpha" "0" "$ALPHA" "1" }"#,
            r#"LightmappedGeneric { Proxies { } }"#,
            r#"LightmappedGeneric { "$alpha" "0" [$WIN32] }"#,
            r#"Refract { }"#,
        ] {
            assert!(opaque_raw("materials/x.vmt", raw.as_bytes()).is_err());
        }
    }
}
