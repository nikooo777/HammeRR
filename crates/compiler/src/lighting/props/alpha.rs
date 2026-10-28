use std::sync::Arc;

use glam::DVec2;
use source_formats::{vmt, vtf::Vtf};

use super::{Allocation, Assets, Budget};

const MAX_TEXTURE_SIZE: u32 = crate::MAX_BRUSH_SHADOW_TEXTURE_SIZE;
const MAX_ALPHA_BYTES: usize = crate::MAX_BRUSH_SHADOW_ALPHA_BYTES;
const CLAMP_S: u32 = 0x0000_0004;
const CLAMP_T: u32 = 0x0000_0008;

#[derive(Debug)]
pub(in crate::lighting) struct Texture {
    dimensions: [usize; 2],
    clamp: [bool; 2],
    alpha: Vec<u8>,
}

#[derive(Debug)]
pub(in crate::lighting) struct Material {
    texture: Arc<Texture>,
    transform: [[f64; 3]; 2],
    reference: f64,
    fractional: bool,
}

impl Material {
    pub(in crate::lighting) fn dimensions(&self) -> [usize; 2] {
        self.texture.dimensions
    }

    pub(in crate::lighting) fn blocks(&self, uv: DVec2) -> Result<bool, String> {
        Ok(self.coverage(uv)? != 0)
    }

    pub(in crate::lighting) fn fractional(&self) -> bool {
        self.fractional
    }

    pub(in crate::lighting) fn coverage(&self, uv: DVec2) -> Result<u8, String> {
        let [s, t] = self.transform;
        let uv = if s[1] == 0.0 && t[0] == 0.0 {
            uv * DVec2::new(s[0], t[1]) + DVec2::new(s[2], t[2])
        } else {
            DVec2::new(
                uv.x * s[0] + uv.y * s[1] + s[2],
                uv.x * t[0] + uv.y * t[1] + t[2],
            )
        };
        if !uv.is_finite() {
            return Err("Texture-shadow coordinates must be finite".into());
        }
        let texel: [usize; 2] = std::array::from_fn(|axis| {
            let coordinate = if self.texture.clamp[axis] {
                uv[axis].clamp(0.0, 1.0)
            } else {
                uv[axis].rem_euclid(1.0)
            };
            ((coordinate * self.texture.dimensions[axis] as f64).floor() as usize)
                .min(self.texture.dimensions[axis] - 1)
        });
        let alpha = self.texture.alpha[texel[1] * self.texture.dimensions[0] + texel[0]];
        Ok(if self.fractional {
            alpha
        } else if f64::from(alpha) / 255.0 >= self.reference {
            255
        } else {
            0
        })
    }
}

fn scalar(material: &vmt::Material, key: &str, default: f64) -> Result<f64, String> {
    let Some(value) = material.get(key) else {
        return Ok(default);
    };
    let value = value
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("invalid {key}"))?;
    if !value.is_finite() {
        return Err(format!("nonfinite {key}"));
    }
    Ok(value)
}

fn transform(value: Option<&str>) -> Result<[[f64; 3]; 2], String> {
    source_formats::vmt::texture_transform_matrix(
        value,
        source_formats::vmt::DuplicateOperations::Reject,
    )
    .map_err(|error| match error {
        source_formats::vmt::TransformError::Syntax => {
            "unsupported $basetexturetransform syntax".into()
        }
        source_formats::vmt::TransformError::Duplicate => {
            "duplicate $basetexturetransform operation".into()
        }
        source_formats::vmt::TransformError::Incomplete => {
            "incomplete $basetexturetransform".into()
        }
        source_formats::vmt::TransformError::Invalid => "invalid $basetexturetransform".into(),
        source_formats::vmt::TransformError::Bounds => {
            "$basetexturetransform values must be finite and within ±1024".into()
        }
        source_formats::vmt::TransformError::Rotation => {
            "invalid $basetexturetransform rotation".into()
        }
    })
}

fn texture_allocations(
    pixels: usize,
    budget: &Budget<'_>,
) -> Result<(Allocation, Allocation), String> {
    let alpha_bytes = pixels
        .checked_add(size_of::<Texture>() + 2 * size_of::<usize>())
        .ok_or("Texture-shadow alpha allocation size overflow")?;
    let alpha = budget.reserve_bytes(alpha_bytes)?;
    let rgba_bytes = pixels
        .checked_mul(4)
        .ok_or("Texture-shadow image allocation size overflow")?;
    let rgba = budget.reserve_bytes(rgba_bytes)?;
    Ok((alpha, rgba))
}

