use source_formats::gamefs::{GameFs, asset_relative};
use source_formats::vmt::{DuplicateOperations, Material, texture_path, texture_transform};
use source_formats::vtf::{FLAG_NOMIP, Vtf};

use super::validate_ordinary_color_texture;

const MAX_ASSETS: usize = 64;
const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_AXIS: u32 = 512;
const MAX_PATH_BYTES: usize = 259;
const IDENTITY_TRANSFORM: [f32; 4] = [1.0, 1.0, 0.0, 0.0];

// Trilinear, clamp, DXT5 hint, legacy color, mip/LOD, alpha and cube flags.
const ALLOWED_FLAGS: u32 = 0x0200_776e;

#[derive(Debug)]
pub(crate) enum Layer {
    Absent,
    Declared(Declaration),
    Ready(Box<Asset>),
    Unavailable(String),
}

#[derive(Debug)]
pub(crate) struct Declaration {
    path: String,
    tint: [f32; 3],
    fresnel: f32,
}

#[derive(Debug)]
pub(crate) struct Asset {
    pub(crate) texture: Vtf,
    declaration: Declaration,
    bytes: usize,
}

impl Asset {
    pub(crate) fn uniform(&self) -> [f32; 4] {
        let [r, g, b] = self.declaration.tint;
        [r, g, b, self.declaration.fresnel]
    }

    pub(crate) fn radiance_bound(&self) -> [f64; 3] {
        self.declaration.tint.map(f64::from)
    }
}

impl Layer {
    pub(super) fn resolve(
        material: &Material,
        base: Option<&Vtf>,
        display_texture: Option<&str>,
    ) -> Self {
        if !requested(material) {
            return Self::Absent;
        }
        match Declaration::new(material, base, display_texture) {
            Ok(declaration) => Self::Declared(declaration),
            Err(error) => Self::Unavailable(error),
        }
    }

    pub(crate) fn ready(&self) -> Option<&Asset> {
        match self {
            Self::Ready(asset) => Some(asset),
            _ => None,
        }
    }
}

pub(super) fn requested(material: &Material) -> bool {
    material.get("$envmap").is_some()
}

#[derive(Debug, Default)]
pub(super) struct Budget {
    assets: usize,
    bytes: usize,
}

impl Budget {
    pub(super) fn load(&mut self, fs: &GameFs, declaration: Declaration) -> Layer {
        if self.assets == MAX_ASSETS || self.bytes == MAX_CACHE_BYTES {
            return Layer::Unavailable(cache_limit());
        }
        match declaration.load(fs, MAX_CACHE_BYTES - self.bytes) {
            Ok(asset) => {
                self.assets += 1;
                self.bytes += asset.bytes;
                Layer::Ready(Box::new(asset))
            }
            Err(error) => Layer::Unavailable(error),
        }
    }

    pub(super) fn release(&mut self, asset: &Asset) {
        self.assets -= 1;
        self.bytes -= asset.bytes;
    }
}

