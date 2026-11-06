use std::collections::BTreeMap;
use std::io;

use source_formats::gamefs::{GameFs, asset_relative};
use source_formats::vmt::{self, Material};
use source_formats::vtf::{ImageFormat, Vtf};

use super::metrics::HeaderProvider;
use super::{MAX_GEOMETRY_MATERIAL_BYTES, MAX_METRIC_TEXTURE_BYTES, Materials, Requirements};
use crate::Control;
use crate::asset_sources::{AssetSourceGuard, BuildMaterialSources};
use crate::brush_shadow_casters::{opaque_parameters, static_material_raw};

const MAX_PATH_BYTES: usize = 259;
const MAX_TEXTURE_EDGE: u32 = 32768;
const UNSUPPORTED_TEXTURE_FLAGS: u32 = 0x80 | 0x800 | 0x4000 | 0x8000 | 0x1_0000 | 0x800_0000;
const MATERIAL_PARAMETERS: [&str; 5] = [
    "$basetexture",
    "$surfaceprop",
    "$surfaceprop2",
    "$reflectivity",
    "%keywords",
];

#[derive(Debug, Clone)]
pub struct FlatTerrainMaterialCapture {
    pub dimensions: BTreeMap<String, [u32; 2]>,
    pub sources: AssetSourceGuard,
}

/// Captures static LightmappedGeneric base-VTF mapping dimensions and original asset guards.
/// Names are case-insensitive relative references without prefixes or extensions.
/// Uses geometry-material and metric-texture read limits without retaining payloads.
pub fn capture_flat_terrain_materials<'a>(
    game: &GameFs,
    names: impl IntoIterator<Item = &'a str>,
    control: &Control,
) -> Result<FlatTerrainMaterialCapture, String> {
    let sources = BuildMaterialSources::without_retained_payloads();
    let mut materials =
        Materials::with_build_sources(game, Requirements::GeometrySemantics, &sources);
    let mut dimensions = BTreeMap::new();
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
        let size = capture_material(&mut materials, &name, control)?;
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
) -> Result<[u32; 2], String> {
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
            let direct = static_material_raw(path, bytes, &["lightmappedgeneric"]).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("Flat terrain retiling requires a direct static LightmappedGeneric material: {error}"),
                )
            })?;
            if !direct {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Flat terrain retiling requires a direct static LightmappedGeneric material; Patch is unsupported",
                ));
            }
        }
        Ok(bytes)
    })
    .map_err(|error| format!("Flat terrain material {path}: {error}"))?
    .ok_or_else(|| format!("Flat terrain material {path} was not found"))?;
    for (key, _) in &material.params {
        if !MATERIAL_PARAMETERS.contains(&key.as_str()) {
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
    let provider = HeaderProvider::new(&material, "$basetexture")
        .map_err(|error| format!("Flat terrain material {path}: {error}"))?;
    checked_path(&provider.path)?;
    if provider.runtime() {
        return Err(format!(
            "Flat terrain material {path} uses a runtime-generated base texture",
        ));
    }
    checked_reference(
        &material.get("$basetexture").unwrap().to_ascii_lowercase(),
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
                "Flat terrain material {path} base texture {} was not found",
                provider.path
            )
        })?;
    let texture = Vtf::parse(bytes)
        .map_err(|error| format!("Flat terrain base texture {}: {error}", provider.path))?;
    control.checkpoint()?;
    validate_texture(&texture).map_err(|error| {
        format!(
            "Flat terrain base texture {} cannot supply native mapping dimensions: {error}",
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
    if [texture.width, texture.height]
        .iter()
        .any(|edge| !edge.is_power_of_two() || *edge > MAX_TEXTURE_EDGE)
    {
        return Err(format!(
            "requires positive power-of-two dimensions no larger than {MAX_TEXTURE_EDGE}"
        ));
    }
    texture
        .validate_native_mips()
        .map_err(|error| error.to_string())
}
