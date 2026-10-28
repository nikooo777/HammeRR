use std::{collections::BTreeSet, io};

use source_formats::{
    bsp::{contents, surf},
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
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
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
) -> Result<vmt::Material, String> {
    validate_material_with_reader(
        name,
        &mut |path| {
            control.checkpoint().map_err(io::Error::other)?;
            let asset = sources
                .retained(path)?
                .ok_or_else(|| io::Error::other("original material dependency was not retained"))?;
            Ok(Some(asset.bytes().to_vec()))
        },
        cutout,
    )
}

pub(crate) fn validate_material_with_reader(
    name: &str,
    read: &mut dyn FnMut(&str) -> io::Result<Option<Vec<u8>>>,
    cutout: bool,
) -> Result<vmt::Material, String> {
    let material = vmt::Material::load_with_reader(&vmt::material_path(name), &mut |path| {
        let Some(bytes) = read(path)? else {
            return Ok(None);
        };
        opaque_raw(path, &bytes)?;
        Ok(Some(bytes))
    })
    .map_err(|error| error.to_string())?
    .ok_or("material is unavailable")?;
    if cutout {
        crate::lighting::validate_brush_cutout_material(&material)
    } else {
        opaque_parameters(name, |key| material.get(key))
    }?;
    Ok(material)
}

pub(crate) fn texture_alpha(info: &MaterialInfo) -> bool {
    info.found
        && info.draws
        && info.contents == (contents::WINDOW | contents::TRANSLUCENT)
        && info.surface & !surf::TRANS == 0
        && !info.hint
        && !info.skip
        && info.unsupported_flags.is_empty()
}

pub(crate) fn opaque_parameters<'a>(
    name: &str,
    get: impl Fn(&str) -> Option<&'a str>,
) -> Result<(), String> {
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
        if let Some(value) = get(key) {
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
                    let cutout = brush_texture_shadows && texture_alpha(info);
                    if !instances::brush_models::ordinary(info) && !cutout {
                        return Err(format!(
                            "{} requires ordinary SOLID contents, or supported alpha-tested or translucent LightmappedGeneric with brush texture shadows enabled, without origin, liquid or special surface semantics",
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
