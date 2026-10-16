use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_HDR_PAYLOAD: AtomicU64 = AtomicU64::new(1);

use source_formats::gamefs::GameFs;
use source_formats::vmt::{Material, texture_path};
use source_formats::vtf::Vtf;

mod hdr;
pub use hdr::{hdr_mip_bytes, validate_hdr_texture};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    #[default]
    Ldr,
    Hdr,
}
impl Channel {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ldr => "ldr",
            Self::Hdr => "hdr",
        }
    }
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ldr" => Ok(Self::Ldr),
            "hdr" => Ok(Self::Hdr),
            _ => Err("Sky channel must be ldr or hdr".into()),
        }
    }
}

pub const HDR_POLICY: &str = "HammeRR sky-only preview: linear RGBA16F RGB, or RGBA16 UNORM RGB ×16; finite nonnegative radiance, opaque output; exposure 2^EV and per-channel Reinhard x/(1+x), then one sRGB target encoding. No game hardware mode, native tonemap/exposure adaptation, baked lighting, fog or material tint. Six direct static Sky materials only; no compressed HDR, Patch or LDR substitution. Source bounds: 128 lookups, 64 MiB aggregate, 1 MiB VMT, 16 MiB VTF, 4096 pixels/axis. One cached HDR family, 64 MiB encoded and 64 MiB GPU texture bytes per set (replacement can briefly retain old and new sets); temporary CPU source+parse copies at most 128 MiB plus one 16 MiB conversion buffer.";

pub const SUFFIXES: [&str; 6] = ["rt", "bk", "lf", "ft", "up", "dn"];
const MAX_TEXTURE_BYTES: usize = 64 * 1024 * 1024;
const MAX_TEXTURE_SIZE: u32 = 4096;

#[derive(Debug)]
pub struct Face {
    pub hdr_texture: Option<Arc<Vtf>>,
    pub texture_path: Option<String>,
    pub source: Option<crate::icons::Source>,
    pub texture_source: Option<crate::icons::Source>,
    pub material: String,
    pub texture: Option<Vtf>,
    pub issue: Option<String>,
    pub transform: [f32; 4],
}

#[derive(Debug)]
pub struct Skybox {
    pub payload_id: u64,
    pub channel: Channel,
    pub name: String,
    pub faces: [Face; 6],
}

impl Skybox {
    pub fn hdr(name: String, mut faces: [Face; 6]) -> Self {
        let mut checked = std::collections::HashMap::new();
        for face in &mut faces {
            if let Some(texture) = &face.hdr_texture {
                let result = checked
                    .entry(Arc::as_ptr(texture) as usize)
                    .or_insert_with(|| validate_hdr_texture(texture));
                if let Err(error) = result {
                    face.issue = Some(error.clone());
                    face.hdr_texture = None;
                }
            }
        }
        Self {
            payload_id: NEXT_HDR_PAYLOAD.fetch_add(1, Ordering::Relaxed),
            channel: Channel::Hdr,
            name,
            faces,
        }
    }
    pub fn load(fs: &GameFs, name: &str) -> Self {
        let name = name.trim().to_ascii_lowercase();
        Self {
            payload_id: 0,
            channel: Channel::Ldr,
            faces: SUFFIXES.map(|suffix| Face::load(fs, &format!("skybox/{name}{suffix}"))),
            name,
        }
    }

    pub fn ready(&self) -> bool {
        self.faces.iter().all(|face| face.image().is_some())
    }

    pub fn summary(&self) -> String {
        if self.ready() {
            return format!(
                "{} · six {} faces ready",
                self.name,
                self.channel.name().to_ascii_uppercase()
            );
        }
        let issues = self
            .faces
            .iter()
            .zip(SUFFIXES)
            .filter_map(|(face, suffix)| {
                face.issue
                    .as_ref()
                    .map(|issue| format!("{suffix}: {issue}"))
            })
            .collect::<Vec<_>>();
        format!("{} · unavailable ({})", self.name, issues.join("; "))
    }
}

