use source_formats::{
    bsp::MAX_PHYSICS_KEY_BYTES,
    keyvalues::{self, Node},
};

use super::{MAX_PHYSICS_SOLIDS, MAX_SURFACE_PROPERTIES, Model};
use crate::Control;

pub(super) const FLUID_NORMAL: [f32; 3] = [0.0, 0.0, 1.0];

const STATIC_FIELDS: [&str; 2] = ["index", "contents"];
const FLUID_FIELDS: [&str; 6] = [
    "index",
    "surfaceprop",
    "damping",
    "contents",
    "surfaceplane",
    "currentvelocity",
];

pub(super) fn fluid_property(contents: i32) -> Result<&'static str, String> {
    match contents {
        source_formats::bsp::contents::WATER => Ok("water"),
        source_formats::bsp::contents::SLIME => Ok("slime"),
        _ => Err("fluid body needs water or slime contents".into()),
    }
}

fn fields<'a>(node: &'a Node, names: &[&str], control: &Control) -> Result<&'a [Node], String> {
    let fields = node.children().ok_or("body keys require a block")?;
    if fields.len() != names.len() {
        return Err("body keys have missing, duplicate or extra fields".into());
    }
    for (index, field) in fields.iter().enumerate() {
        control.checkpoint()?;
        if field.as_str().is_none()
            || !names
                .iter()
                .any(|name| field.key.eq_ignore_ascii_case(name))
            || fields[..index]
                .iter()
                .any(|previous| previous.key.eq_ignore_ascii_case(&field.key))
        {
            return Err("body keys have nested, unknown or duplicate fields".into());
        }
    }
    Ok(fields)
}

fn field<'a>(fields: &'a [Node], name: &str) -> Result<&'a str, String> {
    fields
        .iter()
        .find(|field| field.key.eq_ignore_ascii_case(name))
        .and_then(Node::as_str)
        .ok_or_else(|| format!("body keys are missing {name}"))
}

fn index(value: &str) -> Result<usize, String> {
    value
        .trim_ascii()
        .parse()
        .map_err(|_| "invalid body or material index".into())
}

fn number(value: &str) -> Result<f32, String> {
    value
        .trim_ascii()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| "numeric keys must be finite native floats".into())
}

fn vector<const N: usize>(value: &str) -> Result<[f32; N], String> {
    let mut words = value.split_ascii_whitespace();
    let mut result = [0.0; N];
    for component in &mut result {
        *component = number(words.next().ok_or("vector key has too few components")?)?;
    }
    if words.next().is_some() {
        return Err("vector key has too many components".into());
    }
    Ok(result)
}

fn material_table(node: &Node, materials: &[String], control: &Control) -> Result<(), String> {
    let entries = node.children().ok_or("material table requires a block")?;
    if entries.len() != materials.len() {
        return Err("material table has missing or extra entries".into());
    }
    let mut seen = vec![false; materials.len()];
    for entry in entries {
        control.checkpoint()?;
        let slot = index(
            entry
                .as_str()
                .ok_or("material table entries must be strings")?,
        )?;
        let position = slot
            .checked_sub(1)
            .filter(|&index| index < materials.len())
            .ok_or("material table index is outside the request")?;
        if std::mem::replace(&mut seen[position], true)
            || !entry.key.eq_ignore_ascii_case(&materials[position])
        {
            return Err(
                "material table has duplicate indices or mismatched surface properties".into(),
            );
        }
    }
    Ok(())
}

pub(super) fn validate(
    data: &[u8],
    expected: &Model,
    materials: &[String],
    control: &Control,
) -> Result<(), String> {
    control.checkpoint()?;
    if expected.index != 0
        || expected.bodies.is_empty()
        || expected.bodies.len() > MAX_PHYSICS_SOLIDS
        || materials.is_empty()
        || materials.len() > MAX_SURFACE_PROPERTIES
    {
        return Err("invalid requested world body or material counts".into());
    }
    if data.len() > MAX_PHYSICS_KEY_BYTES {
        return Err("world key text exceeds its native byte limit".into());
    }
    let text = std::str::from_utf8(data)
        .map_err(|_| "world keys are not UTF-8")?
        .strip_suffix('\0')
        .ok_or("world keys need one terminator")?;
    let max_nodes = expected.bodies.len() * (FLUID_FIELDS.len() + 1) + materials.len() + 1;
    let nodes = keyvalues::parse_bounded(text, 1, max_nodes).map_err(|error| error.to_string())?;
    if nodes.len() != expected.bodies.len() + 1 {
        return Err("world keys need one block per requested body and one material table".into());
    }
    let mut seen = vec![false; expected.bodies.len()];
    let mut table = false;
    for node in &nodes {
        control.checkpoint()?;
        if node.key.eq_ignore_ascii_case("materialtable") {
            if std::mem::replace(&mut table, true) {
                return Err("world keys contain duplicate material tables".into());
            }
            material_table(node, materials, control)?;
            continue;
        }
        let fluid = node.key.eq_ignore_ascii_case("fluid");
        if !fluid && !node.key.eq_ignore_ascii_case("staticsolid") {
            return Err("world keys contain an unsupported body role".into());
        }
        let values = fields(
            node,
            if fluid {
                &FLUID_FIELDS[..]
            } else {
                &STATIC_FIELDS[..]
            },
            control,
        )?;
        let slot = index(field(values, "index")?)?;
        let body = expected
            .bodies
            .get(slot)
            .ok_or("body index is outside the request")?;
        if std::mem::replace(&mut seen[slot], true) {
            return Err("world keys contain duplicate body indices".into());
        }
        let contents = field(values, "contents")?
            .trim_ascii()
            .parse::<i32>()
            .map_err(|_| "body contents must be an integer")?;
        if fluid != body.surface.is_some() || contents != body.contents {
            return Err("world key body role or contents disagree with the request".into());
        }
        if let Some(height) = body.surface {
            let plane = vector::<4>(field(values, "surfaceplane")?)?;
            if plane[..3] != FLUID_NORMAL[..]
                || plane[3] != height
                || !field(values, "surfaceprop")?
                    .eq_ignore_ascii_case(fluid_property(body.contents)?)
                || number(field(values, "damping")?)?
                    != body
                        .damping
                        .unwrap_or(super::damping::DEFAULT_WATER_DAMPING)
                || vector::<3>(field(values, "currentvelocity")?)?
                    != super::current::native(body.current_hu)
            {
                return Err(
                    "fluid surface, property, damping or current disagrees with the request".into(),
                );
            }
        }
    }
    if !table || seen.iter().any(|&seen| !seen) {
        return Err("world keys are missing a requested body or material table".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
