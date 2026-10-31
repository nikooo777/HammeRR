use serde::{Deserialize, Serialize};
use source_formats::vtf::RgbaImage;

use super::Session;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    #[default]
    Color,
    Alpha,
}

impl Channel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Color => "Color",
            Self::Alpha => "Alpha",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layer {
    #[default]
    Primary,
    Secondary,
}

impl Layer {
    pub fn from_number(layer: u32) -> Result<Self, String> {
        match layer {
            1 => Ok(Self::Primary),
            2 => Ok(Self::Secondary),
            _ => Err("Choose material image layer 1 or 2".into()),
        }
    }

    pub fn number(self) -> u32 {
        match self {
            Self::Primary => 1,
            Self::Secondary => 2,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Source {
    pub texture: String,
    pub material_identity: String,
    pub cached: bool,
    pub mip: u8,
    pub frame: u32,
    pub face: u32,
}

#[derive(Debug)]
pub struct MaterialPreview {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub missing: bool,
    pub tool: bool,
    pub channel: Channel,
    pub layer: Layer,
    pub source: Option<Source>,
    pub alpha_range: Option<[u8; 2]>,
    pub image: Option<RgbaImage>,
}

impl Session {
    pub fn material_preview_channel(
        &self,
        name: &str,
        channel: Channel,
    ) -> Result<MaterialPreview, String> {
        self.material_preview_layer_channel(name, Layer::Primary, channel)
    }

    pub fn material_preview_layer_channel(
        &self,
        name: &str,
        layer: Layer,
        channel: Channel,
    ) -> Result<MaterialPreview, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("material name must not be empty".into());
        }
        let cached = self.materials.find(name);
        let loaded;
        let material = if let Some(material) = cached {
            material
        } else {
            loaded = render::materials::MaterialInfo::load(&self.game, name);
            &loaded
        };
        if let Some(error) = &material.error {
            return Err(error.clone());
        }
        let (texture, texture_path) = match layer {
            Layer::Primary => (material.base.as_ref(), material.display_texture.as_ref()),
            Layer::Secondary => {
                let path = material.blend_texture.as_ref().ok_or("Layer 2 requires a WorldVertexTransition material with an explicit $basetexture2; the primary image is not a substitute")?;
                let texture = material.blend.as_ref().ok_or("The captured $basetexture2 image is unavailable. Reload material assets after repairing the source; layer 1 is not substituted")?;
                (Some(texture), Some(path))
            }
        };
        let mut image = texture
            .map(|texture| texture.decode_rgba8(texture.mip_for_size(256), 0, 0))
            .transpose()
            .map_err(|error| error.to_string())?;
        let source = texture.and_then(|texture| {
            Some(Source {
                texture: texture_path.cloned()?,
                material_identity: format!("{:016x}", material.source_identity()?),
                cached: cached.is_some(),
                mip: texture.mip_for_size(256),
                frame: 0,
                face: 0,
            })
        });
        let alpha_range = image.as_mut().and_then(|image| {
            let mut range: Option<[u8; 2]> = None;
            for pixel in image.pixels.as_chunks_mut::<4>().0 {
                let alpha = pixel[3];
                range =
                    Some(range.map_or([alpha; 2], |[low, high]| [low.min(alpha), high.max(alpha)]));
                if channel == Channel::Alpha {
                    pixel.copy_from_slice(&[alpha, alpha, alpha, 255]);
                }
            }
            range
        });
        Ok(MaterialPreview {
            name: material.name.clone(),
            width: texture.map_or(material.width, |texture| texture.width),
            height: texture.map_or(material.height, |texture| texture.height),
            missing: match layer {
                Layer::Primary => material.missing,
                Layer::Secondary => false,
            },
            tool: material.tool,
            channel,
            layer,
            source,
            alpha_range,
            image,
        })
    }
}
