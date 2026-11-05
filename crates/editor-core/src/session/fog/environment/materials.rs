use std::collections::BTreeSet;
use std::io;

use compiler::Control;
use compiler::asset_sources::BuildMaterialSources;
use compiler::map::FogLiquidVolume;
use source_formats::gamefs::GameFs;
use source_formats::keyvalues::{self, Node};
use source_formats::vmt::{Material, material_path};

use super::UnderwaterFog;

const MAX_READS: usize = 256;
const MAX_KEY_BYTES: usize = 128;
const MAX_VALUE_BYTES: usize = 512;
const MAX_PROXIES: usize = 64;

fn proxy_parameters(proxy: &Node) -> Result<(), String> {
    let parameters = proxy
        .children()
        .ok_or("Underwater fog material proxy must be a block")?;
    let mut keys = BTreeSet::new();
    for parameter in parameters {
        let key = parameter.key.to_ascii_lowercase();
        let value = parameter
            .as_str()
            .ok_or("Underwater fog material proxy parameters must be scalar")?;
        if !key.is_ascii()
            || key.len() > MAX_KEY_BYTES
            || value.len() > MAX_VALUE_BYTES
            || !keys.insert(key)
        {
            return Err(
                "Underwater fog material proxy has oversized or duplicate parameters".into(),
            );
        }
    }
    let output = match proxy.key.to_ascii_lowercase().as_str() {
        "waterlod" => return Ok(()),
        "texturescroll" => "texturescrollvar",
        "animatedtexture" => "animatedtextureframenumvar",
        _ => {
            return Err(format!(
                "Underwater fog material proxy {:?} has no qualified fog-independent write targets",
                proxy.key
            ));
        }
    };
    let target = parameters
        .iter()
        .find(|parameter| parameter.key.eq_ignore_ascii_case(output))
        .and_then(Node::as_str)
        .ok_or_else(|| format!("Underwater fog material proxy requires explicit {output}"))?
        .trim()
        .to_ascii_lowercase();
    let admitted = match output {
        "texturescrollvar" => matches!(
            target.as_str(),
            "$basetexturetransform"
                | "$bumptransform"
                | "$bumptransform2"
                | "$normalmaptransform"
                | "$dudvtransform"
                | "$envmapmasktransform"
        ),
        _ => matches!(
            target.as_str(),
            "$frame"
                | "$bumpframe"
                | "$bumpframe2"
                | "$normalmapframe"
                | "$normalmapframe2"
                | "$envmapmaskframe"
                | "$refracttextureframe"
                | "$reflecttextureframe"
        ),
    };
    if !admitted {
        return Err(format!(
            "Underwater fog material proxy output {target:?} is outside the qualified texture transform/frame catalog"
        ));
    }
    Ok(())
}

fn proxies(nodes: &[Node]) -> Result<(), String> {
    if nodes.len() > MAX_PROXIES {
        return Err("Underwater fog material exceeds 64 proxies".into());
    }
    for proxy in nodes {
        proxy_parameters(proxy)?;
    }
    Ok(())
}

fn static_parameters(nodes: &[Node], patch: bool) -> Result<(), String> {
    let mut keys = BTreeSet::new();
    for node in nodes {
        let key = node.key.to_ascii_lowercase();
        if !key.is_ascii() || key.len() > MAX_KEY_BYTES || !keys.insert(key.clone()) {
            return Err("Underwater fog material has oversized or duplicate declarations".into());
        }
        if let Some(value) = node.as_str() {
            if value.len() > MAX_VALUE_BYTES {
                return Err("Underwater fog material declaration exceeds 512 bytes".into());
            }
        } else if patch && matches!(key.as_str(), "insert" | "replace") {
            static_parameters(node.children().unwrap(), false)?;
        } else if key == "proxies" {
            proxies(node.children().unwrap())?;
        } else {
            return Err("Underwater fog requires static parameters and qualified texture-only proxies without conditional blocks".into());
        }
    }
    Ok(())
}