impl Declaration {
    fn new(
        material: &Material,
        base: Option<&Vtf>,
        display_texture: Option<&str>,
    ) -> Result<Self, String> {
        let path = texture_reference(material, "$envmap")?;
        if path == "materials/env_cubemap.vtf" {
            return Err(
                "Environment preview requires a named cube VTF for runtime env_cubemap materials"
                    .into(),
            );
        }
        if path
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with("_rt_"))
        {
            return Err("Environment preview excludes live render targets".into());
        }
        if material.shader != "lightmappedgeneric"
            || material.is_tool()
            || material.translucent()
            || material.alpha_test()
        {
            return Err("Environment preview requires opaque non-tool LightmappedGeneric".into());
        }
        if !material.blocks.is_empty() {
            return Err("Environment preview excludes material proxies and fallback blocks".into());
        }
        for (key, value) in &material.params {
            match key.as_str() {
                "$basetexture"
                | "$basetexturetransform"
                | "$surfaceprop"
                | "$surfaceprop2"
                | "$reflectivity"
                | "$nodecal"
                | "%keywords"
                | "$envmap"
                | "$envmaptint"
                | "$fresnelreflection" => {}
                "$envmapframe" | "$frame" | "$alphatest" | "$translucent"
                    if value.trim().parse::<i32>() == Ok(0) => {}
                "$envmapcontrast" if value.trim().parse::<f32>() == Ok(0.0) => {}
                "$envmapsaturation" if value.trim().parse::<f32>() == Ok(1.0) => {}
                _ => {
                    let key: String = key.chars().take(128).collect();
                    return Err(format!(
                        "Environment preview excludes material parameter {key}"
                    ));
                }
            }
        }
        if texture_transform::<f32>(
            material.get("$basetexturetransform"),
            DuplicateOperations::Reject,
        ) != Ok(IDENTITY_TRANSFORM)
        {
            return Err("Environment preview requires an identity base texture transform".into());
        }
        let base_path = texture_reference(material, "$basetexture")?;
        let base = base
            .filter(|_| display_texture == Some(base_path.as_str()))
            .ok_or("Environment preview requires the exact declared base display texture")?;
        validate_ordinary_color_texture(base, u32::from(u16::MAX))
            .map_err(|error| format!("{base_path}: {error}"))?;
        let tint = material.get("$envmaptint").map_or(Ok([1.0; 3]), |value| {
            super::emission::tint(value, "$envmaptint")
        })?;
        let fresnel = material
            .get("$fresnelreflection")
            .map_or(Ok(1.0), |value| value.trim().parse::<f32>())
            .ok()
            .filter(|value| (0.0..=1.0).contains(value))
            .ok_or("$fresnelreflection requires a finite scalar in 0..=1")?;
        Ok(Self {
            path,
            tint,
            fresnel,
        })
    }

    fn load(self, fs: &GameFs, available: usize) -> Result<Asset, String> {
        let texture = super::read_vtf(fs, &self.path, Some(MAX_SOURCE_BYTES.min(available)))?
            .ok_or_else(|| format!("Environment cube {} was not found", self.path))?;
        validate_cube(&texture).map_err(|error| format!("{}: {error}", self.path))?;
        let gpu_bytes: usize = (0..texture.mip_count)
            .map(|mip| {
                let (w, h) = texture.mip_size(mip);
                w as usize * h as usize * 6 * 4
            })
            .sum();
        let bytes = texture.encoded_size() + gpu_bytes;
        if bytes > available {
            return Err(cache_limit());
        }
        Ok(Asset {
            texture,
            declaration: self,
            bytes,
        })
    }
}

fn texture_reference(material: &Material, key: &str) -> Result<String, String> {
    material
        .get(key)
        .map(str::trim)
        .filter(|name| !name.is_empty() && asset_relative(name).is_ok())
        .map(texture_path)
        .filter(|path| path.len() <= MAX_PATH_BYTES && asset_relative(path).is_ok())
        .ok_or_else(|| format!("Environment preview requires a bounded mounted {key} texture path"))
}

fn validate_cube(texture: &Vtf) -> Result<(), String> {
    if !matches!(texture.faces, 6 | 7)
        || texture.frames != 1
        || texture.authored_frames != 1
        || !matches!(texture.first_frame, 0 | u16::MAX)
        || texture.depth != 1
        || texture.authored_depth == Some(0)
        || texture.width != texture.height
        || !texture.width.is_power_of_two()
        || texture.width > MAX_AXIS
    {
        return Err(
            "Environment preview needs a static square power-of-two cubemap with axes 1–512".into(),
        );
    }
    if texture.flags & !ALLOWED_FLAGS != 0
        || !crate::normal_maps::ordinary_ldr_format(texture.format)
    {
        return Err("Environment preview requires an ordinary LDR cubemap without special sampling, procedural or render-target flags".into());
    }
    if texture.flags & FLAG_NOMIP != 0 && texture.mip_count != 1 {
        return Err("Environment cubes with the No mip flag require exactly one mip".into());
    }
    texture
        .validate_native_mips()
        .map_err(|error| error.to_string())
}

fn cache_limit() -> String {
    "Environment preview exceeds its 64-cube or 32 MiB cache limit; reload material assets to retry"
        .into()
}
