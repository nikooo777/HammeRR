use std::{io, sync::Arc};

use source_formats::{bsp::PakArchive, vmt, vtf::ImageFormat};

mod normals;
pub use normals::{validate_directional_inputs, validate_directional_inputs_controlled};

pub const POLICY: &str = "Exact compiled static diffuse closure, PAK first and guarded original assets only when absent; base mip, linear repeat filtering; LDR diffuse sRGB to linear, HDR diffuse linear; finite static colour modulation; genuine UnlitGeneric without a lightmap retains final diffuse geometry without fabricated samples. Directional tangent normals use normalized squared positive compiler-basis projections; SSBump RGB directly weights the three directional maps. No native appearance, overbright upload or adaptation parity.";
pub const PROP_POLICY: &str = "Compiler-admitted selected-skin VertexLitGeneric and strict cutout closure; absent optional base texture means white, declared missing textures refuse. Base mip and linear repeat filtering; material alpha supplies default 0.5 cutout coverage independently of VHV alpha. Decoded vertex radiance interpolates before diffuse and shared EV, safe Reinhard and one sRGB output; no VHV style decomposition. Double-sided editor inspection retains final triangles in diagnostic views; FACE luxel density and smoothing groups do not apply. Native culling, composed cutout appearance, gamma and adaptation parity are not established.";

pub type Transform = [[f32; 3]; 2];
pub const GPU_MATERIAL_OVERHEAD: usize = 176;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Single,
    VertexTransition,
    TwoTextureBlend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalEncoding {
    Tangent,
    SsBump,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialLighting {
    Unlit,
    Lightmap { directional: bool },
    Vertex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    Opaque,
    AlphaTest,
}

#[derive(Debug)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 4]>,
}

impl Texture {
    pub fn decoded_bytes(&self) -> usize {
        self.pixels.len() * std::mem::size_of::<[f32; 4]>()
    }
}

#[derive(Debug)]
pub struct NormalTexture {
    pub texture: Arc<Texture>,
    pub encoding: NormalEncoding,
}

#[derive(Debug)]
pub struct Material {
    pub name: String,
    pub kind: Kind,
    pub diffuse: [Option<Arc<Texture>>; 2],
    pub normals: [Option<NormalTexture>; 2],
    pub diffuse_transform: [Transform; 2],
    pub normal_transform: [Transform; 2],
    pub color: [f32; 3],
    pub detail_alpha_mask: bool,
    pub lighting: MaterialLighting,
    pub coverage: Coverage,
    pub nocull: bool,
}

impl Material {
    pub fn decoded_bytes(&self) -> usize {
        self.diffuse
            .iter()
            .flatten()
            .map(|t| t.decoded_bytes())
            .sum::<usize>()
            + self
                .normals
                .iter()
                .flatten()
                .map(|n| n.texture.decoded_bytes())
                .sum::<usize>()
    }

    pub fn gpu_bytes(&self) -> usize {
        self.decoded_bytes() + GPU_MATERIAL_OVERHEAD
    }
}

#[derive(Debug)]
pub struct Budget {
    pub max_asset_bytes: usize,
    pub max_texture_axis: u32,
    pub max_decoded_bytes: usize,
    pub decoded_bytes: usize,
}

pub fn load(
    name: &str,
    directional: bool,
    lightmapped: bool,
    pak: &PakArchive,
    read: &mut dyn FnMut(&str) -> io::Result<Option<Vec<u8>>>,
    control: &mut dyn FnMut() -> Result<(), String>,
    budget: &mut Budget,
) -> Result<Material, String> {
    let initial = budget.decoded_bytes;
    let result = load_inner(name, directional, lightmapped, pak, read, control, budget);
    if result.is_err() {
        budget.decoded_bytes = initial;
    }
    result.map_err(|error| format!("Baked material {name}: {error}"))
}

/// The resolved material must have passed the compiler's static-prop receiver admission.
pub fn load_static_prop_resolved(
    name: &str,
    material: &vmt::Material,
    pak: &PakArchive,
    read: &mut dyn FnMut(&str) -> io::Result<Option<Vec<u8>>>,
    control: &mut dyn FnMut() -> Result<(), String>,
    budget: &mut Budget,
) -> Result<Material, String> {
    let initial = budget.decoded_bytes;
    let result = load_static_prop_inner(name, material, pak, read, control, budget);
    if result.is_err() {
        budget.decoded_bytes = initial;
    }
    result.map_err(|error| format!("Baked prop material {name}: {error}"))
}

