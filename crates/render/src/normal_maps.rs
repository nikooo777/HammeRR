use std::collections::BTreeMap;
use std::sync::Arc;

use glam::DVec3;
use serde::Serialize;
use source_formats::gamefs::{GameFs, asset_relative};
use source_formats::vmf::Side;
use source_formats::vmt::{DuplicateOperations, Material, texture_path, texture_transform};
use source_formats::vtf::{ImageFormat, RgbaImage, Vtf};

pub const MAX_ASSETS: usize = 128;
pub const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_AXIS: u32 = 1024;
const MAX_MATERIAL_BYTES: usize = 64 * 1024;
const MAX_TEXTURE_BYTES: usize = 16 * 1024 * 1024;
const SSBUMP_FLAG: u32 = 0x0800_0000;
const UNSUPPORTED_TEXTURE_FLAGS: u32 = 0x2201_c84d;

pub(crate) fn ordinary_ldr_format(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Rgb888
            | ImageFormat::Bgr888
            | ImageFormat::Rgba8888
            | ImageFormat::Bgra8888
            | ImageFormat::Dxt1
            | ImageFormat::Dxt3
            | ImageFormat::Dxt5
    )
}

pub(crate) fn validate_static_texture(texture: &Vtf, maximum_axis: u32) -> Result<(), String> {
    if texture.width > maximum_axis
        || texture.height > maximum_axis
        || texture.frames != 1
        || texture.faces != 1
        || texture.depth != 1
        || texture.authored_frames == 0
        || texture.authored_depth == Some(0)
    {
        return Err(format!(
            "Static texture requires one frame, face and depth slice with axes 1–{maximum_axis}"
        ));
    }
    Ok(())
}
pub const LIMITS: &str = "Flat LightmappedGeneric brush polygons only; terrain, models, sprites, icons and helpers are excluded. Static single-frame/face/depth RGB/BGR/RGBA/BGRA8888 or DXT1/3/5 tangent-space normals, axes 1–1024. RGB is linear, alpha is ignored; normal images repeat with linear filtering at their base mip. Clamp/border/point-sample, sRGB, procedural and render-target VTF flags are unsupported. RGB normal Z must be at least 128, preventing degenerate interpolated normals; signed/swizzled, HDR and SSBump textures are unsupported. One bump map; no proxies, fallback blocks, detail/second normals, animated frames or base display overrides. Base texture transform must be identity. Bump transform supports positive axis scale and translation with zero rotation. Authored face UV rotation and signed scales define the tangent frame. Fixed editor directional shading, or admitted unshadowed authored lights in Detail quality; no specular or native game parity. 128 cached admissions and 64 MiB decoded bytes; reload materials clears successes/failures. Disabled and non-Textured views perform no normal admission.";

#[derive(Serialize)]
pub struct Limits {
    pub assets: usize,
    pub decoded_bytes: usize,
    pub texture_bytes: usize,
    pub material_bytes: usize,
    pub axis: u32,
    pub policy: &'static str,
}

