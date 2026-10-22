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
pub(super) enum Dependency {
    Include(String),
    Texture(String),
}

pub(super) fn dependency(path: &str, bytes: &[u8]) -> io::Result<Dependency> {
    if bytes.len() > MAX_MATERIAL_BYTES {
        return Err(invalid(format!(
            "Static-prop material exceeds its byte budget: {path}"
        )));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| invalid(format!("Static-prop material is not UTF-8: {path}")))?;
    let nodes = keyvalues::parse_static_material_bounded(text, 2, 3)
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
    let [parameter] = children else {
        return Err(invalid(format!(
            "Static-prop material requires only one $basetexture or Patch include: {path}"
        )));
    };
    let patch = root.key.eq_ignore_ascii_case("Patch");
    let key = if patch { "include" } else { "$basetexture" };
    if (!patch && !root.key.eq_ignore_ascii_case("VertexLitGeneric"))
        || !parameter.key.eq_ignore_ascii_case(key)
    {
        return Err(invalid(format!(
            "Static-prop material supports only plain VertexLitGeneric or include-only Patch: {path}"
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
    if parsed.shader != shader
        || parsed.params.len() != 1
        || !parsed.blocks.is_empty()
        || parsed.params[0] != (key.into(), reference.into())
    {
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
        Ok(Dependency::Include(logical_path(name, "vmt")?))
    } else {
        Ok(Dependency::Texture(logical_path(reference, "vtf")?))
    }
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
