use std::collections::{BTreeMap, BTreeSet};
use std::io;

use source_formats::gamefs::{GameFs, asset_relative};
use source_formats::vmt::{self, Material};
use source_formats::vtf::{ImageFormat, Vtf};

use super::metrics::{DimensionProvider, HeaderProvider, MetricPlan};
use super::{MAX_GEOMETRY_MATERIAL_BYTES, MAX_METRIC_TEXTURE_BYTES, Materials, Requirements};
use crate::Control;
use crate::asset_sources::{AssetSourceGuard, BuildMaterialSources};
use crate::brush_shadow_casters::{opaque_parameters, static_material_raw};

const MAX_PATH_BYTES: usize = 259;
const MAX_TEXTURE_EDGE: u32 = 32768;
const MAX_DETAIL_SCALAR_BYTES: usize = 32;
const DEFAULT_DETAIL_SCALE: f32 = 4.0;
const MAX_DETAIL_BLEND_MODE: u32 = 3;
const UNSUPPORTED_TEXTURE_FLAGS: u32 = 0x80 | 0x800 | 0x4000 | 0x8000 | 0x1_0000 | 0x800_0000;
const MATERIAL_PARAMETERS: [&str; 6] = [
    "$basetexture",
    "$surfaceprop",
    "$surfaceprop2",
    "$reflectivity",
    "%keywords",
    "%tooltexture",
];
const DETAIL_PARAMETERS: [&str; 4] = [
    "$detail",
    "$detailscale",
    "$detailblendfactor",
    "$detailblendmode",
];

#[derive(Debug, Clone)]
pub struct FlatTerrainMaterialCapture {
    pub dimensions: BTreeMap<String, [u32; 2]>,
    pub detail_scales: BTreeMap<String, f32>,
    pub secondary_surfaces: BTreeSet<String>,
    pub sources: AssetSourceGuard,
}

/// Captures static LightmappedGeneric or WorldVertexTransition mapping dimensions and asset guards.
/// Names are case-insensitive relative references without prefixes or extensions.
/// Uses geometry-material and metric-texture read limits without retaining payloads.
/// Detail scales require a separate exact scaled-UV check on every native sample.
pub fn capture_flat_terrain_materials<'a>(
    game: &GameFs,
    names: impl IntoIterator<Item = &'a str>,
    control: &Control,
) -> Result<FlatTerrainMaterialCapture, String> {
    let sources = BuildMaterialSources::without_retained_payloads();
    let mut materials =
        Materials::with_build_sources(game, Requirements::GeometrySemantics, &sources);
    let mut dimensions = BTreeMap::new();
    let mut detail_scales = BTreeMap::new();
    let mut secondary_surfaces = BTreeSet::new();
    for (index, name) in names.into_iter().enumerate() {
        control.checkpoint()?;
        if index >= map_core::clip::MAX_TERRAIN_SOURCES {
            return Err(format!(
                "Flat terrain material capture exceeds {} selected names",
                map_core::clip::MAX_TERRAIN_SOURCES
            ));
        }
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() || name.len() > MAX_PATH_BYTES {
            return Err(format!(
                "Flat terrain material names require 1..={MAX_PATH_BYTES} bytes"
            ));
        }
        checked_reference(&name, ".vmt")?;
        if dimensions.contains_key(&name) {
            continue;
        }
        let (size, detail_scale, secondary_surface) =
            capture_material(&mut materials, &name, control)?;
        if let Some(scale) = detail_scale {
            detail_scales.insert(name.clone(), scale);
        }
        if secondary_surface {
            secondary_surfaces.insert(name.clone());
        }
        dimensions.insert(name, size);
    }
    if dimensions.is_empty() {
        return Err("Flat terrain material capture requires a selected material".into());
    }
    let sources = sources
        .source_guard(game, control)
        .map_err(|error| format!("Flat terrain material sources changed: {error}"))?;
    Ok(FlatTerrainMaterialCapture {
        dimensions,
        detail_scales,
        secondary_surfaces,
        sources,
    })
}

fn checked_path(path: &str) -> Result<(), String> {
    if path.len() > MAX_PATH_BYTES
        || !path.is_ascii()
        || path.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(format!(
            "Flat terrain asset paths require ASCII names within {MAX_PATH_BYTES} bytes"
        ));
    }
    asset_relative(path)
        .map(|_| ())
        .map_err(|error| format!("Flat terrain asset path {path}: {error}"))
}

fn checked_reference(reference: &str, extension: &str) -> Result<(), String> {
    if reference.is_empty()
        || reference.starts_with("materials/")
        || reference.starts_with('_')
        || reference == "env_cubemap"
        || reference.ends_with(extension)
        || reference
            .rsplit('/')
            .next()
            .is_none_or(|name| name.contains('.'))
        || !reference.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'/' | b'_' | b'-' | b'.')
        })
        || reference
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(format!(
            "Flat terrain reference {reference:?} requires a lowercase relative name without a materials/ prefix, {extension} suffix, runtime provider, alias or special characters"
        ));
    }
    Ok(())
}