fn load_static_prop_inner(
    name: &str,
    material: &vmt::Material,
    pak: &PakArchive,
    read: &mut dyn FnMut(&str) -> io::Result<Option<Vec<u8>>>,
    control: &mut dyn FnMut() -> Result<(), String>,
    budget: &mut Budget,
) -> Result<Material, String> {
    control()?;
    source_formats::gamefs::asset_relative(name).map_err(|error| error.to_string())?;
    if material.shader != "vertexlitgeneric" || !material.blocks.is_empty() {
        return Err("Compiled prop diffuse requires admitted plain VertexLitGeneric".into());
    }
    let diffuse = match material.get("$basetexture") {
        Some(texture) => Some(load_texture(texture, false, false, pak, read, control, budget)?.0),
        None => None,
    };
    let transform = transform(material.get("$basetexturetransform"))?;
    control()?;
    Ok(Material {
        name: name.to_ascii_lowercase(),
        kind: Kind::Single,
        diffuse: [diffuse, None],
        normals: [None, None],
        diffuse_transform: [transform; 2],
        normal_transform: [transform; 2],
        color: [1.0; 3],
        detail_alpha_mask: false,
        lighting: MaterialLighting::Vertex,
        coverage: if material.get("$alphatest") == Some("1") {
            Coverage::AlphaTest
        } else {
            Coverage::Opaque
        },
        nocull: material
            .get("$nocull")
            .and_then(|value| value.trim().parse::<f64>().ok())
            == Some(1.0),
    })
}

fn load_inner(
    name: &str,
    directional: bool,
    lightmapped: bool,
    pak: &PakArchive,
    read: &mut dyn FnMut(&str) -> io::Result<Option<Vec<u8>>>,
    control: &mut dyn FnMut() -> Result<(), String>,
    budget: &mut Budget,
) -> Result<Material, String> {
    control()?;
    source_formats::gamefs::asset_relative(name).map_err(|error| error.to_string())?;
    let maximum = budget.max_asset_bytes;
    let material = vmt::Material::load_with_reader(&vmt::material_path(name), &mut |path| {
        read_asset(path, pak, read, control, maximum)
    })
    .map_err(|error| error.to_string())?
    .ok_or("Compiled material was not found in the owned PAK or original sources")?;
    let unlit = material.shader == "unlitgeneric";
    let kind = match material.shader.as_str() {
        "lightmappedgeneric"
            if material.get("$basetexture2").is_some() || material.get("$bumpmap2").is_some() =>
        {
            Kind::VertexTransition
        }
        "lightmappedgeneric" | "unlitgeneric" => Kind::Single,
        "worldvertextransition" => Kind::VertexTransition,
        "worldtwotextureblend" => Kind::TwoTextureBlend,
        _ => {
            return Err(format!(
                "Shader {} is outside the static lightmapped matrix",
                material.shader
            ));
        }
    };
    validate_parameters(&material, kind)?;
    if unlit && (lightmapped || directional) || !unlit && !lightmapped {
        return Err(
            "Compiled lightmap availability and resolved unlit/lit material policy disagree".into(),
        );
    }
    if !unlit && !material.compile_semantics().lit() {
        return Err("Sky, hidden and no-light faces do not have baked diffuse lighting".into());
    }
    let suppressed = flag(&material, "$nodiffusebumplighting")?;
    let primary_normal = material.get("$bumpmap");
    if directional != (primary_normal.is_some() && !suppressed) {
        return Err("Compiled BUMPLIGHT and resolved primary normal policy disagree".into());
    }
    let primary = material
        .primary_base_texture()?
        .ok_or("Static lit material requires an actual diffuse texture")?;
    let mut diffuse = [None, None];
    diffuse[0] = Some(load_texture(primary, false, false, pak, read, control, budget)?.0);
    let base_transform = transform(material.get("$basetexturetransform"))?;
    let mut diffuse_transform = [base_transform; 2];
    if kind == Kind::VertexTransition {
        if let Some(secondary) = material.get("$basetexture2") {
            diffuse[1] = Some(load_texture(secondary, false, false, pak, read, control, budget)?.0);
        } else if material.shader == "worldvertextransition" {
            return Err("WorldVertexTransition requires $basetexture2".into());
        }
    } else if kind == Kind::TwoTextureBlend
        && let Some(detail) = material.get("$detail")
    {
        diffuse[1] = Some(load_texture(detail, false, false, pak, read, control, budget)?.0);
        let scale = scalar(&material, "$detailscale", 4.0)?;
        if scale == 0.0 {
            return Err("WorldTwoTextureBlend requires nonzero $detailscale".into());
        }
        diffuse_transform[1] = base_transform.map(|row| row.map(|value| value * scale));
    }
    let mut normals = [None, None];
    let bump_transform = transform(material.get("$bumptransform"))?;
    let normal_transform = if kind == Kind::TwoTextureBlend && diffuse[1].is_some() {
        [diffuse_transform[1]; 2]
    } else if kind == Kind::TwoTextureBlend {
        [bump_transform; 2]
    } else {
        [base_transform, bump_transform]
    };
    let ssbump = flag(&material, "$ssbump")?;
    if directional {
        let primary = primary_normal.ok_or("Directional material requires $bumpmap")?;
        let (texture, encoding) = load_texture(primary, true, ssbump, pak, read, control, budget)?;
        normals[0] = Some(NormalTexture { texture, encoding });
        if let Some(secondary) = material.get("$bumpmap2") {
            let (texture, encoding) =
                load_texture(secondary, true, ssbump, pak, read, control, budget)?;
            if Some(encoding) != normals[0].as_ref().map(|normal| normal.encoding) {
                return Err("Primary and secondary normals require the same encoding".into());
            }
            normals[1] = Some(NormalTexture { texture, encoding });
        }
    }
    let first = color(&material, "$color")?;
    let second = color(&material, "$color2")?;
    let color = std::array::from_fn(|index| first[index] * second[index]);
    if color.iter().any(|value| !value.is_finite()) {
        return Err("Static colour modulation overflowed".into());
    }
    control()?;
    let material = Material {
        name: name.to_ascii_lowercase(),
        kind,
        diffuse,
        normals,
        diffuse_transform,
        normal_transform,
        color,
        detail_alpha_mask: flag(&material, "$detail_alpha_mask_base_texture")?,
        lighting: if unlit {
            MaterialLighting::Unlit
        } else {
            MaterialLighting::Lightmap { directional }
        },
        coverage: Coverage::Opaque,
        nocull: false,
    };
    validate_directional_inputs_controlled(&material, control)?;
    Ok(material)
}