impl Face {
    pub fn image(&self) -> Option<&Vtf> {
        self.texture.as_ref().or(self.hdr_texture.as_deref())
    }
    pub fn hdr(
        texture: Option<Arc<Vtf>>,
        material: String,
        transform: Option<&str>,
        source: Option<crate::icons::Source>,
        texture_path: Option<String>,
        texture_source: Option<crate::icons::Source>,
        mut issue: Option<String>,
    ) -> Self {
        let transform = match texture_transform(transform) {
            Ok(value) => value,
            Err(error) => {
                issue.get_or_insert(error);
                [1.0, 1.0, 0.0, 0.0]
            }
        };
        Self {
            hdr_texture: if issue.is_none() { texture } else { None },
            texture: None,
            material,
            transform,
            source,
            texture_path,
            texture_source,
            issue: issue.map(|error| error.chars().take(1024).collect()),
        }
    }
    fn load(fs: &GameFs, name: &str) -> Self {
        let result = load_texture(fs, name);
        let (texture, issue) = match result {
            Ok((texture, transform)) => {
                return Self {
                    hdr_texture: None,
                    texture_path: None,
                    source: None,
                    texture_source: None,
                    material: name.to_owned(),
                    texture: Some(texture),
                    issue: None,
                    transform,
                };
            }
            Err(issue) => (None, Some(issue)),
        };
        Self {
            hdr_texture: None,
            texture_path: None,
            source: None,
            texture_source: None,
            material: name.to_owned(),
            texture,
            issue,
            transform: [1.0, 1.0, 0.0, 0.0],
        }
    }
}

fn load_texture(fs: &GameFs, name: &str) -> Result<(Vtf, [f32; 4]), String> {
    source_formats::gamefs::asset_relative(name).map_err(|error| error.to_string())?;
    let material = Material::load(fs, name)
        .map_err(|error| error.to_string())?
        .ok_or("missing material")?;
    if !matches!(material.shader.as_str(), "unlitgeneric" | "sky") {
        return Err(format!("unsupported shader {}", material.shader));
    }
    if !material.blocks.is_empty() {
        return Err("nested material effects or animation are unsupported".into());
    }
    for (key, _) in &material.params {
        if !matches!(
            key.as_str(),
            "$basetexture"
                | "$basetexturetransform"
                | "$hdrbasetexture"
                | "$hdrcompressedtexture"
                | "$hdrcompressedtexture0"
                | "$hdrcompressedtexture1"
                | "$hdrcompressedtexture2"
                | "$nofog"
                | "$ignorez"
                | "$surfaceprop"
                | "$surfaceprop2"
                | "$srgb"
                | "%tooltexture"
                | "%keywords"
        ) {
            return Err(format!("unsupported sky material parameter {key}"));
        }
    }
    let transform = texture_transform(material.get("$basetexturetransform"))?;
    let path = material
        .get("$basetexture")
        .filter(|value| !value.trim().is_empty())
        .ok_or("no LDR $basetexture")?;
    let bytes = fs
        .read_limited(&texture_path(path), MAX_TEXTURE_BYTES)
        .map_err(|error| error.to_string())?
        .ok_or("missing LDR texture")?;
    let texture = Vtf::parse(bytes).map_err(|error| error.to_string())?;
    if texture.format == source_formats::vtf::ImageFormat::Rgba16161616F {
        return Err("HDR-only base textures are unsupported".into());
    }
    if texture.frames != 1 || texture.faces != 1 || texture.depth != 1 {
        return Err("animated, cubemap or volume textures are unsupported".into());
    }
    if texture.width > MAX_TEXTURE_SIZE || texture.height > MAX_TEXTURE_SIZE {
        return Err(format!("texture exceeds {MAX_TEXTURE_SIZE} pixels"));
    }
    Ok((texture, transform))
}

fn texture_transform(value: Option<&str>) -> Result<[f32; 4], String> {
    let Some(value) = value else {
        return Ok([1.0, 1.0, 0.0, 0.0]);
    };
    let mut tokens = value.split_whitespace();
    let mut center = [0.5, 0.5];
    let mut scale = [1.0, 1.0];
    let mut translate = [0.0, 0.0];
    while let Some(key) = tokens.next() {
        let count = match key {
            "center" | "scale" | "translate" => 2,
            "rotate" => 1,
            _ => return Err("unsupported texture transform".into()),
        };
        let mut values = [0.0; 2];
        for value in values.iter_mut().take(count) {
            *value = tokens
                .next()
                .ok_or("incomplete texture transform")?
                .parse::<f32>()
                .map_err(|_| "invalid texture transform")?;
            if !value.is_finite() || value.abs() > 1024.0 {
                return Err("invalid texture transform".into());
            }
        }
        match key {
            "center" => center = values,
            "scale" => scale = values,
            "translate" => translate = values,
            "rotate" if values[0] != 0.0 => {
                return Err("rotated texture transforms are unsupported".into());
            }
            _ => {}
        }
    }
    Ok([
        scale[0],
        scale[1],
        center[0] * (1.0 - scale[0]) + translate[0],
        center[1] * (1.0 - scale[1]) + translate[1],
    ])
}

#[cfg(test)]
mod tests;