fn capture_material(
    materials: &mut Materials<'_>,
    name: &str,
    control: &Control,
) -> Result<([u32; 2], Option<f32>, bool), String> {
    let path = vmt::material_path(name);
    checked_path(&path)?;
    let material = Material::load_with_reader(&path, &mut |path| {
        control.checkpoint().map_err(io::Error::other)?;
        let bytes = materials.geometry_reads.read(
            materials.fs,
            path,
            MAX_GEOMETRY_MATERIAL_BYTES,
            None,
        )?;
        if let Some(bytes) = &bytes {
            let direct = static_material_raw(path, bytes, &["lightmappedgeneric", "worldvertextransition"]).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("Flat terrain retiling requires a direct static LightmappedGeneric or WorldVertexTransition material: {error}"),
                )
            })?;
            if !direct {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Flat terrain retiling requires a direct static LightmappedGeneric or WorldVertexTransition material; Patch is unsupported",
                ));
            }
        }
        Ok(bytes)
    })
    .map_err(|error| format!("Flat terrain material {path}: {error}"))?
    .ok_or_else(|| format!("Flat terrain material {path} was not found"))?;
    let transition = material.shader == "worldvertextransition";
    for (key, _) in &material.params {
        let shader_parameter = if transition {
            key == "$basetexture2"
        } else {
            DETAIL_PARAMETERS.contains(&key.as_str())
        };
        if !MATERIAL_PARAMETERS.contains(&key.as_str()) && !shader_parameter {
            return Err(format!(
                "Flat terrain material {path} has unsupported parameter {key}; retiling requires an untransformed static base texture",
            ));
        }
    }
    opaque_parameters(&path, |key| material.get(key))?;
    if let Some(reflectivity) = material.get("$reflectivity") {
        super::parse_reflectivity(reflectivity)
            .map_err(|error| format!("Flat terrain material {path}: {error}"))?;
    }
    let secondary_surface = secondary_surface(&material)
        .map_err(|error| format!("Flat terrain material {path}: {error}"))?;
    let detail_scale = if transition {
        None
    } else {
        detail_parameters(&material)
            .map_err(|error| format!("Flat terrain material {path}: {error}"))?
    };
    let base_dimensions = capture_texture(materials, &material, "$basetexture", &path, control)?;
    if transition {
        capture_texture(materials, &material, "$basetexture2", &path, control)?;
    }
    if detail_scale.is_some() {
        capture_texture(materials, &material, "$detail", &path, control)?;
    }
    let dimensions = match MetricPlan::new(&material, Some("$basetexture"))
        .map_err(|error| format!("Flat terrain material {path}: {error}"))?
        .dimensions
    {
        DimensionProvider::Header(provider, _) if provider.key == "$basetexture" => base_dimensions,
        DimensionProvider::Header(provider, _) => {
            capture_texture(materials, &material, provider.key, &path, control)?
        }
        DimensionProvider::CompileDefault | DimensionProvider::NativeNoPreview => {
            return Err(format!(
                "Flat terrain material {path} requires explicit static texture dimensions"
            ));
        }
    };
    Ok((dimensions, detail_scale, secondary_surface))
}

fn secondary_surface(material: &Material) -> Result<bool, String> {
    let Some(value) = material.get("$surfaceprop2") else {
        return Ok(false);
    };
    let value = value.trim();
    if !crate::physics::valid_surface_property(value) {
        return Err(
            "$surfaceprop2 requires 1–127 ASCII letters, digits, underscores, dots or dashes"
                .into(),
        );
    }
    Ok(true)
}

fn detail_parameters(material: &Material) -> Result<Option<f32>, String> {
    let controls = ["$detailscale", "$detailblendfactor", "$detailblendmode"];
    if material.get("$detail").is_none() {
        if controls.iter().any(|key| material.get(key).is_some()) {
            return Err("Detail controls require an explicit static $detail texture".into());
        }
        return Ok(None);
    }
    if let Some(value) = material.get("$detailblendfactor") {
        let value = value.trim_ascii();
        if value.len() > MAX_DETAIL_SCALAR_BYTES {
            return Err("$detailblendfactor requires a finite scalar from zero to one".into());
        }
        let factor = value.parse::<f32>().map_err(|_| {
            "$detailblendfactor requires a finite scalar from zero to one".to_string()
        })?;
        if !factor.is_finite() || !(0.0..=1.0).contains(&factor) {
            return Err("$detailblendfactor requires a finite scalar from zero to one".into());
        }
    }
    if let Some(value) = material.get("$detailblendmode") {
        let value = value.trim_ascii();
        if value.len() > MAX_DETAIL_SCALAR_BYTES {
            return Err("$detailblendmode requires an integer from zero to three".into());
        }
        let mode = value
            .parse::<u32>()
            .map_err(|_| "$detailblendmode requires an integer from zero to three".to_string())?;
        if mode > MAX_DETAIL_BLEND_MODE {
            return Err("$detailblendmode requires an integer from zero to three".into());
        }
    }
    material
        .get("$detailscale")
        .map_or(Ok(DEFAULT_DETAIL_SCALE), detail_scale)
        .map(Some)
}