pub fn read_asset(
    path: &str,
    pak: &PakArchive,
    read: &mut dyn FnMut(&str) -> io::Result<Option<Vec<u8>>>,
    control: &mut dyn FnMut() -> Result<(), String>,
    maximum: usize,
) -> io::Result<Option<Vec<u8>>> {
    control().map_err(io::Error::other)?;
    let path = source_formats::gamefs::asset_relative(path)?;
    let mut matching = pak
        .entries
        .iter()
        .filter(|entry| entry.name.eq_ignore_ascii_case(&path));
    let entry = matching.next();
    if matching.next().is_some() {
        return Err(io::Error::other(format!(
            "Ambiguous compiled PAK asset {path}"
        )));
    }
    let bytes = match entry {
        Some(entry) => {
            if entry.data.len() > maximum {
                return Err(io::Error::other(format!(
                    "PAK asset {path} exceeds the asset working budget"
                )));
            }
            Some(entry.data.clone())
        }
        None => read(&path)?,
    };
    if bytes.as_ref().is_some_and(|bytes| bytes.len() > maximum) {
        return Err(io::Error::other(format!(
            "Asset {path} exceeds the asset working budget"
        )));
    }
    control().map_err(io::Error::other)?;
    Ok(bytes)
}

fn transform(value: Option<&str>) -> Result<Transform, String> {
    let rows = vmt::texture_transform_matrix(value, vmt::DuplicateOperations::Reject)
        .map_err(|error| format!("Invalid static texture transform: {error:?}"))?;
    let determinant = rows[0][0] * rows[1][1] - rows[0][1] * rows[1][0];
    if !determinant.is_finite() || determinant.abs() < 1e-12 {
        return Err("Static texture transform must be nonsingular".into());
    }
    Ok(rows.map(|row| row.map(|value| value as f32)))
}

