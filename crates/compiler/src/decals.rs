use std::{collections::BTreeMap, io};

use source_formats::{
    bsp::PakLimits,
    gamefs::{GameFs, asset_relative},
    vmf::Vmf,
    vmt::{self, Material},
    vtf::Vtf,
};

use crate::{Control, Diagnostic, Report, asset_sources::BuildMaterialSources};

pub const CLASSNAME: &str = "infodecal";
pub const TEXTURE_KEY: &str = "texture";
const MAX_VMT_BYTES: usize = 64 * 1024;
const MAX_NATIVE_VTF_MINOR: u32 = 4;

pub fn is_class(classname: &str) -> bool {
    classname.eq_ignore_ascii_case(CLASSNAME)
}

pub fn texture_name(value: &str) -> Result<String, String> {
    let name = asset_relative(value.trim()).map_err(|error| error.to_string())?;
    let name = name.strip_prefix("materials/").unwrap_or(&name);
    let name = name.strip_suffix(".vmt").unwrap_or(name);
    if name.is_empty() || vmt::material_path(name).len() > PakLimits::default().max_name_bytes {
        return Err("Enter a decal material name within Source's 259-byte asset path limit".into());
    }
    Ok(name.into())
}

pub fn validate_placement_material(game: &GameFs, name: &str) -> Result<(), String> {
    match check_material(name, &mut |path, limit| game.read_limited(path, limit))? {
        Some(message) => Err(message),
        None => Ok(()),
    }
}

fn check_material(
    name: &str,
    read: &mut impl FnMut(&str, usize) -> io::Result<Option<Vec<u8>>>,
) -> Result<Option<String>, String> {
    let path = vmt::material_path(name);
    let material = Material::load_with_reader(&path, &mut |path| read(path, MAX_VMT_BYTES))
        .map_err(|error| format!("{path}: {error}"))?
        .ok_or_else(|| format!("Decal material {path} was not found"))?;
    let supported = match material.shader.as_str() {
        "lightmappedgeneric" | "unlitgeneric" => {
            if !material.compile_flag("$decal") {
                return Err(format!(
                    "{path}: {} decals require $decal 1",
                    material.shader
                ));
            }
            true
        }
        "decalmodulate" => true,
        _ => false,
    };
    let scale = material
        .get("$decalscale")
        .map_or(Ok(1.0), |value| value.trim().parse::<f32>().map_err(|_| ()))
        .ok()
        .filter(|scale| *scale > 0.0 && scale.is_normal())
        .ok_or_else(|| format!("{path}: $decalscale must be positive and finite"))?;
    let texture = material
        .get("$basetexture")
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| format!("{path}: decal material requires $basetexture"))?;
    asset_relative(texture.trim()).map_err(|error| format!("{path}: $basetexture: {error}"))?;
    let texture = vmt::texture_path(texture);
    let data = read(&texture, PakLimits::default().max_entry_bytes)
        .map_err(|error| format!("{texture}: {error}"))?
        .ok_or_else(|| format!("{path}: decal base texture {texture} was not found"))?;
    let image = Vtf::parse(data).map_err(|error| format!("{texture}: {error}"))?;
    if image.version.1 > MAX_NATIVE_VTF_MINOR {
        return Err(format!(
            "{texture}: Counter-Strike: Source supports VTF 7.0 through 7.4"
        ));
    }
    if image.authored_frames != 1 || image.faces != 1 || image.depth != 1 {
        return Err(format!(
            "{texture}: decal textures must be static 2D images"
        ));
    }
    if [image.width, image.height]
        .into_iter()
        .any(|size| !(1.0..i32::MAX as f32).contains(&(size as f32 * scale)))
    {
        return Err(format!(
            "{path}: $decalscale produces invalid native decal dimensions"
        ));
    }
    image
        .validate_native_mips()
        .map_err(|error| format!("{texture}: {error}"))?;
    Ok((!supported).then(|| format!(
        "{path}: checked decal placement supports LightmappedGeneric, UnlitGeneric and DecalModulate"
    )))
}

pub(crate) fn validate(
    vmf: &Vmf,
    game: &GameFs,
    sources: &BuildMaterialSources,
    report: &mut Report,
    control: &Control,
) -> Result<(), String> {
    let mut materials = BTreeMap::new();
    let mut bytes_left = PakLimits::default().max_total_bytes;
    for entity in &vmf.entities {
        control.checkpoint()?;
        if entity.hidden || !entity.classname().is_some_and(is_class) {
            continue;
        }
        let origin = entity
            .origin()
            .filter(|point| crate::map::within_compile_bounds(*point));
        let mut textures = entity.properties.get_all(TEXTURE_KEY);
        let result = if origin.is_none() {
            Err("infodecal requires a finite origin within Source's compile bounds".into())
        } else if !entity.solids.is_empty() {
            Err("infodecal is a point entity; remove its brush geometry".into())
        } else if let Some(texture) = textures.next()
            && textures.next().is_none()
        {
            texture_name(texture).and_then(|name| {
                materials
                    .entry(name.clone())
                    .or_insert_with(|| {
                        check_material(&name, &mut |path, limit| {
                            control.checkpoint().map_err(io::Error::other)?;
                            let asset = sources.read_asset(game, path, limit.min(bytes_left))?;
                            Ok(asset.map(|asset| {
                                bytes_left -= asset.bytes().len();
                                asset.bytes().to_vec()
                            }))
                        })
                    })
                    .clone()
            })
        } else {
            Err("infodecal requires exactly one texture property".into())
        };
        let object = || Some(format!("entity:{}", entity.id));
        match result {
            Err(error) => report
                .diagnostics
                .push(Diagnostic::error(error, object(), origin)),
            Ok(Some(message)) => report.diagnostics.push(Diagnostic::warning(
                format!(
                    "{message}; passing the authored infodecal to the game with its mounted assets"
                ),
                object(),
                origin,
            )),
            Ok(None) => {}
        }
    }
    control.checkpoint()
}