fn detail_scale(value: &str) -> Result<f32, String> {
    let invalid = || {
        format!(
            "$detailscale requires a positive exact native float in a decimal literal of at most {MAX_DETAIL_SCALAR_BYTES} bytes without an exponent"
        )
    };
    let value = value.trim_ascii();
    if value.len() > MAX_DETAIL_SCALAR_BYTES {
        return Err(invalid());
    }
    let value = value.strip_prefix('+').unwrap_or(value);
    let mut numerator = 0u128;
    let mut denominator = 1u128;
    let mut fractional = false;
    let mut digits = 0;
    for byte in value.bytes() {
        if byte == b'.' && !fractional {
            fractional = true;
            continue;
        }
        if !byte.is_ascii_digit() {
            return Err(invalid());
        }
        numerator = numerator * 10 + u128::from(byte - b'0');
        if fractional {
            denominator *= 10;
        }
        digits += 1;
    }
    if digits == 0 || numerator == 0 {
        return Err(invalid());
    }
    while denominator.is_multiple_of(5) {
        if !numerator.is_multiple_of(5) {
            return Err(invalid());
        }
        numerator /= 5;
        denominator /= 5;
    }
    if u128::BITS - numerator.leading_zeros() - numerator.trailing_zeros() > f32::MANTISSA_DIGITS {
        return Err(invalid());
    }
    let scale = (numerator as f64 / denominator as f64) as f32;
    scale.is_normal().then_some(scale).ok_or_else(invalid)
}

fn capture_texture(
    materials: &mut Materials<'_>,
    material: &Material,
    key: &'static str,
    path: &str,
    control: &Control,
) -> Result<[u32; 2], String> {
    let provider = HeaderProvider::new(material, key)
        .map_err(|error| format!("Flat terrain material {path}: {error}"))?;
    checked_path(&provider.path)?;
    if provider.runtime() {
        return Err(format!(
            "Flat terrain material {path} uses a runtime-generated {key} texture",
        ));
    }
    checked_reference(
        &material
            .get(key)
            .unwrap()
            .replace('\\', "/")
            .to_ascii_lowercase(),
        ".vtf",
    )
    .map_err(|error| format!("Flat terrain material {path}: {error}"))?;
    control.checkpoint()?;
    let bytes = materials
        .texture_reads
        .read(materials.fs, &provider.path, MAX_METRIC_TEXTURE_BYTES, None)
        .map_err(|error| format!("Flat terrain material {path}: {error}"))?
        .ok_or_else(|| {
            format!(
                "Flat terrain material {path} {key} texture {} was not found",
                provider.path
            )
        })?;
    let texture = Vtf::parse(bytes)
        .map_err(|error| format!("Flat terrain {key} texture {}: {error}", provider.path))?;
    control.checkpoint()?;
    let validated = if key == "%tooltexture" {
        validate_dimensions(&texture)
    } else {
        validate_texture(&texture)
    };
    validated.map_err(|error| {
        format!(
            "Flat terrain {key} texture {} cannot supply static native UV data: {error}",
            provider.path
        )
    })?;
    Ok([texture.width, texture.height])
}

fn validate_texture(texture: &Vtf) -> Result<(), String> {
    if texture.authored_frames != 1
        || texture.frames != 1
        || texture.faces != 1
        || texture.depth != 1
        || texture.authored_depth.is_some_and(|depth| depth != 1)
        || texture.first_frame != 0
        || texture.flags & UNSUPPORTED_TEXTURE_FLAGS != 0
    {
        return Err("requires one static ordinary frame, face and depth without normal, procedural, environment or render-target flags".into());
    }
    if !matches!(
        texture.format,
        ImageFormat::Rgba8888
            | ImageFormat::Abgr8888
            | ImageFormat::Rgb888
            | ImageFormat::Bgr888
            | ImageFormat::Rgb565
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
    ) {
        return Err("requires an ordinary native RGB or RGBA image format".into());
    }
    validate_dimensions(texture)?;
    texture
        .validate_native_mips()
        .map_err(|error| error.to_string())
}

fn validate_dimensions(texture: &Vtf) -> Result<(), String> {
    if [texture.width, texture.height]
        .iter()
        .any(|edge| !edge.is_power_of_two() || *edge > MAX_TEXTURE_EDGE)
    {
        return Err(format!(
            "requires positive power-of-two dimensions no larger than {MAX_TEXTURE_EDGE}"
        ));
    }
    Ok(())
}