fn flag(material: &vmt::Material, key: &str) -> Result<bool, String> {
    match material.get(key).map(str::trim) {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        _ => Err(format!("Static {key} requires literal 0 or 1")),
    }
}

fn scalar(material: &vmt::Material, key: &str, default: f32) -> Result<f32, String> {
    let value = material
        .get(key)
        .map(str::parse::<f32>)
        .transpose()
        .map_err(|_| format!("Invalid static {key}"))?
        .unwrap_or(default);
    if !value.is_finite() || value.abs() > 1024.0 {
        return Err(format!("Static {key} must be finite and within ±1024"));
    }
    Ok(value)
}

fn color(material: &vmt::Material, key: &str) -> Result<[f32; 3], String> {
    let Some(value) = material.get(key) else {
        return Ok([1.0; 3]);
    };
    let value = value.trim();
    let integer = value.starts_with('{') && value.ends_with('}');
    let body = if integer || value.starts_with('[') && value.ends_with(']') {
        &value[1..value.len() - 1]
    } else {
        value
    };
    let values = body
        .split_whitespace()
        .map(str::parse::<f32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| format!("Invalid static {key} colour"))?;
    let values: [f32; 3] = values
        .try_into()
        .map_err(|_| format!("Static {key} requires three colour components"))?;
    let values = values.map(|value| if integer { value / 255.0 } else { value });
    if values
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0 || *value > 1024.0)
    {
        return Err(format!(
            "Static {key} colour components must be finite and within 0..1024"
        ));
    }
    Ok(values)
}

fn validate_parameters(material: &vmt::Material, kind: Kind) -> Result<(), String> {
    if !material.blocks.is_empty() {
        return Err(
            "Proxies, conditional fallbacks and nested effect blocks are outside the static matrix"
                .into(),
        );
    }
    for (key, value) in &material.params {
        if material.shader == "unlitgeneric"
            && (key.starts_with("$bump")
                || key.starts_with("$ssbump")
                || key.starts_with("$normal")
                || key == "$nodiffusebumplighting")
        {
            return Err(format!(
                "UnlitGeneric static diffuse excludes normal input {key}"
            ));
        }
        if key.starts_with('%')
            || key.starts_with("$envmap")
            || matches!(
                key.as_str(),
                "$surfaceprop"
                    | "$surfaceprop2"
                    | "$reflectivity"
                    | "$nodecal"
                    | "$nolod"
                    | "$basetexture"
                    | "$albedo"
                    | "$basetexturetransform"
                    | "$bumpmap"
                    | "$bumptransform"
                    | "$nodiffusebumplighting"
                    | "$color"
                    | "$color2"
                    | "$normalmapalphaenvmapmask"
                    | "$basealphaenvmapmask"
                    | "$basetexturenoenvmap"
                    | "$basetexture2noenvmap"
            )
        {
            continue;
        }
        match key.as_str() {
            "$basetexture2" | "$bumpmap2" if kind == Kind::VertexTransition => {}
            "$ssbump" if kind != Kind::TwoTextureBlend => {
                flag(material, key)?;
            }
            "$detail" | "$detailscale" | "$detail_alpha_mask_base_texture"
                if kind == Kind::TwoTextureBlend => {}
            "$frame" | "$bumpframe" | "$bumpframe2" | "$detailframe" if value.trim() == "0" => {}
            "$alpha" | "$alpha2" if value.trim() == "1" => {}
            "$translucent" | "$alphatest" | "$additive" | "$selfillum" | "$vertexcolor"
            | "$vertexalpha"
                if value.trim() == "0" => {}
            _ => return Err(format!("Unsupported static material parameter {key}")),
        }
    }
    if material.get("$bumpmap2").is_some() && material.get("$bumpmap").is_none() {
        return Err("A secondary-only normal has no compiled directional maps".into());
    }
    if kind == Kind::TwoTextureBlend
        && flag(material, "$detail_alpha_mask_base_texture")?
        && material.get("$detail").is_none()
    {
        return Err("Detail alpha masking requires an actual $detail texture".into());
    }
    Ok(())
}

