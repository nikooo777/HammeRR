use std::collections::BTreeMap;
use std::io;

use serde::Serialize;
use source_formats::gamefs::asset_relative;
use source_formats::keyvalues::Value;
use source_formats::vmt::{Material, material_path, texture_path};
use source_formats::vtf::Vtf;

use crate::materials::{MaterialId, MaterialLibrary};

use super::Issue;

pub const MAX_MATERIALS: usize = 128;
pub const MAX_MATERIAL_BYTES: usize = 64 * 1024;
pub const MAX_TEXTURE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_TEXTURE_AXIS: u32 = 2048;
pub const MAX_PATH_BYTES: usize = 256;
const UNSUPPORTED_TEXTURE_FLAGS: u32 = 0x2e01_c88d;

pub const LIMITS: &str = "Base-texture appearance for static Cable and UnlitGeneric materials with fixed editor directional shading. Cable bump maps, MinLight/MaxLight, static lighting, native billboard/AA back materials and native pixel parity are excluded. One static 2D LDR RGB/BGR/RGBA/BGRA8888 or DXT1/3/5 base, axes 1–2048, sampled through the editor's ordinary repeating sampler. Opaque, translucent base alpha and alpha test at 0.5 are admitted; UnlitGeneric also admits scalar alpha without alpha test. Cable requires scalar alpha one. Point/clamp/border sampling, normal/HDR/render-target/procedural bases, proxies, arbitrary nested blocks, base transforms, display overrides and other shader parameters are unavailable. The legacy UnlitGeneric_DX6 base-only fallback is retained as metadata; this preview uses the root base. 128 materials, 64 KiB per VMT, 16 MiB per VTF and 64 MiB decoded or retained mip data. A material failure makes the requested appearance snapshot unavailable; endpoint helpers and selection guides are separate.";

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub material: String,
    pub shader: String,
    pub texture: String,
    pub dimensions: [u32; 2],
    pub opacity: f32,
    pub translucent: bool,
    pub alpha_test: bool,
    pub interpretation: &'static str,
    pub not_rendered: Vec<&'static str>,
}

#[derive(Debug, Clone)]
pub struct Appearance {
    pub material: MaterialId,
    pub report: Report,
}

#[derive(Debug, Default, Clone)]
pub struct Prepared {
    pub by_source: BTreeMap<u32, Appearance>,
    pub issues: Vec<Issue>,
    pub generation: u64,
}

#[derive(Debug)]
pub struct Asset {
    material: Material,
    base: Vtf,
    pub report: Report,
    pub decoded_bytes: usize,
    pub retained_bytes: usize,
}

fn parameter_number(material: &Material, key: &str, default: f64) -> Result<f64, String> {
    let value = material.get(key).map_or(Ok(default), |value| {
        value
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("{key} must be a finite number"))
    })?;
    if !value.is_finite() {
        return Err(format!("{key} must be a finite number"));
    }
    Ok(value)
}

fn parameter_flag(material: &Material, key: &str) -> Result<bool, String> {
    let value = parameter_number(material, key, 0.0)?;
    if value != 0.0 && value != 1.0 {
        return Err(format!("{key} must be zero or one"));
    }
    Ok(value != 0.0)
}

fn validate_material(material: &Material) -> Result<f32, String> {
    if !matches!(material.shader.as_str(), "cable" | "unlitgeneric") {
        return Err(format!(
            "Shader {} is outside the Cable/UnlitGeneric base-texture catalog",
            material
                .shader
                .chars()
                .take(MAX_PATH_BYTES)
                .collect::<String>()
        ));
    }
    for block in &material.blocks {
        let Value::Block(parameters) = &block.value else {
            return Err("Unsupported rope material block".into());
        };
        if material.shader != "cable"
            || !block.key.eq_ignore_ascii_case("unlitgeneric_dx6")
            || parameters.len() != 1
            || !parameters[0].key.eq_ignore_ascii_case("$basetexture")
            || !matches!(parameters[0].value, Value::String(_))
        {
            return Err(
                "Rope base-texture preview excludes proxies, effects and arbitrary fallback blocks"
                    .into(),
            );
        }
    }
    for (key, _) in &material.params {
        let admitted = matches!(
            key.as_str(),
            "$basetexture"
                | "$alpha"
                | "$translucent"
                | "$alphatest"
                | "$alphatestreference"
                | "$vertexcolor"
                | "$vertexalpha"
                | "$nocull"
                | "$surfaceprop"
                | "$surfaceprop2"
                | "$nodecal"
                | "%keywords"
        ) || (material.shader == "cable"
            && matches!(key.as_str(), "$bumpmap" | "$minlight" | "$maxlight"));
        if !admitted {
            return Err(format!(
                "Rope base-texture preview excludes parameter {}",
                key.chars().take(MAX_PATH_BYTES).collect::<String>()
            ));
        }
    }
    for key in [
        "$translucent",
        "$alphatest",
        "$vertexcolor",
        "$vertexalpha",
        "$nocull",
        "$nodecal",
    ] {
        parameter_flag(material, key)?;
    }
    if material.alpha_test() && material.alpha_test_reference()? != 0.5 {
        return Err("Rope base-texture alpha test requires a 0.5 reference".into());
    }
    for key in ["$minlight", "$maxlight"] {
        parameter_number(material, key, 0.0)?;
    }
    let opacity = parameter_number(material, "$alpha", 1.0)?;
    if !(0.0..=1.0).contains(&opacity) {
        return Err("Rope material $alpha must be between zero and one".into());
    }
    if opacity != 1.0 && (material.shader == "cable" || material.alpha_test()) {
        return Err("Cable and alpha-tested rope materials require scalar $alpha one".into());
    }
    Ok(opacity as f32)
}

