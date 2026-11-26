use source_formats::{
    gamefs::asset_relative,
    vmf::parse_vec3,
    vmt::{Material, texture_path},
    vtf::{ImageFormat, Vtf},
};

use super::{Emission, ORDINARY_COLOR_UNSUPPORTED_TEXTURE_FLAGS};

pub(super) fn resolve(
    material: &Material,
    base: Option<&Vtf>,
    display_texture: Option<&str>,
) -> Result<Option<Emission>, String> {
    if !requested(material) {
        return Ok(None);
    }
    let unlit = material.shader == "unlitgeneric";
    if !unlit
        && !matches!(
            material.shader.as_str(),
            "lightmappedgeneric" | "vertexlitgeneric"
        )
    {
        return Err(
            "Self-illumination requires plain LightmappedGeneric or VertexLitGeneric".into(),
        );
    }
    if material.is_tool() || material.translucent() || material.alpha_test() {
        return Err("Material emission requires an opaque non-tool material".into());
    }
    if !material.blocks.is_empty() {
        return Err("Material emission does not support proxies or fallback blocks".into());
    }
    if let Some((key, _)) = material.params.iter().find(|(key, _)| {
        !matches!(
            key.as_str(),
            "$basetexture"
                | "$basetexturetransform"
                | "$model"
                | "$surfaceprop"
                | "$surfaceprop2"
                | "$nodecal"
                | "%keywords"
                | "$selfillum"
        ) && (unlit || key != "$selfillumtint")
    }) {
        let key: String = key.chars().take(128).collect();
        return Err(format!(
            "Material emission does not support parameter {key}"
        ));
    }
    let enabled = material.get("$selfillum").map_or(Ok(false), |value| {
        match value.trim().parse::<i32>() {
            Ok(0) => Ok(false),
            Ok(1) => Ok(true),
            _ => Err("$selfillum requires integer 0 or 1"),
        }
    })?;
    if !unlit && !enabled {
        return Err("Self-illumination tint requires $selfillum 1".into());
    }
    if !super::uv::is_axis_identity(material.get("$basetexturetransform")) {
        return Err("Material emission requires an identity base texture transform".into());
    }
    let declared = material
        .get("$basetexture")
        .filter(|value| !value.trim().is_empty() && asset_relative(value.trim()).is_ok())
        .map(texture_path);
    let Some(base) = base.filter(|_| declared.is_some() && declared.as_deref() == display_texture)
    else {
        return Err("Material emission requires the exact declared base texture".into());
    };
    crate::normal_maps::validate_static_texture(base, u32::from(u16::MAX))
        .map_err(|error| format!("Material emission base texture: {error}"))?;
    if base.flags & ORDINARY_COLOR_UNSUPPORTED_TEXTURE_FLAGS != 0
        || !(crate::normal_maps::ordinary_ldr_format(base.format)
            || base.format == ImageFormat::Dxt1OneBitAlpha)
    {
        return Err(
            "Material emission requires an ordinary repeat-sampled LDR base texture".into(),
        );
    }
    if unlit {
        if base.has_alpha() {
            return Err("UnlitGeneric emission requires an opaque base texture".into());
        }
        return Ok(Some(Emission {
            tint: [1.0; 3],
            base_alpha: false,
        }));
    }
    if !base.has_alpha()
        || !matches!(
            base.format,
            ImageFormat::Rgba8888
                | ImageFormat::Bgra8888
                | ImageFormat::Dxt1
                | ImageFormat::Dxt1OneBitAlpha
                | ImageFormat::Dxt3
                | ImageFormat::Dxt5
        )
    {
        return Err("Self-illumination requires a native base texture alpha mask".into());
    }
    Ok(Some(Emission {
        tint: material
            .get("$selfillumtint")
            .map_or(Ok([1.0; 3]), |value| tint(value, "$selfillumtint"))?,
        base_alpha: true,
    }))
}

pub(super) fn requested(material: &Material) -> bool {
    material.shader == "unlitgeneric"
        || material.params.iter().any(|(key, value)| {
            key.starts_with("$selfillum")
                && (key != "$selfillum" || value.trim().parse::<i32>() != Ok(0))
        })
}

pub(super) fn tint(value: &str, key: &str) -> Result<[f32; 3], String> {
    let value = value.trim();
    let (components, maximum) = if let Some(raw) = value
        .strip_prefix('[')
        .and_then(|raw| raw.strip_suffix(']'))
    {
        (raw, 1.0)
    } else if let Some(raw) = value
        .strip_prefix('{')
        .and_then(|raw| raw.strip_suffix('}'))
    {
        (raw, 255.0)
    } else {
        ("", 1.0)
    };
    (!components.contains(['[', ']', '(', ')', '{', '}']))
        .then(|| parse_vec3(components))
        .flatten()
        .filter(|rgb| rgb.is_finite() && rgb.min_element() >= 0.0 && rgb.max_element() <= maximum)
        .map(|rgb| (rgb.as_vec3() * (1.0 / maximum as f32)).to_array())
        .ok_or_else(|| format!("{key} requires three bounded channels in [0–1] or {{0–255}}"))
}