fn load_texture(
    name: &str,
    normal: bool,
    ssbump: bool,
    pak: &PakArchive,
    read: &mut dyn FnMut(&str) -> io::Result<Option<Vec<u8>>>,
    control: &mut dyn FnMut() -> Result<(), String>,
    budget: &mut Budget,
) -> Result<(Arc<Texture>, NormalEncoding), String> {
    if name.trim().is_empty() {
        return Err("A declared static texture path is blank".into());
    }
    source_formats::gamefs::asset_relative(name.trim()).map_err(|error| error.to_string())?;
    let path = vmt::texture_path(name);
    let bytes = read_asset(&path, pak, read, control, budget.max_asset_bytes)
        .map_err(|error| format!("{path}: {error}"))?
        .ok_or_else(|| format!("Required compiled texture {path} was not found"))?;
    let source_bytes = bytes.len();
    let texture =
        source_formats::vtf::Vtf::parse(bytes).map_err(|error| format!("{path}: {error}"))?;
    crate::normal_maps::validate_static_texture(&texture, budget.max_texture_axis)?;
    const UNSUPPORTED_FLAGS: u32 = 0x2201_c80d;
    if texture.flags & UNSUPPORTED_FLAGS != 0 || normal && texture.flags & 0x40 != 0 {
        return Err(format!(
            "{path}: clamp, point, border, procedural, environment, render-target or sRGB-normal texture flags are outside the static matrix"
        ));
    }
    let encoded_ssbump = texture.flags & 0x0800_0000 != 0;
    if !normal && encoded_ssbump {
        return Err(format!("{path}: SSBump cannot serve as a diffuse texture"));
    }
    if normal && encoded_ssbump && !ssbump {
        return Err(format!(
            "{path}: SSBump texture requires an explicit $ssbump 1 material declaration"
        ));
    }
    let encoding = if ssbump {
        NormalEncoding::SsBump
    } else {
        NormalEncoding::Tangent
    };
    let count = (texture.width as usize)
        .checked_mul(texture.height as usize)
        .ok_or("Texture pixel count overflow")?;
    let decoded = count
        .checked_mul(std::mem::size_of::<[f32; 4]>())
        .ok_or("Texture memory overflow")?;
    let temporary = count
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(source_bytes))
        .ok_or("Texture working memory overflow")?;
    let total = budget
        .decoded_bytes
        .checked_add(decoded)
        .ok_or("Texture decoded budget overflow")?;
    if total
        .checked_add(temporary)
        .is_none_or(|peak| peak > budget.max_decoded_bytes)
    {
        return Err(format!(
            "{path}: decoded textures and temporary decoder exceed the material working budget"
        ));
    }
    control()?;
    let pixels = if normal {
        decode_normals(&texture, encoding)?
    } else {
        decode_diffuse(&texture)?
    };
    if pixels.len() != count {
        return Err(format!("{path}: decoded texture dimensions disagree"));
    }
    if pixels.iter().any(|pixel| {
        pixel.iter().any(|value| !value.is_finite())
            || !normal
                && (pixel[..3].iter().any(|value| *value < 0.0) || !(0.0..=1.0).contains(&pixel[3]))
    }) {
        return Err(format!(
            "{path}: texture contains invalid linear components or alpha outside 0..1"
        ));
    }
    control()?;
    budget.decoded_bytes = total;
    Ok((
        Arc::new(Texture {
            width: texture.width,
            height: texture.height,
            pixels,
        }),
        encoding,
    ))
}

fn decode_diffuse(texture: &source_formats::vtf::Vtf) -> Result<Vec<[f32; 4]>, String> {
    match texture.format {
        ImageFormat::Uv88 | ImageFormat::Uvwq8888 | ImageFormat::Uvlx8888 => {
            Err("Signed/DUDV textures cannot serve as diffuse colour".into())
        }
        ImageFormat::Rgba16161616F | ImageFormat::Rgba16161616 => {
            linear_sampler_components(texture)
        }
        _ => {
            let image = texture
                .decode_rgba8(0, 0, 0)
                .map_err(|error| error.to_string())?;
            Ok(image
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| {
                    [
                        srgb_to_linear(f32::from(pixel[0]) / 255.0),
                        srgb_to_linear(f32::from(pixel[1]) / 255.0),
                        srgb_to_linear(f32::from(pixel[2]) / 255.0),
                        f32::from(pixel[3]) / 255.0,
                    ]
                })
                .collect())
        }
    }
}