fn same_texture(left: &Vtf, right: &Vtf) -> bool {
    left.width == right.width
        && left.height == right.height
        && left.depth == right.depth
        && left.frames == right.frames
        && left.faces == right.faces
        && left.flags == right.flags
        && left.format == right.format
        && left.mip_count == right.mip_count
        && (0..left.mip_count).all(|mip| {
            matches!((left.image_data(mip, 0, 0), right.image_data(mip, 0, 0)),
                (Ok(left), Ok(right)) if left == right)
        })
}

impl Asset {
    pub fn load<F>(name: &str, read: &mut F) -> Result<Self, String>
    where
        F: FnMut(&str, usize) -> io::Result<Option<Vec<u8>>>,
    {
        if name.is_empty() || name.len() > MAX_PATH_BYTES {
            return Err("Rope material name must contain 1–256 bytes".into());
        }
        let path = material_path(name);
        asset_relative(&path).map_err(|error| error.to_string())?;
        let material = Material::load_with_reader(&path, &mut |path| {
            if path.len() > MAX_PATH_BYTES {
                return Err(io::Error::other(
                    "Rope material include path exceeds 256 bytes",
                ));
            }
            read(path, MAX_MATERIAL_BYTES)
        })
        .map_err(|error| format!("Rope material {name}: {error}"))?
        .ok_or_else(|| format!("Rope material {name} is missing"))?;
        let opacity = validate_material(&material)?;
        let texture = material
            .get("$basetexture")
            .filter(|path| !path.trim().is_empty())
            .ok_or("Rope material needs an authored $basetexture")?;
        let texture = texture_path(texture);
        if texture.len() > MAX_PATH_BYTES {
            return Err("Rope base texture path exceeds 256 bytes".into());
        }
        asset_relative(&texture).map_err(|error| error.to_string())?;
        let bytes = read(&texture, MAX_TEXTURE_BYTES)
            .map_err(|error| format!("Rope texture {texture}: {error}"))?
            .ok_or_else(|| format!("Rope texture {texture} is missing"))?;
        let retained_bytes = bytes.len();
        let base = Vtf::parse(bytes).map_err(|error| format!("Rope texture {texture}: {error}"))?;
        crate::normal_maps::validate_static_texture(&base, MAX_TEXTURE_AXIS)?;
        if !crate::normal_maps::ordinary_ldr_format(base.format) {
            return Err("Rope base texture requires an ordinary LDR RGB format".into());
        }
        if base.flags & UNSUPPORTED_TEXTURE_FLAGS != 0 {
            return Err(
                "Rope base texture requests unsupported sampling, normal or dynamic texture flags"
                    .into(),
            );
        }
        base.validate_native_mips()
            .map_err(|error| error.to_string())?;
        let mut decoded_bytes = 0usize;
        for mip in 0..base.mip_count {
            let image = base
                .decode_rgba8(mip, 0, 0)
                .map_err(|error| format!("Rope texture {texture}: {error}"))?;
            decoded_bytes = decoded_bytes
                .checked_add(image.pixels.len())
                .ok_or("Rope decoded texture size overflow")?;
            if decoded_bytes > MAX_DECODED_BYTES {
                return Err("Rope decoded texture exceeds 64 MiB".into());
            }
        }
        let report = Report {
            material: path
                .strip_prefix("materials/")
                .unwrap_or(&path)
                .strip_suffix(".vmt")
                .unwrap_or(name)
                .to_owned(),
            shader: material.shader.clone(),
            texture,
            dimensions: [base.width, base.height],
            opacity,
            translucent: material.translucent(),
            alpha_test: material.alpha_test(),
            interpretation: "base_texture",
            not_rendered: if material.shader == "cable" {
                vec![
                    "Cable bump-map lighting",
                    "Cable MinLight/MaxLight",
                    "native billboard and back-material antialiasing",
                ]
            } else {
                vec![
                    "native UnlitGeneric shading",
                    "native billboard antialiasing",
                ]
            },
        };
        Ok(Self {
            material,
            base,
            report,
            decoded_bytes,
            retained_bytes,
        })
    }

    pub fn check_binding(&self, library: &MaterialLibrary) -> Result<(), String> {
        if let Some(cached) = library.find(&self.report.material)
            && (cached.missing
                || cached.error.is_some()
                || !cached.matches_material(&self.material)
                || cached.display_texture.as_deref() != Some(self.report.texture.as_str())
                || cached
                    .base
                    .as_ref()
                    .is_none_or(|base| !same_texture(base, &self.base)))
        {
            return Err(format!(
                "Rope material {} differs from the cached display source; reload material assets",
                self.report.material
            ));
        }
        Ok(())
    }

    pub fn bind(self, library: &mut MaterialLibrary) -> Appearance {
        let material = library.insert_base_texture(
            &self.report.material,
            &self.material,
            self.base,
            self.report.texture.clone(),
        );
        Appearance {
            material,
            report: self.report,
        }
    }
}