fn static_material(bytes: &[u8]) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    let nodes =
        keyvalues::parse_static_material_bounded(text.trim_start_matches('\u{feff}'), 16, 4096)
            .map_err(|error| error.to_string())?;
    let [shader] = nodes.as_slice() else {
        return Err("Underwater fog material needs one shader block".into());
    };
    static_parameters(
        shader
            .children()
            .ok_or("Underwater fog material shader is not a block")?,
        shader.key.eq_ignore_ascii_case("patch"),
    )
}

fn number(material: &Material, key: &str) -> Result<f32, String> {
    let value = material
        .get(key)
        .ok_or_else(|| format!("Underwater fog requires explicit {key}"))?
        .trim()
        .parse::<f32>()
        .map_err(|_| format!("Underwater fog {key} must be a finite number"))?;
    if !value.is_finite() {
        return Err(format!("Underwater fog {key} must be finite"));
    }
    Ok(value)
}

fn color(material: &Material) -> Result<[f32; 3], String> {
    let value = material
        .get("$fogcolor")
        .ok_or("Underwater fog requires explicit $fogcolor")?
        .trim();
    let (inner, maximum, scale) = if let Some(inner) = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        (inner, 1.0, 1.0)
    } else if let Some(inner) = value
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
    {
        (inner, 255.0, 1.0_f32 / 255.0)
    } else {
        return Err("Underwater fog $fogcolor requires a square-bracket normalized or curly-brace byte RGB triple".into());
    };
    let mut values = inner.split_ascii_whitespace();
    let mut color = [0.0; 3];
    for channel in &mut color {
        let value = values
            .next()
            .ok_or("Underwater fog $fogcolor needs three channels")?
            .parse::<f64>()
            .map_err(|_| "Underwater fog $fogcolor needs finite channels")?;
        if !value.is_finite() || !(0.0..=maximum).contains(&value) {
            return Err(format!(
                "Underwater fog $fogcolor channels must be finite within 0–{maximum}"
            ));
        }
        *channel = value as f32 * scale;
    }
    if values.next().is_some() {
        return Err("Underwater fog $fogcolor needs exactly three channels".into());
    }
    Ok(color)
}

pub(super) fn load(
    volume: &FogLiquidVolume,
    game: &GameFs,
    sources: &BuildMaterialSources,
    control: &Control,
    reads: &mut usize,
) -> Result<UnderwaterFog, String> {
    let path = material_path(&volume.material);
    let material = Material::load_with_reader(&path, &mut |path| {
        control.checkpoint().map_err(io::Error::other)?;
        *reads = reads.saturating_add(1);
        if *reads > MAX_READS {
            return Err(io::Error::other(
                "Underwater fog exceeded 256 material reads",
            ));
        }
        let asset =
            sources.read_asset(game, path, compiler::materials::MAX_GEOMETRY_MATERIAL_BYTES)?;
        asset
            .map(|asset| {
                static_material(asset.bytes()).map_err(io::Error::other)?;
                Ok(asset.bytes().to_vec())
            })
            .transpose()
    })
    .map_err(|error| format!("Underwater material {path}: {error}"))?
    .ok_or_else(|| format!("Underwater material {path} was not found"))?;
    if !compiler::water_dependencies::is_water_shader(&material.shader)
        && !matches!(
            material.shader.as_str(),
            "lightmappedgeneric" | "unlitgeneric"
        )
    {
        return Err(format!(
            "Underwater fog shader {:?} is outside the static catalog",
            material.shader
        ));
    }
    let enabled = match material.get("$fogenable").map(str::trim) {
        Some("0") => false,
        Some("1") => true,
        _ => {
            return Err(format!(
                "Underwater material {path} requires explicit $fogenable 0 or 1"
            ));
        }
    };
    let start = number(&material, "$fogstart")?;
    let end = number(&material, "$fogend")?;
    let range = end - start;
    if range <= 0.0 || !range.is_finite() || !range.recip().is_finite() {
        return Err(format!(
            "Underwater material {path} needs fogstart < fogend and a finite inverse range"
        ));
    }
    Ok(UnderwaterFog {
        solid: volume.solid,
        source_solids: volume.source_solids.clone(),
        contents: volume.contents,
        material: volume.material.clone(),
        surface_material: volume.surface_material.clone(),
        surface_z: volume.surface,
        color_normalized: color(&material)?,
        start,
        end,
        enabled,
    })
}
