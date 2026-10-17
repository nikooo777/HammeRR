use std::collections::{BTreeMap, BTreeSet};

use render::MaterialLibrary;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use source_formats::gamefs::{AssetOrigin, GameFs, asset_relative};
use source_formats::vmt::{
    DuplicateOperations, Material, material_path, texture_path, texture_transform,
};
use source_formats::vtf::{ImageFormat, Vtf};

const MAX_MATERIAL_BYTES: usize = 1024 * 1024;
const MAX_TEXTURE_BYTES: usize = 16 * 1024 * 1024;
const MAX_READ_BYTES: usize = 64 * 1024 * 1024;
const MAX_DECODE_BYTES: usize = 64 * 1024 * 1024;
const MAX_ASSETS: usize = 256;
const MAX_PHYSICAL_PATHS: usize = 1024;
const MAX_AXIS: u32 = 2048;
const MAX_PATH_BYTES: usize = 512;
const UNSUPPORTED_FLAGS: u32 = 0x2801_c88d;
pub(super) const POLICY: &str = "Static opaque single-base LightmappedGeneric, UnlitGeneric or VertexLitGeneric; identity base transform, no proxies/fallback blocks, detail/second layers or substituted editor image. One 2D LDR color frame, axes 1–2048; point/clamp/border, normal/SSBump, procedural and render-target textures are unsupported. Base pixels use the current renderer's repeat/linear sRGB sampling and fixed editor shading. Normal maps, authored lights, material effects and native pixel parity are excluded. Failed Textured requests return no image; choose Flat explicitly to omit textures.";

#[derive(Clone)]
pub(super) struct Identity {
    path: String,
    origin: AssetOrigin,
    digest: [u8; 32],
    bytes: usize,
}

#[derive(Default)]
struct Reader {
    identities: BTreeMap<String, Identity>,
    bytes: usize,
}

#[derive(Clone, Default)]
pub(super) struct Assets {
    identities: Vec<Identity>,
    guard: Option<compiler::asset_sources::AssetSourceGuard>,
}

impl Reader {
    fn read(&mut self, game: &GameFs, path: &str, maximum: usize) -> Result<Vec<u8>, String> {
        if path.len() > MAX_PATH_BYTES {
            return Err("Textured prefab thumbnail asset path exceeds 512 bytes".into());
        }
        let path = asset_relative(path).map_err(|error| error.to_string())?;
        if !self.identities.contains_key(&path) && self.identities.len() >= MAX_ASSETS {
            return Err("Textured prefab thumbnail exceeds 256 material/texture sources".into());
        }
        let maximum = maximum.min(MAX_READ_BYTES.saturating_sub(self.bytes));
        let asset = game
            .read_asset_limited(&path, maximum)
            .map_err(|error| format!("Textured prefab thumbnail {path}: {error}"))?
            .ok_or_else(|| format!("Textured prefab thumbnail asset is missing: {path}"))?;
        self.bytes += asset.data.len();
        let identity = Identity {
            path: path.clone(),
            origin: asset.origin,
            digest: Sha256::digest(&asset.data).into(),
            bytes: asset.data.len(),
        };
        if let Some(previous) = self.identities.get(&path)
            && (previous.origin != identity.origin || previous.digest != identity.digest)
        {
            return Err(format!(
                "Textured prefab thumbnail asset changed while loading: {path}"
            ));
        }
        self.identities.insert(path, identity);
        Ok(asset.data)
    }
}

pub(super) fn limits() -> Value {
    json!({"material_bytes_per_file":MAX_MATERIAL_BYTES,"texture_bytes_per_file":MAX_TEXTURE_BYTES,
        "read_bytes_per_pass":MAX_READ_BYTES,"source_guard_read_bytes":MAX_READ_BYTES+MAX_TEXTURE_BYTES,"decoded_mip_bytes":MAX_DECODE_BYTES,"asset_sources":MAX_ASSETS,"physical_source_paths":MAX_PHYSICAL_PATHS,"texture_axis":MAX_AXIS,
        "policy":POLICY})
}