pub(in crate::lighting) fn prepare(
    material: &vmt::Material,
    assets: &mut Assets<'_, '_>,
    budget: &mut Budget<'_>,
) -> Result<Option<Arc<Material>>, String> {
    prepare_for(material, assets, budget, false)
}

pub(in crate::lighting) fn prepare_brush(
    material: &vmt::Material,
    assets: &mut Assets<'_, '_>,
    budget: &mut Budget<'_>,
) -> Result<Arc<Material>, String> {
    prepare_for(material, assets, budget, true)?
        .ok_or_else(|| "Brush texture shadows requires $alphatest 1 or $translucent 1".into())
}

fn prepare_for(
    material: &vmt::Material,
    assets: &mut Assets<'_, '_>,
    budget: &mut Budget<'_>,
    brush: bool,
) -> Result<Option<Arc<Material>>, String> {
    let Some(Parameters {
        path,
        transform,
        reference,
        fractional,
    }) = parameters(material, brush)?
    else {
        return Ok(None);
    };
    prepare_texture(
        path, transform, reference, fractional, assets, budget, brush,
    )
    .map(Some)
}

struct Parameters {
    path: String,
    transform: [[f64; 3]; 2],
    reference: f64,
    fractional: bool,
}

pub(in crate::lighting) fn validate_brush(material: &vmt::Material) -> Result<(), String> {
    parameters(material, true)?.ok_or_else(|| {
        "Brush texture shadows requires $alphatest 1 or $translucent 1".to_string()
    })?;
    Ok(())
}

fn parameters(material: &vmt::Material, brush: bool) -> Result<Option<Parameters>, String> {
    if brush {
        if material.shader != "lightmappedgeneric" {
            return Err(
                "Brush texture shadows supports only LightmappedGeneric alpha materials".into(),
            );
        }
        for (key, _) in &material.params {
            if !matches!(
                key.as_str(),
                "$basetexture"
                    | "$basetexturetransform"
                    | "$alphatest"
                    | "$alphatestreference"
                    | "$translucent"
                    | "$alpha"
                    | "$frame"
                    | "$surfaceprop"
                    | "$surfaceprop2"
                    | "%keywords"
                    | "%tooltexture"
            ) {
                return Err(format!(
                    "Brush texture shadows does not support alpha material parameter {key}"
                ));
            }
        }
    }
    if !matches!(
        material.shader.as_str(),
        "vertexlitgeneric" | "unlitgeneric" | "lightmappedgeneric"
    ) {
        return Err(format!(
            "unsupported texture-shadow shader {}",
            material.shader
        ));
    }
    if !material.blocks.is_empty() {
        return Err(
            "material animation and conditional blocks are unsupported for texture shadows".into(),
        );
    }
    let translucent = scalar(material, "$translucent", 0.0)?;
    let alpha_test = scalar(material, "$alphatest", 0.0)?;
    let fractional = translucent != 0.0;
    if fractional && (translucent != 1.0 || alpha_test != 0.0) {
        return Err("$translucent texture shadows require value one and no alpha test".into());
    }
    if fractional && material.get("$alphatestreference").is_some() {
        return Err("$alphatestreference is unsupported for $translucent texture shadows".into());
    }
    if brush && !fractional && alpha_test != 1.0 {
        return Err("Brush texture shadows requires $alphatest 1 or $translucent 1".into());
    }
    for key in ["$additive", "$vertexalpha", "$distancealpha"] {
        if scalar(material, key, 0.0)? != 0.0 {
            return Err(format!("{key} is unsupported for texture shadows"));
        }
    }
    if scalar(material, "$alpha", 1.0)? != 1.0 {
        return Err("$alpha modulation is unsupported for texture shadows".into());
    }
    if alpha_test == 0.0 && !fractional {
        return Ok(None);
    }
    for key in ["$selfillum", "$basealphaenvmapmask", "$seamless_scale"] {
        if scalar(material, key, 0.0)? != 0.0 {
            return Err(format!("{key} is unsupported for texture shadows"));
        }
    }
    if !brush
        && !matches!(
            material.shader.as_str(),
            "vertexlitgeneric" | "unlitgeneric"
        )
    {
        return Err(format!(
            "alpha-texture shadows do not support shader {}",
            material.shader
        ));
    }
    for (key, _) in &material.params {
        if fractional
            && !matches!(
                key.as_str(),
                "$basetexture"
                    | "$basetexturetransform"
                    | "$translucent"
                    | "$alphatest"
                    | "$alpha"
                    | "$additive"
                    | "$vertexalpha"
                    | "$distancealpha"
                    | "$selfillum"
                    | "$basealphaenvmapmask"
                    | "$seamless_scale"
                    | "$frame"
                    | "$model"
                    | "$surfaceprop"
                    | "$surfaceprop2"
                    | "%keywords"
                    | "%tooltexture"
            )
        {
            return Err(format!(
                "unsupported $translucent texture-shadow parameter {key}"
            ));
        }
        if !matches!(
            key.as_str(),
            "$basetexture"
                | "$basetexturetransform"
                | "$alphatest"
                | "$alphatestreference"
                | "$alpha"
                | "$translucent"
                | "$additive"
                | "$vertexalpha"
                | "$distancealpha"
                | "$selfillum"
                | "$seamless_scale"
                | "$frame"
                | "$model"
                | "$nocull"
                | "$surfaceprop"
                | "$surfaceprop2"
                | "$color"
                | "$color2"
                | "$halflambert"
                | "$bumpmap"
                | "$envmap"
                | "$envmapmask"
                | "$envmaptint"
                | "$basealphaenvmapmask"
                | "$normalmapalphaenvmapmask"
                | "$nodecal"
                | "$nofog"
                | "%keywords"
                | "%tooltexture"
        ) {
            return Err(format!(
                "unsupported alpha-test texture-shadow parameter {key}"
            ));
        }
    }
    if scalar(material, "$frame", 0.0)? != 0.0 {
        return Err("nonzero $frame is unsupported for texture shadows".into());
    }
    let reference = material.alpha_test_reference()?;
    let transform = transform(material.get("$basetexturetransform"))?;
    let path = material
        .get("$basetexture")
        .filter(|value| !value.trim().is_empty())
        .ok_or("alpha-texture shadows require $basetexture")?;
    let path = vmt::texture_path(path);
    Ok(Some(Parameters {
        path,
        transform,
        reference,
        fractional,
    }))
}

