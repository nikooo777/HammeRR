use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use source_formats::{
    gamefs::{GameFs, asset_relative},
    keyvalues, vmt,
    vtf::Vtf,
};

use super::{
    MAX_MATERIAL_BYTES, MAX_PATH_BYTES, Reader, SUFFIXES, SkyAssetSource, supported_parameter,
    texture_issues,
};

pub const MAX_HDR_PREVIEW_TEXTURE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub struct HdrSkyFace {
    pub material: String,
    pub source: Option<SkyAssetSource>,
    pub texture_path: Option<String>,
    pub texture_source: Option<SkyAssetSource>,
    pub texture: Option<Arc<Vtf>>,
    pub transform: Option<String>,
    pub error: Option<String>,
}

pub fn load_hdr_sky_preview(game: &GameFs, family: &str) -> [HdrSkyFace; 6] {
    if let Err(error) = super::closure::validate_family(family) {
        let mut end = family.len().min(256);
        while !family.is_char_boundary(end) {
            end -= 1;
        }
        let preview = &family[..end];
        return SUFFIXES.map(|suffix| HdrSkyFace {
            material: format!("skybox/{preview}{suffix}"),
            source: None,
            texture_path: None,
            texture_source: None,
            texture: None,
            transform: None,
            error: Some(error.to_string()),
        });
    }
    let mut reader = Reader::new(game);
    let mut textures = BTreeMap::<String, Arc<Vtf>>::new();
    SUFFIXES.map(|suffix| {
        let mut face = HdrSkyFace {
            material: format!("skybox/{family}{suffix}"),
            source: None,
            texture_path: None,
            texture_source: None,
            texture: None,
            transform: None,
            error: None,
        };
        let result = load_face(&mut reader, &mut textures, family, &mut face);
        if let Err(error) = result {
            face.error = Some(error);
        }
        face
    })
}
fn load_face(
    reader: &mut Reader<'_>,
    textures: &mut BTreeMap<String, Arc<Vtf>>,
    family: &str,
    face: &mut HdrSkyFace,
) -> Result<(), String> {
    super::closure::validate_family(family).map_err(|error| error.to_string())?;
    let material_path = vmt::material_path(&face.material);
    let material = reader
        .read(&material_path, MAX_MATERIAL_BYTES)?
        .ok_or("Missing HDR sky material")?;
    face.source = Some(material.source.clone());
    let text =
        std::str::from_utf8(&material.data).map_err(|_| "HDR preview requires UTF-8 VMT source")?;
    let nodes = keyvalues::parse_bounded(text.trim_start_matches('\u{feff}'), 1, 64)
        .map_err(|error| format!("HDR preview requires complete strict VMT syntax: {error}"))?;
    if nodes.len() != 1 || !nodes[0].key.eq_ignore_ascii_case("Sky") {
        return Err("HDR preview requires six direct Sky materials; Patch and other shaders are unsupported".into());
    }
    let children = nodes[0]
        .children()
        .ok_or("Sky material requires a parameter block")?;
    let mut keys = HashSet::new();
    let mut reference = None;
    for node in children {
        let key = node.key.to_ascii_lowercase();
        let value = node
            .as_str()
            .ok_or("Nested HDR sky effects are unsupported")?;
        if key.starts_with("$hdrcompressedtexture") {
            return Err("Compressed HDR sky declarations are unsupported by this preview".into());
        }
        if !supported_parameter(&key) || !keys.insert(key.clone()) {
            return Err(format!(
                "Unsupported or duplicate HDR sky parameter {}",
                key.chars().take(128).collect::<String>()
            ));
        }
        if key == "$hdrbasetexture" {
            reference = Some(value);
        }
        if key == "$basetexturetransform" {
            face.transform = Some(value.to_owned());
        }
    }
    let reference = reference.ok_or("No HDR $hdrbasetexture; LDR is not substituted")?;
    if reference.is_empty()
        || reference.len() > MAX_PATH_BYTES
        || reference.trim() != reference
        || reference.chars().any(char::is_control)
        || reference.starts_with('_')
        || reference.eq_ignore_ascii_case("env_cubemap")
    {
        return Err("HDR texture reference must be an exact bounded static mounted path".into());
    }
    asset_relative(reference).map_err(|error| error.to_string())?;
    let path = vmt::texture_path(reference);
    face.texture_path = Some(path.clone());
    let asset = reader
        .read(&path, MAX_HDR_PREVIEW_TEXTURE_BYTES)?
        .ok_or("Missing HDR texture; LDR is not substituted")?;
    face.texture_source = Some(asset.source.clone());
    if let Some(texture) = textures.get(&path) {
        face.texture = Some(texture.clone());
        return Ok(());
    }
    if asset.data.get(24..26) == Some(&[0, 0])
        || (asset.data.get(8).is_some_and(|minor| *minor >= 2)
            && asset.data.get(63..65) == Some(&[0, 0]))
    {
        return Err("HDR preview requires nonzero authored frame/depth counts".into());
    }
    let texture = Vtf::parse(asset.data.clone()).map_err(|error| error.to_string())?;
    let issues = texture_issues(&texture, "$hdrbasetexture", true);
    if !issues.is_empty() {
        return Err(issues.join("; "));
    }
    let texture = Arc::new(texture);
    textures.insert(path, texture.clone());
    face.texture = Some(texture);
    Ok(())
}

#[cfg(test)]
mod tests;