pub(super) fn verify(game: &GameFs, assets: &Assets) -> Result<(), String> {
    if let Some(guard) = &assets.guard {
        guard.verify_metadata().map_err(|error|format!("Textured prefab thumbnail source changed: {error}. Refresh thumbnail to load current assets."))?;
    }
    let mut reader = Reader::default();
    for identity in &assets.identities {
        let bytes = reader
            .read(game, &identity.path, identity.bytes)
            .map_err(|error| format!("{error}. Refresh thumbnail to load current assets."))?;
        let current = &reader.identities[&identity.path];
        if bytes.len() != identity.bytes
            || current.origin != identity.origin
            || current.digest != identity.digest
        {
            return Err(format!(
                "Textured prefab thumbnail source changed: {}. Refresh thumbnail to load current assets.",
                identity.path
            ));
        }
    }
    if let Some(guard) = &assets.guard {
        guard.verify_metadata().map_err(|error|format!("Textured prefab thumbnail source changed: {error}. Refresh thumbnail to load current assets."))?;
    }
    Ok(())
}

fn base_path(material: &Material) -> Result<String, String> {
    if !matches!(
        material.shader.as_str(),
        "lightmappedgeneric" | "unlitgeneric" | "vertexlitgeneric"
    ) || material.is_tool()
        || !material.blocks.is_empty()
        || [
            "$basetexture2",
            "$detail",
            "$color",
            "$color2",
            "$selfillum",
            "$additive",
        ]
        .iter()
        .any(|key| material.get(key).is_some())
        || material.alpha_test()
        || material.translucent()
    {
        return Err("Textured prefab thumbnails require a supported opaque single-base material without tint, proxies, detail or second layers".into());
    }
    if texture_transform::<f64>(
        material.get("$basetexturetransform"),
        DuplicateOperations::Reject,
    )
    .map_err(|error| error.to_string())?
        != [1.0, 1.0, 0.0, 0.0]
    {
        return Err("Textured prefab thumbnails require an identity base texture transform".into());
    }
    let base = material
        .get("$basetexture")
        .filter(|value| !value.trim().is_empty())
        .ok_or("Textured prefab thumbnail material needs an actual $basetexture")?;
    let base = asset_relative(&texture_path(base)).map_err(|error| error.to_string())?;
    if let Some(display) = material
        .get("%tooltexture")
        .filter(|value| !value.trim().is_empty())
        && asset_relative(&texture_path(display)).map_err(|error| error.to_string())? != base
    {
        return Err(
            "Textured prefab thumbnail %tooltexture differs from its actual $basetexture".into(),
        );
    }
    Ok(base)
}

pub(super) fn validate_texture(texture: &Vtf, decoded: &mut usize) -> Result<(), String> {
    if texture.width > MAX_AXIS
        || texture.height > MAX_AXIS
        || texture.authored_frames != 1
        || texture.frames != 1
        || texture.faces != 1
        || texture.depth != 1
        || texture.authored_depth.is_some_and(|depth| depth != 1)
        || texture.flags & UNSUPPORTED_FLAGS != 0
        || !matches!(
            texture.format,
            ImageFormat::Rgba8888
                | ImageFormat::Bgra8888
                | ImageFormat::Rgb888
                | ImageFormat::Bgr888
                | ImageFormat::Dxt1
                | ImageFormat::Dxt3
                | ImageFormat::Dxt5
        )
    {
        return Err("Textured prefab thumbnail needs a supported static 2D LDR color texture, at most 2048 pixels per axis".into());
    }
    for mip in 0..texture.mip_count {
        let (width, height) = texture.mip_size(mip);
        let bytes = width as usize * height as usize * 4;
        *decoded = decoded
            .checked_add(bytes)
            .filter(|total| *total <= MAX_DECODE_BYTES)
            .ok_or("Textured prefab thumbnail exceeds 64 MiB decoded mip bytes")?;
        let image = texture
            .decode_rgba8(mip, 0, 0)
            .map_err(|error| error.to_string())?;
        if image
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] != 255)
        {
            return Err("Textured prefab thumbnails currently require opaque base pixels".into());
        }
    }
    Ok(())
}

