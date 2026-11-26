use source_formats::gamefs::{GameFs, asset_relative};
use source_formats::vmt::{DEFAULT_DETAIL_SCALE, Material, texture_path};
use source_formats::vtf::{FLAG_NOMIP, Vtf};

use super::validate_ordinary_color_texture;

pub(crate) const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_ASSET_BYTES: usize = 8 * 1024 * 1024;
const MAX_AXIS: u32 = 1024;
const MAX_PATH_BYTES: usize = 259;

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
    pub(crate) scale: f32,
    pub(crate) tint: [f32; 3],
    pub(crate) factor: f32,
}

#[derive(Debug)]
pub(crate) struct Asset {
    pub(crate) texture: Vtf,
    pub(crate) parameters: Declaration,
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
            Ok(parameters) => Self::Declared(parameters),
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
    material
        .params
        .iter()
        .any(|(key, _)| key.starts_with("$detail"))
}

impl Declaration {
    fn new(
        material: &Material,
        base: Option<&Vtf>,
        display_texture: Option<&str>,
    ) -> Result<Self, String> {
        if material.shader != "lightmappedgeneric" || material.is_tool() {
            return Err("Detail layers require a non-tool LightmappedGeneric surface".into());
        }
        if !material.blocks.is_empty() {
            return Err("Detail layers exclude material proxies and fallback blocks".into());
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
                | "%detailtype"
                | "$detail"
                | "$detailscale"
                | "$detailblendfactor"
                | "$detailblendmode"
                | "$detailtint" => {}
                "$frame" | "$detailframe" | "$alphatest" | "$translucent" | "$additive"
                | "$selfillum" | "$vertexalpha" | "$vertexcolor"
                    if value.trim().parse::<i32>() == Ok(0) => {}
                "$alpha" if value.trim().parse::<f32>() == Ok(1.0) => {}
                _ => return Err(format!("Detail layers exclude material parameter {key}")),
            }
        }
        if let Some(mode) = material.get("$detailblendmode")
            && mode.trim().parse::<i32>() != Ok(0)
        {
            return Err("Detail layers require $detailblendmode 0".into());
        }
        if !super::uv::is_axis_identity(material.get("$basetexturetransform")) {
            return Err("Detail layers require an identity base texture transform".into());
        }
        let base_path = texture_reference(material, "$basetexture")?;
        let base = base
            .filter(|_| display_texture == Some(base_path.as_str()))
            .ok_or("Detail layers require the exact declared base display texture")?;
        validate_texture(base, u32::from(u16::MAX))
            .map_err(|error| format!("{base_path}: {error}"))?;
        let path = texture_reference(material, "$detail")?;
        let scale = scalar(
            material,
            "$detailscale",
            DEFAULT_DETAIL_SCALE,
            1.0 / 1024.0..=1024.0,
        )?;
        let factor = scalar(material, "$detailblendfactor", 1.0, 0.0..=1.0)?;
        let tint = material.get("$detailtint").map_or(Ok([1.0; 3]), |value| {
            super::emission::tint(value, "$detailtint")
        })?;
        Ok(Self {
            path,
            scale,
            tint,
            factor,
        })
    }

    pub(super) fn load(self, fs: &GameFs, available_bytes: usize) -> Layer {
        if available_bytes == 0 {
            return Layer::Unavailable(cache_limit());
        }
        let texture =
            match super::read_vtf(fs, &self.path, Some(MAX_ASSET_BYTES.min(available_bytes))) {
                Ok(Some(texture)) => texture,
                Ok(None) => {
                    return Layer::Unavailable(format!(
                        "Detail texture {} was not found",
                        self.path
                    ));
                }
                Err(error) => return Layer::Unavailable(format!("Detail texture {error}")),
            };
        if let Err(error) = validate_texture(&texture, MAX_AXIS) {
            return Layer::Unavailable(format!("Detail texture {}: {error}", self.path));
        }
        let asset = Asset {
            texture,
            parameters: self,
        };
        if asset.cache_bytes() > available_bytes {
            return Layer::Unavailable(cache_limit());
        }
        Layer::Ready(Box::new(asset))
    }
}

impl Asset {
    pub(super) fn cache_bytes(&self) -> usize {
        let gpu_bytes = (0..self.texture.mip_count)
            .map(|mip| {
                let (width, height) = self.texture.mip_size(mip);
                width as usize * height as usize * 4
            })
            .sum::<usize>();
        self.texture.encoded_size() + gpu_bytes
    }

    pub(crate) fn maximum_gain(&self) -> [f64; 3] {
        let factor = f64::from(self.parameters.factor);
        self.parameters
            .tint
            .map(|tint| 1.0 + (2.0 * f64::from(tint) - 1.0) * factor)
    }
}

fn texture_reference(material: &Material, key: &str) -> Result<String, String> {
    material
        .get(key)
        .map(str::trim)
        .filter(|name| !name.is_empty() && asset_relative(name).is_ok())
        .map(texture_path)
        .filter(|path| path.len() <= MAX_PATH_BYTES && asset_relative(path).is_ok())
        .ok_or_else(|| format!("Detail layers require a bounded mounted {key} texture path"))
}

fn scalar(
    material: &Material,
    key: &str,
    default: f32,
    range: std::ops::RangeInclusive<f32>,
) -> Result<f32, String> {
    match material.get(key) {
        None => Ok(default),
        Some(value) => value
            .trim()
            .parse()
            .ok()
            .filter(|value| range.contains(value))
            .ok_or_else(|| {
                format!(
                    "{key} requires a finite scalar in {}..={}",
                    range.start(),
                    range.end()
                )
            }),
    }
}

fn validate_texture(texture: &Vtf, maximum_axis: u32) -> Result<(), String> {
    validate_ordinary_color_texture(texture, maximum_axis)?;
    if texture.flags & FLAG_NOMIP != 0 && texture.mip_count != 1 {
        return Err("Detail layers require one mip with the No mip flag".into());
    }
    texture
        .validate_native_mips()
        .map_err(|error| error.to_string())
}

fn cache_limit() -> String {
    "Detail texture payload exceeds the 32 MiB cache budget".into()
}