fn decode_normals(
    texture: &source_formats::vtf::Vtf,
    encoding: NormalEncoding,
) -> Result<Vec<[f32; 4]>, String> {
    let mut pixels = linear_sampler_components(texture)?;
    for value in &mut pixels {
        if encoding == NormalEncoding::Tangent {
            for component in &mut value[..3] {
                *component = *component * 2.0 - 1.0;
            }
        } else if value[..3].iter().any(|value| *value < 0.0) {
            return Err("SSBump coefficients must be nonnegative".into());
        }
    }
    Ok(pixels)
}

fn linear_sampler_components(texture: &source_formats::vtf::Vtf) -> Result<Vec<[f32; 4]>, String> {
    let bytes = texture
        .image_data(0, 0, 0)
        .map_err(|error| error.to_string())?;
    let pixels = match texture.format {
        ImageFormat::Uv88 => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pixel| {
                [
                    signed_component(pixel[0]),
                    signed_component(pixel[1]),
                    1.0,
                    1.0,
                ]
            })
            .collect(),
        ImageFormat::Uvwq8888 => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| pixel.map(signed_component))
            .collect(),
        ImageFormat::Uvlx8888 => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| {
                [
                    signed_component(pixel[0]),
                    signed_component(pixel[1]),
                    f32::from(pixel[2]) / 255.0,
                    1.0,
                ]
            })
            .collect(),
        ImageFormat::Rgba16161616F | ImageFormat::Rgba16161616 => bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|pixel| {
                std::array::from_fn(|index| {
                    let bits = u16::from_le_bytes([pixel[index * 2], pixel[index * 2 + 1]]);
                    if texture.format == ImageFormat::Rgba16161616F {
                        half::f16::from_bits(bits).to_f32()
                    } else {
                        f32::from(bits) / 65535.0
                    }
                })
            })
            .collect(),
        ImageFormat::Rgb888
        | ImageFormat::Bgr888
        | ImageFormat::Rgba8888
        | ImageFormat::Bgra8888
        | ImageFormat::Abgr8888
        | ImageFormat::Argb8888
        | ImageFormat::Bgrx8888
        | ImageFormat::Rgb565
        | ImageFormat::Bgr565
        | ImageFormat::Bgrx5551
        | ImageFormat::Bgra4444
        | ImageFormat::Bgra5551
        | ImageFormat::Dxt1
        | ImageFormat::Dxt3
        | ImageFormat::Dxt5
        | ImageFormat::Dxt1OneBitAlpha => {
            let image = texture
                .decode_rgba8(0, 0, 0)
                .map_err(|error| error.to_string())?;
            image
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| pixel.map(|value| f32::from(value) / 255.0))
                .collect()
        }
        _ => {
            return Err(format!(
                "Normal encoding {:?} does not provide a declared three-component static normal input",
                texture.format
            ));
        }
    };
    Ok(pixels)
}

fn signed_component(value: u8) -> f32 {
    (f32::from(value as i8) / 127.0).max(-1.0)
}

fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

pub fn directional_weights(normal: [f32; 3], encoding: NormalEncoding) -> Result<[f32; 3], String> {
    if normal.iter().any(|value| !value.is_finite()) {
        return Err("Directional normal components must be finite".into());
    }
    if encoding == NormalEncoding::SsBump {
        if normal.iter().any(|value| *value < 0.0) {
            return Err("SSBump coefficients must be nonnegative".into());
        }
        return Ok(normal);
    }
    let normal = glam::Vec3::from_array(normal)
        .try_normalize()
        .ok_or("Filtered tangent normal is degenerate")?;
    let z = (1.0_f32 / 3.0).sqrt();
    let x = (1.0_f32 / 6.0).sqrt();
    let y = (1.0_f32 / 2.0).sqrt();
    let basis = [
        glam::Vec3::new((2.0_f32 / 3.0).sqrt(), 0.0, z),
        glam::Vec3::new(-x, y, z),
        glam::Vec3::new(-x, -y, z),
    ];
    let weights = basis.map(|basis| normal.dot(basis).max(0.0).powi(2));
    let sum: f32 = weights.iter().sum();
    if !sum.is_finite() || sum <= 0.0 {
        return Err("Filtered tangent normal has no positive directional projection".into());
    }
    Ok(weights.map(|value| value / sum))
}
