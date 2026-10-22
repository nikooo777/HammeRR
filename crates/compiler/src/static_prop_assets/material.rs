use std::io;

use source_formats::gamefs::asset_relative;
use source_formats::keyvalues;
use source_formats::vmt::Material;
use source_formats::vtf::{ImageFormat, Vtf};

use super::{MAX_MATERIAL_BYTES, MAX_TEXTURE_AXIS, checkpoint, invalid};
use crate::Control;

pub(super) fn logical_path(name: &str, extension: &str) -> io::Result<String> {
    if name.is_empty()
        || name.trim() != name
        || name.starts_with("materials/")
        || name.starts_with('_')
        || name.contains('.')
        || name.bytes().any(|byte| {
            !byte.is_ascii_lowercase()
                && !byte.is_ascii_digit()
                && !matches!(byte, b'/' | b'_' | b'-')
        })
        || asset_relative(name)? != name
    {
        return Err(invalid(format!(
            "Static-prop resource must use a canonical lowercase logical name: {name}"
        )));
    }
    let path = format!("materials/{name}.{extension}");
    if path.len() > source_formats::bsp::PakLimits::default().max_name_bytes {
        return Err(invalid(
            "Static-prop material or texture path exceeds 259 bytes",
        ));
    }
    Ok(path)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Dependency {
    Include(String),
    EditedInclude { include: String, texture: String },
    Texture(String),
}

pub(crate) fn dependency(path: &str, bytes: &[u8]) -> io::Result<Dependency> {
    if bytes.len() > MAX_MATERIAL_BYTES {
        return Err(invalid(format!(
            "Static-prop material exceeds its byte budget: {path}"
        )));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| invalid(format!("Static-prop material is not UTF-8: {path}")))?;
    let nodes = keyvalues::parse_static_material_bounded(text, 2, 4)
        .map_err(|error| invalid(format!("Unsupported static-prop material {path}: {error}")))?;
    let [root] = nodes.as_slice() else {
        return Err(invalid(format!(
            "Static-prop material requires exactly one shader root: {path}"
        )));
    };
    let Some(children) = root.children() else {
        return Err(invalid(format!(
            "Static-prop material requires a shader block: {path}"
        )));
    };
    let patch = root.key.eq_ignore_ascii_case("Patch");
    if !patch && !root.key.eq_ignore_ascii_case("VertexLitGeneric") {
        return Err(invalid(format!(
            "Unsupported static-prop material {path}: requires VertexLitGeneric or Patch"
        )));
    }
    let (parameter, edit) = match children {
        [parameter] => (parameter, None),
        [first, second] if patch => {
            let (parameter, section) = if first.key.eq_ignore_ascii_case("include") {
                (first, second)
            } else {
                (second, first)
            };
            if !section.key.eq_ignore_ascii_case("insert")
                && !section.key.eq_ignore_ascii_case("replace")
            {
                return Err(invalid(format!(
                    "Unsupported static-prop material {path}: requires one insert or replace block"
                )));
            }
            let Some([texture]) = section.children() else {
                return Err(invalid(format!(
                    "Unsupported static-prop material {path}: Patch edit requires one scalar $basetexture"
                )));
            };
            let reference = texture
                .as_str()
                .filter(|_| texture.key.eq_ignore_ascii_case("$basetexture"))
                .ok_or_else(|| {
                    invalid(format!(
                        "Unsupported static-prop material {path}: Patch edit requires one scalar $basetexture"
                    ))
                })?;
            (parameter, Some((section, reference)))
        }
        [first, second] if !patch => {
            let (parameter, alpha) = if first.key.eq_ignore_ascii_case("$basetexture") {
                (first, second)
            } else {
                (second, first)
            };
            if !alpha.key.eq_ignore_ascii_case("$alphatest") || alpha.as_str() != Some("1") {
                return Err(invalid(format!(
                    "Unsupported static-prop material {path}: alpha-tested VertexLitGeneric requires one literal $alphatest 1"
                )));
            }
            (parameter, None)
        }
        _ => {
            return Err(invalid(format!(
                "Unsupported static-prop material {path}: requires one $basetexture or Patch include with at most one edit"
            )));
        }
    };
    let key = if patch { "include" } else { "$basetexture" };
    if !parameter.key.eq_ignore_ascii_case(key) {
        return Err(invalid(format!(
            "Unsupported static-prop material {path}: requires one scalar {key}"
        )));
    }
    let reference = parameter.as_str().ok_or_else(|| {
        invalid(format!(
            "Static-prop {key} must be a scalar resource name: {path}"
        ))
    })?;
    let parsed = Material::parse(text)
        .map_err(|error| invalid(format!("Invalid static-prop material {path}: {error}")))?;
    let shader = if patch { "patch" } else { "vertexlitgeneric" };
    let blocks_agree = match edit {
        Some((section, _)) => parsed.blocks.as_slice() == std::slice::from_ref(section),
        None => parsed.blocks.is_empty(),
    };
    let mut raw_params = children
        .iter()
        .filter_map(|node| node.as_str().map(|value| (node.key.as_str(), value)));
    let params_agree = parsed.params.iter().all(|(key, value)| {
        raw_params.next().is_some_and(|(raw_key, raw_value)| {
            raw_key.eq_ignore_ascii_case(key) && raw_value == value.as_str()
        })
    }) && raw_params.next().is_none();
    if parsed.shader != shader || !params_agree || !blocks_agree {
        return Err(invalid(format!(
            "Static-prop raw and parsed material disagree: {path}"
        )));
    }
    if patch {
        let name = reference
            .strip_prefix("materials/")
            .and_then(|name| name.strip_suffix(".vmt"))
            .ok_or_else(|| {
                invalid(format!(
                    "Static-prop Patch include must use a canonical materials/...vmt path: {path}"
                ))
            })?;
        let include = logical_path(name, "vmt")?;
        if let Some((_, reference)) = edit {
            Ok(Dependency::EditedInclude {
                include,
                texture: logical_path(reference, "vtf")?,
            })
        } else {
            Ok(Dependency::Include(include))
        }
    } else {
        Ok(Dependency::Texture(logical_path(reference, "vtf")?))
    }
}

pub(crate) fn validate_edit_chain(depth: usize, edited: bool) -> io::Result<()> {
    if edited && depth > 1 {
        return Err(invalid(
            "Static-prop Patch edits require a direct plain VertexLitGeneric include",
        ));
    }
    Ok(())
}

pub(super) fn texture(path: &str, bytes: Vec<u8>, control: &Control) -> io::Result<()> {
    checkpoint(control)?;
    let parsed = Vtf::parse(bytes.clone())
        .map_err(|error| invalid(format!("Unsupported static-prop VTF {path}: {error}")))?;
    if parsed.version.1 > 4
        || parsed.width > MAX_TEXTURE_AXIS
        || parsed.height > MAX_TEXTURE_AXIS
        || !parsed.width.is_power_of_two()
        || !parsed.height.is_power_of_two()
        || parsed.authored_frames != 1
        || parsed.first_frame != 0
        || parsed.authored_depth.is_some_and(|depth| depth != 1)
        || parsed.depth != 1
        || parsed.faces != 1
        || parsed.flags & (0x800 | 0x4000 | 0x8000 | 0x10000 | 0x4000000) != 0
        || parsed
            .reflectivity
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        || !matches!(
            parsed.format,
            ImageFormat::Rgba8888
                | ImageFormat::Abgr8888
                | ImageFormat::Rgb888
                | ImageFormat::Bgr888
                | ImageFormat::Rgb565
                | ImageFormat::I8
                | ImageFormat::Ia88
                | ImageFormat::A8
                | ImageFormat::Rgb888Bluescreen
                | ImageFormat::Bgr888Bluescreen
                | ImageFormat::Argb8888
                | ImageFormat::Bgra8888
                | ImageFormat::Dxt1
                | ImageFormat::Dxt3
                | ImageFormat::Dxt5
                | ImageFormat::Bgrx8888
                | ImageFormat::Bgr565
                | ImageFormat::Bgrx5551
                | ImageFormat::Bgra4444
                | ImageFormat::Dxt1OneBitAlpha
                | ImageFormat::Bgra5551
        )
    {
        return Err(invalid(format!(
            "Static-prop VTF requires a supported ordinary one-frame depth-one texture: {path}"
        )));
    }
    if parsed.version.1 >= 3 {
        let count = u32::from_le_bytes(bytes[68..72].try_into().unwrap());
        if count != 1 || bytes.get(80..84) != Some(&[0x30, 0, 0, 0]) {
            return Err(invalid(format!(
                "Static-prop VTF resource extensions are unsupported: {path}"
            )));
        }
    }
    for mip in 0..parsed.mip_count {
        checkpoint(control)?;
        parsed.image_data(mip, 0, 0).map_err(|error| {
            invalid(format!(
                "Incomplete static-prop VTF mip layout {path}: {error}"
            ))
        })?;
    }
    Ok(())
}