pub(super) fn load(
    game: &GameFs,
    names: BTreeSet<String>,
) -> Result<(MaterialLibrary, Assets, Value), String> {
    let mut materials = MaterialLibrary::default();
    let mut reader = Reader::default();
    let mut decoded = 0;
    let mut bindings = Vec::with_capacity(names.len());
    for name in names {
        if name.len() > MAX_PATH_BYTES || name.starts_with("__editor/") {
            return Err(
                "Textured prefab thumbnail material name is invalid or exceeds 512 bytes".into(),
            );
        }
        let path = asset_relative(&material_path(&name)).map_err(|error| error.to_string())?;
        let material = Material::load_with_reader(&path, &mut |path| {
            reader
                .read(game, path, MAX_MATERIAL_BYTES)
                .map(Some)
                .map_err(std::io::Error::other)
        })
        .map_err(|error| format!("Textured prefab thumbnail material {name}: {error}"))?
        .ok_or_else(|| format!("Textured prefab thumbnail material is missing: {name}"))?;
        let path = base_path(&material)
            .map_err(|error| format!("Textured prefab thumbnail material {name}: {error}"))?;
        let texture = Vtf::parse(reader.read(game, &path, MAX_TEXTURE_BYTES)?)
            .map_err(|error| format!("Textured prefab thumbnail {path}: {error}"))?;
        validate_texture(&texture, &mut decoded)
            .map_err(|error| format!("Textured prefab thumbnail {path}: {error}"))?;
        bindings.push(
            json!({"material":name,"texture":path,"width":texture.width,"height":texture.height}),
        );
        materials.insert_base_texture(&name, &material, texture, path);
    }
    let sources = reader.identities.values().map(|identity|json!({"path":identity.path,"bytes":identity.bytes,
        "sha256":identity.digest.iter().map(|byte|format!("{byte:02x}")).collect::<String>(),"source":render::icons::Source::from(identity.origin.clone())})).collect::<Vec<_>>();
    let report = json!({"bindings":bindings,"sources":sources,"read_bytes":reader.bytes,"decoded_mip_bytes":decoded});
    let sources = reader
        .identities
        .values()
        .map(|identity| {
            (
                identity.path.as_str(),
                identity
                    .digest
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
            )
        })
        .collect::<Vec<_>>();
    let guard = compiler::asset_sources::capture_asset_source_guard(
        sources
            .iter()
            .map(|(path, digest)| (*path, digest.as_str())),
        [],
        game,
        &compiler::Control::default(),
        compiler::asset_sources::SourceGuardPolicy {
            label: "Textured prefab thumbnail",
            missing_label: "Textured prefab thumbnail asset",
            asset_bytes: MAX_TEXTURE_BYTES,
            physical_paths: MAX_PHYSICAL_PATHS,
        },
    )
    .map_err(|error| error.to_string())?;
    drop(sources);
    let assets = Assets {
        identities: reader.identities.into_values().collect(),
        guard: Some(guard),
    };
    Ok((materials, assets, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_read_budget_bounds_the_actual_mounted_read_before_retention() {
        let (_directory, game) = crate::test_assets::game();
        let mut reader = Reader {
            bytes: MAX_READ_BYTES - 4,
            ..Default::default()
        };
        assert!(
            reader
                .read(&game, "materials/test/checker.vtf", MAX_TEXTURE_BYTES)
                .is_err()
        );
        assert_eq!(reader.bytes, MAX_READ_BYTES - 4);
        assert!(reader.identities.is_empty());
    }
}