fn prepare_texture(
    path: String,
    transform: [[f64; 3]; 2],
    reference: f64,
    fractional: bool,
    assets: &mut Assets<'_, '_>,
    budget: &mut Budget<'_>,
    brush: bool,
) -> Result<Arc<Material>, String> {
    let texture = match assets.textures.get(&path) {
        Some(texture) => Arc::clone(texture),
        None => {
            if brush {
                assets.allocations.push(budget.reserve_bytes(
                    path.len() + 16 * size_of::<(String, Arc<Texture>)>() + 16 * size_of::<usize>(),
                )?);
            }
            let bytes = assets.read(&path, budget)?;
            let vtf = Vtf::parse(bytes).map_err(|error| format!("{path}: {error}"))?;
            vtf.validate_native_mips()
                .map_err(|error| format!("{path}: {error}"))?;
            if vtf.frames != 1 || vtf.faces != 1 || vtf.depth != 1 {
                return Err(format!(
                    "{path}: animated, cubemap and volume texture shadows are unsupported"
                ));
            }
            if vtf.width > MAX_TEXTURE_SIZE || vtf.height > MAX_TEXTURE_SIZE {
                return Err(format!(
                    "{path}: texture shadows exceed the {MAX_TEXTURE_SIZE} pixel dimension limit"
                ));
            }
            let pixels = vtf.width as usize * vtf.height as usize;
            if pixels > MAX_ALPHA_BYTES - assets.alpha_bytes {
                return Err("Texture shadows exceed the 64 MiB decoded alpha limit".into());
            }
            let (alpha_allocation, _rgba_allocation) = texture_allocations(pixels, budget)?;
            budget.consume(pixels * 2)?;
            let image = vtf
                .decode_rgba8(0, 0, 0)
                .map_err(|error| format!("{path}: {error}"))?;
            if image.pixels.len() != pixels * 4 {
                return Err(format!(
                    "{path}: decoded texture-shadow image has invalid dimensions"
                ));
            }
            let texture = Arc::new(Texture {
                dimensions: [vtf.width as usize, vtf.height as usize],
                clamp: [vtf.flags & CLAMP_S != 0, vtf.flags & CLAMP_T != 0],
                alpha: image
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|pixel| pixel[3])
                    .collect(),
            });
            assets.allocations.push(alpha_allocation);
            assets.alpha_bytes += pixels;
            assets.textures.insert(path, Arc::clone(&texture));
            texture
        }
    };
    let material_allocation =
        budget.reserve_bytes(size_of::<Material>() + 2 * size_of::<usize>())?;
    assets.allocations.push(material_allocation);
    Ok(Arc::new(Material {
        texture,
        transform,
        reference,
        fractional,
    }))
}