pub fn limits() -> Limits {
    Limits {
        assets: MAX_ASSETS,
        decoded_bytes: MAX_CACHE_BYTES,
        texture_bytes: MAX_TEXTURE_BYTES,
        material_bytes: MAX_MATERIAL_BYTES,
        axis: MAX_AXIS,
        policy: LIMITS,
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AssetReport {
    pub material: String,
    pub state: &'static str,
    pub texture: Option<String>,
    pub source: Option<crate::icons::Source>,
    pub texture_source: Option<crate::icons::Source>,
    pub dimensions: Option<[u32; 2]>,
    pub transform: [f32; 4],
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct Asset {
    pub retained: bool,
    pub report: AssetReport,
    pub image: Option<RgbaImage>,
}

#[derive(Debug, Default)]
pub struct Library {
    entries: BTreeMap<String, Arc<Asset>>,
    bytes: usize,
}

impl Library {
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub fn load(&mut self, fs: &GameFs, name: &str) -> Arc<Asset> {
        self.load_matching(fs, name, None)
    }

    pub fn load_matching(&mut self, fs: &GameFs, name: &str, expected: Option<u64>) -> Arc<Asset> {
        if let Some(asset) = self.entries.get(name) {
            return asset.clone();
        }
        let mut report = AssetReport {
            material: name.chars().take(256).collect(),
            state: "unsupported",
            texture: None,
            source: None,
            texture_source: None,
            dimensions: None,
            transform: [1.0, 1.0, 0.0, 0.0],
            error: None,
        };
        let retained = name.len() <= 256 && self.entries.len() < MAX_ASSETS;
        let image = if retained {
            match load(
                fs,
                name,
                MAX_CACHE_BYTES.saturating_sub(self.bytes),
                expected,
                &mut report,
            ) {
                Ok(image) => {
                    report.state = if image.is_some() { "ready" } else { "absent" };
                    image
                }
                Err(error) => {
                    report.error = Some(error.chars().take(512).collect());
                    None
                }
            }
        } else {
            report.error = Some(
                if name.len() > 256 {
                    "Normal-map material path exceeds 256 bytes"
                } else {
                    "Normal-map admission cache reached 128 entries; reload material assets to retry"
                }
                .into(),
            );
            None
        };
        let asset = Arc::new(Asset {
            retained,
            report,
            image,
        });
        if retained {
            self.bytes += asset.image.as_ref().map_or(0, |image| image.pixels.len());
            self.entries.insert(name.into(), asset.clone());
        }
        asset
    }
}

fn transform(value: Option<&str>) -> Result<[f32; 4], String> {
    let transform = texture_transform::<f32>(value, DuplicateOperations::Reject)
        .map_err(|_| "Normal preview requires a finite bounded axis-scale/translation transform with zero rotation and no duplicate operations".to_owned())?;
    if transform[..2].iter().any(|value| *value < 1.0 / 1024.0) {
        return Err(
            "Normal preview requires positive bump transform scales from 1/1024 to 1024".into(),
        );
    }
    Ok(transform)
}

fn load(
    fs: &GameFs,
    name: &str,
    budget: usize,
    expected: Option<u64>,
    report: &mut AssetReport,
) -> Result<Option<RgbaImage>, String> {
    if name.len() > 256 {
        return Err("Normal-map material path exceeds 256 bytes".into());
    }
    let name = asset_relative(name).map_err(|error| error.to_string())?;
    let material =
        Material::load_with_reader(&source_formats::vmt::material_path(&name), &mut |path| {
            let asset = fs.read_asset_limited(path, MAX_MATERIAL_BYTES)?;
            if report.source.is_none() {
                report.source = asset.as_ref().map(|asset| asset.origin.clone().into());
            }
            Ok(asset.map(|asset| asset.data))
        })
        .map_err(|error| error.to_string())?
        .ok_or("Normal-preview material was not found")?;
    if expected.is_some_and(|expected| expected != crate::materials::material_identity(&material)) {
        return Err("Material settings changed since its base display was cached; Reload material assets before normal preview".into());
    }
    if !material.params.iter().any(|(key, _)| {
        key.starts_with("$bump") || key.starts_with("$normalmap") || key.starts_with("$ssbump")
    }) {
        return Ok(None);
    }
    if material.shader != "lightmappedgeneric" {
        return Err(format!(
            "Normal preview does not support shader {}",
            material.shader
        ));
    }
    if !material.blocks.is_empty() {
        return Err(
            "Normal preview does not support material proxies or nested fallback/effect blocks"
                .into(),
        );
    }
    for (key, _) in &material.params {
        if (key.starts_with("$bump") && !["$bumpmap", "$bumptransform"].contains(&key.as_str()))
            || key.starts_with("$ssbump")
            || key.starts_with("$detail")
            || [
                "$normalmap",
                "$normalmap2",
                "$basetexture2",
                "%tooltexture",
                "$frame",
                "$forcebump",
            ]
            .contains(&key.as_str())
        {
            return Err(format!(
                "Normal preview does not support material parameter {key}"
            ));
        }
    }
    let bump = material
        .get("$bumpmap")
        .filter(|value| !value.trim().is_empty())
        .ok_or("Normal preview requires one nonempty $bumpmap")?;
    if material
        .get("$basetexture")
        .is_none_or(|value| value.trim().is_empty())
    {
        return Err("Normal preview requires an explicit base texture".into());
    }
    if transform(material.get("$basetexturetransform"))? != [1.0, 1.0, 0.0, 0.0] {
        return Err("Normal preview requires an identity base texture transform".into());
    }
    report.transform = transform(material.get("$bumptransform"))?;
    let path = asset_relative(&texture_path(bump)).map_err(|error| error.to_string())?;
    if path.len() > 256 || bump.trim().is_empty() {
        return Err("Normal texture requires a bounded nonempty mounted VTF path".into());
    }
    report.texture = Some(path.clone());
    let asset = fs
        .read_asset_limited(&path, MAX_TEXTURE_BYTES)
        .map_err(|error| error.to_string())?
        .ok_or("Normal texture was not found")?;
    report.texture_source = Some(asset.origin.into());
    let texture = Vtf::parse(asset.data).map_err(|error| error.to_string())?;
    validate_static_texture(&texture, MAX_AXIS).map_err(|_| {
        "Normal preview requires a static single-frame/face/depth VTF with axes 1–1024".to_owned()
    })?;
    if texture.flags & SSBUMP_FLAG != 0 || !ordinary_ldr_format(texture.format) {
        return Err("Normal preview requires ordinary RGB tangent-space LDR normals; signed/swizzled, SSBump and HDR encodings are unsupported".into());
    }
    if texture.flags & UNSUPPORTED_TEXTURE_FLAGS != 0 {
        return Err("Normal preview does not support clamp/border/point-sample, sRGB, procedural, environment or render-target VTF flags".into());
    }
    let bytes = texture.width as usize * texture.height as usize * 4;
    if bytes > budget {
        return Err("Normal-map decoded cache exceeds 64 MiB; reload materials to retry".into());
    }
    let image = texture
        .decode_rgba8(0, 0, 0)
        .map_err(|error| error.to_string())?;
    if image
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .any(|pixel| pixel[2] < 128)
    {
        return Err("Normal preview requires RGB normal Z≥128; lower-hemisphere or ambiguous normal encodings are unsupported".into());
    }
    report.dimensions = Some([image.width, image.height]);
    Ok(Some(image))
}

pub fn tangent(
    side: &Side,
    normal: DVec3,
    points: &[DVec3],
    dimensions: [u32; 2],
    transform: [f32; 4],
) -> Result<[f32; 4], String> {
    let s = side.u_axis.axis / side.u_axis.scale;
    let t = side.v_axis.axis / side.v_axis.scale;
    let orientation = normal.dot(s.cross(t));
    if !s.is_finite() || !t.is_finite() || !orientation.is_finite() || orientation.abs() < 1e-8 {
        return Err(
            "Normal preview requires a finite nonsingular authored face UV projection".into(),
        );
    }
    for &point in points {
        let uv =
            map_core::texel_uv(side, point) / glam::DVec2::from_array(dimensions.map(f64::from));
        let bumped = uv * glam::DVec2::new(f64::from(transform[0]), f64::from(transform[1]))
            + glam::DVec2::new(f64::from(transform[2]), f64::from(transform[3]));
        if !uv.is_finite()
            || !bumped.is_finite()
            || uv.abs().max_element().max(bumped.abs().max_element()) > 1_000_000.0
        {
            return Err("Normal-preview UV coordinates must fit finite ±1000000".into());
        }
    }
    let (tangent, _, _) = map_core::surface::texture_tangents(normal, s, orientation < 0.0)?;
    Ok([
        tangent.x as f32,
        tangent.y as f32,
        tangent.z as f32,
        if orientation < 0.0 { -1.0 } else { 1.0 },
    ])
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub requested: bool,
    pub active: bool,
    pub applied_faces: usize,
    pub rejected_faces: usize,
    pub materials: Vec<AssetReport>,
    pub errors: Vec<String>,
    pub errors_truncated: bool,
    pub materials_truncated: bool,
}

impl Report {
    pub fn summary(&self) -> String {
        if !self.requested {
            return "Normal map preview is off for this view".into();
        }
        if !self.active {
            return "Normal map preview requires Textured view; no normal assets were read".into();
        }
        let mut summary = format!(
            "Normal maps · {} brush polygons · {} unavailable",
            self.applied_faces, self.rejected_faces
        );
        for error in self.errors.iter().take(3) {
            summary.push_str(&format!("\n{error}"));
        }
        summary
    }
}
