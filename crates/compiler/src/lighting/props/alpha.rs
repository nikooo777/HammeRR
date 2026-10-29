use std::sync::Arc;

use glam::DVec2;
use source_formats::{
    vmt,
    vtf::{ImageFormat, Vtf},
};

use super::{Allocation, Assets, Budget};
use crate::lighting::transport::Coverage;

const MAX_TEXTURE_SIZE: u32 = crate::MAX_BRUSH_SHADOW_TEXTURE_SIZE;
const MAX_ALPHA_BYTES: usize = crate::MAX_BRUSH_SHADOW_ALPHA_BYTES;
const CLAMP_S: u32 = 0x0000_0004;
const CLAMP_T: u32 = 0x0000_0008;
const DATA_FLAGS: u32 = 0x80 | 0x800 | 0x4000 | 0x8000 | 0x1_0000 | 0x800_0000;
const COLOR_PREPARE_BATCH: usize = 1024;
const COLOR_PIXEL_WORK: usize = 24;

#[derive(Debug)]
enum ColorProblem {
    Format(ImageFormat),
    Flags(u32),
    Frame {
        frames: u16,
        depth: Option<u16>,
        first: u16,
    },
}

impl ColorProblem {
    fn classify(vtf: &Vtf) -> Option<Self> {
        if vtf.authored_frames != 1
            || vtf.authored_depth.is_some_and(|depth| depth != 1)
            || vtf.first_frame != 0
        {
            return Some(Self::Frame {
                frames: vtf.authored_frames,
                depth: vtf.authored_depth,
                first: vtf.first_frame,
            });
        }
        if vtf.flags & DATA_FLAGS != 0 {
            return Some(Self::Flags(vtf.flags & DATA_FLAGS));
        }
        (!matches!(
            vtf.format,
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
        ))
        .then_some(Self::Format(vtf.format))
    }

    fn message(&self) -> String {
        match self {
            Self::Format(format) => format!(
                "Colored texture shadows requires ordinary unsigned LDR RGB; VTF format {format:?} is unsupported"
            ),
            Self::Flags(flags) => format!(
                "Colored texture shadows does not support normal/data, procedural, cubemap or render-target VTF flags {flags:#010x}"
            ),
            Self::Frame {
                frames,
                depth,
                first,
            } => format!(
                "Colored texture shadows requires one authored frame/depth and first frame zero; VTF has frames {frames}, depth {depth:?}, first frame {first}"
            ),
        }
    }
}

#[derive(Debug)]
enum Colored {
    Disabled,
    Prepared(Vec<[u8; 3]>),
    Unavailable(ColorProblem),
}

#[derive(Debug)]
pub(in crate::lighting) struct Texture {
    dimensions: [usize; 2],
    clamp: [bool; 2],
    alpha: Vec<u8>,
    colored: Colored,
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

    pub(in crate::lighting) fn fractional(&self) -> bool {
        self.fractional
    }

    pub(in crate::lighting) fn coverage(&self, uv: DVec2) -> Result<Coverage, String> {
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
        let index = texel[1] * self.texture.dimensions[0] + texel[0];
        let alpha = self.texture.alpha[index];
        Ok(if self.fractional {
            match &self.texture.colored {
                Colored::Prepared(coverage) => Coverage::from_channels(coverage[index]),
                Colored::Disabled => Coverage::scalar(alpha),
                Colored::Unavailable(problem) => return Err(problem.message()),
            }
        } else if f64::from(alpha) / 255.0 >= self.reference {
            Coverage::OPAQUE
        } else {
            Coverage::CLEAR
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
    colored: bool,
) -> Result<Option<Arc<Material>>, String> {
    prepare_for(material, assets, budget, false, colored)
}

pub(in crate::lighting) fn prepare_brush(
    material: &vmt::Material,
    assets: &mut Assets<'_, '_>,
    budget: &mut Budget<'_>,
    colored: bool,
) -> Result<Arc<Material>, String> {
    prepare_for(material, assets, budget, true, colored)?
        .ok_or_else(|| "Brush texture shadows requires $alphatest 1 or $translucent 1".into())
}

fn prepare_for(
    material: &vmt::Material,
    assets: &mut Assets<'_, '_>,
    budget: &mut Budget<'_>,
    brush: bool,
    colored: bool,
) -> Result<Option<Arc<Material>>, String> {
    let Some(parameters) = parameters(material, brush)? else {
        return Ok(None);
    };
    prepare_texture(parameters, assets, budget, brush, colored).map(Some)
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
    parameters: Parameters,
    assets: &mut Assets<'_, '_>,
    budget: &mut Budget<'_>,
    brush: bool,
    colored: bool,
) -> Result<Arc<Material>, String> {
    let Parameters {
        path,
        transform,
        reference,
        fractional,
    } = parameters;
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
            let color_problem = colored.then(|| ColorProblem::classify(&vtf)).flatten();
            let color_allocation = if colored && color_problem.is_none() {
                Some(budget.reserve::<[u8; 3]>(pixels)?)
            } else {
                None
            };
            budget.consume(pixels * 2)?;
            let image = vtf
                .decode_rgba8(0, 0, 0)
                .map_err(|error| format!("{path}: {error}"))?;
            if image.pixels.len() != pixels * 4 {
                return Err(format!(
                    "{path}: decoded texture-shadow image has invalid dimensions"
                ));
            }
            let colored = if let Some(problem) = color_problem {
                Colored::Unavailable(problem)
            } else if colored {
                Colored::Prepared(prepare_colored(image.pixels.as_chunks::<4>().0, budget)?)
            } else {
                Colored::Disabled
            };
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
                colored,
            });
            assets.allocations.push(alpha_allocation);
            assets.allocations.extend(color_allocation);
            assets.alpha_bytes += pixels;
            assets.textures.insert(path, Arc::clone(&texture));
            texture
        }
    };
    if colored && fractional {
        match &texture.colored {
            Colored::Unavailable(problem) => return Err(problem.message()),
            Colored::Disabled => {
                return Err(
                    "Colored texture shadows requires consistently prepared texture assets".into(),
                );
            }
            Colored::Prepared(_) => {}
        }
    }
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

fn prepare_colored(pixels: &[[u8; 4]], budget: &mut Budget<'_>) -> Result<Vec<[u8; 3]>, String> {
    budget.consume(usize::from(u8::MAX) + 1)?;
    let linear: [f64; 256] =
        std::array::from_fn(|value| (value as f64 / f64::from(u8::MAX)).powf(2.2));
    let mut coverage = Vec::with_capacity(pixels.len());
    for batch in pixels.chunks(COLOR_PREPARE_BATCH) {
        budget.consume(batch.len() * COLOR_PIXEL_WORK)?;
        coverage.extend(batch.iter().map(|pixel| {
            let alpha = pixel[3];
            if alpha == 0 || alpha == u8::MAX {
                return [alpha; 3];
            }
            let alpha = f64::from(alpha);
            let tint = alpha * (1.0 - alpha / f64::from(u8::MAX));
            std::array::from_fn(|axis| {
                (alpha + tint * (1.0 - linear[usize::from(pixel[axis])]))
                    .round()
                    .clamp(0.0, f64::from(u8::MAX)) as u8
            })
        }));
    }
    Ok(coverage)
}
