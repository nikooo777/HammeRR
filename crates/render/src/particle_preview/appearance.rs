use source_formats::gamefs::{AssetOrigin, GameFs, asset_relative};
use source_formats::keyvalues;
use source_formats::vmt::{Material, material_path, texture_path};
use source_formats::vtf::Vtf;

use crate::icons::Blend;
use crate::materials::{MaterialInfo, validate_static_color_texture};

const MAX_PATH_BYTES: usize = 256;
const VTF_RESOURCE_TABLE: usize = 80;
const SHEET_RESOURCE: [u8; 3] = [0x10, 0, 0];
const REQUIRED_FLAGS: [&str; 3] = ["$vertexcolor", "$vertexalpha", "$translucent"];
const ADDITIVE_FLAGS: [&str; 2] = ["$additive", "$nocull"];

#[derive(Debug)]
pub(crate) struct Appearance {
    pub(crate) material: MaterialInfo,
    pub(crate) blend: Blend,
    pub(crate) clamp_edges: bool,
}

impl Appearance {
    pub(crate) fn load(fs: &GameFs, material: &str) -> Result<Self, String> {
        let name = bounded_asset_name(material)
            .map_err(|error| format!("Particle material path: {error}"))?;
        let path = material_path(&name);
        let mounted = fs
            .read_asset_limited(&path, crate::icons::MAX_MATERIAL_BYTES)
            .map_err(|error| format!("Particle material {path}: {error}"))?
            .ok_or_else(|| format!("Particle material {path} was not found"))?;
        let material_source = mounted_source(&path, &mounted.origin);
        let text = std::str::from_utf8(&mounted.data)
            .map_err(|error| format!("Particle material {material_source}: {error}"))?;
        let (material, blend) = parse_material(text)
            .map_err(|error| format!("Particle material {material_source}: {error}"))?;
        let texture_name = material.get("$basetexture").unwrap_or_default();
        let texture_name = bounded_asset_name(texture_name).map_err(|error| {
            format!("Particle material {material_source} $basetexture: {error}")
        })?;
        let texture_path = texture_path(&texture_name);
        if texture_path
            .rsplit('/')
            .next()
            .is_some_and(|filename| filename.starts_with("_rt_"))
        {
            return Err(format!(
                "Particle material {material_source} uses live render target {texture_path}"
            ));
        }
        let mounted = fs
            .read_asset_limited(&texture_path, crate::icons::MAX_TEXTURE_BYTES)
            .map_err(|error| format!("Particle texture {texture_path}: {error}"))?
            .ok_or_else(|| format!("Particle texture {texture_path} was not found"))?;
        let texture_source = mounted_source(&texture_path, &mounted.origin);
        let sheet = has_sheet_resource(&mounted.data);
        let texture = Vtf::parse(mounted.data)
            .map_err(|error| format!("Particle texture {texture_source}: {error}"))?;
        if sheet {
            return Err(format!(
                "Particle texture {texture_source} has an unsupported VTF sprite sheet"
            ));
        }
        let clamp_edges = validate_static_color_texture(&texture, crate::icons::MAX_AXIS, true)
            .and_then(|clamp_edges| {
                texture
                    .validate_native_mips()
                    .map(|()| clamp_edges)
                    .map_err(|error| error.to_string())
            })
            .map_err(|error| format!("Particle texture {texture_source}: {error}"))?;
        Ok(Self {
            material: MaterialInfo::from_resolved(
                &path,
                Some(&material),
                Some(texture),
                None,
                Some(texture_path),
            ),
            blend,
            clamp_edges,
        })
    }
}

fn parse_material(text: &str) -> Result<(Material, Blend), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let roots =
        keyvalues::parse_static_material_bounded(text, 2, 32).map_err(|error| error.to_string())?;
    let [root] = roots.as_slice() else {
        return Err("Requires one direct UnlitGeneric block".into());
    };
    let children = root
        .children()
        .filter(|_| root.key.eq_ignore_ascii_case("unlitgeneric"))
        .ok_or("Requires one direct UnlitGeneric block")?;
    let mut params = Vec::with_capacity(REQUIRED_FLAGS.len() + ADDITIVE_FLAGS.len() + 1);
    for node in children {
        let key = node.key.to_ascii_lowercase();
        let value = node
            .as_str()
            .ok_or("Particle material parameters must be scalar")?;
        if key != "$basetexture"
            && (value != "1"
                || !(REQUIRED_FLAGS.contains(&key.as_str())
                    || ADDITIVE_FLAGS.contains(&key.as_str())))
        {
            return Err(format!(
                "Unsupported particle material parameter {}",
                key.chars().take(128).collect::<String>()
            ));
        }
        if params.iter().any(|(previous, _)| previous == &key) {
            return Err(format!("Duplicate particle material parameter {key}"));
        }
        params.push((key, value.to_owned()));
    }
    let material = Material {
        shader: "unlitgeneric".into(),
        params,
        blocks: Vec::new(),
    };
    if material.get("$basetexture").is_none()
        || REQUIRED_FLAGS.iter().any(|key| material.get(key).is_none())
    {
        return Err(
            "Requires $basetexture and explicit 1 for $vertexcolor, $vertexalpha and $translucent"
                .into(),
        );
    }
    let blend = match (material.get("$additive"), material.get("$nocull")) {
        (None, None) => Blend::Alpha,
        (Some(_), Some(_)) => Blend::Additive,
        _ => {
            return Err(
                "Particle additive materials require both $additive 1 and $nocull 1".into(),
            );
        }
    };
    Ok((material, blend))
}

fn bounded_asset_name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.len() > MAX_PATH_BYTES {
        return Err(format!("Mounted asset name exceeds {MAX_PATH_BYTES} bytes"));
    }
    let path = asset_relative(value).map_err(|error| error.to_string())?;
    if path.is_empty() {
        return Err("Requires a mounted asset name".into());
    }
    Ok(path)
}

fn mounted_source(path: &str, origin: &AssetOrigin) -> String {
    let source = match origin {
        AssetOrigin::Directory { file, .. } => file,
        AssetOrigin::Vpk { index, .. } => index,
    };
    format!("{path} ({})", source.display())
}

fn has_sheet_resource(data: &[u8]) -> bool {
    let read_u32 = |offset| {
        data.get(offset..offset + 4)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u32::from_le_bytes)
    };
    let Some(((minor, header_size), count)) = read_u32(8).zip(read_u32(12)).zip(read_u32(68))
    else {
        return false;
    };
    let header_size = header_size as usize;
    if minor < 3
        || header_size < VTF_RESOURCE_TABLE
        || count as usize > (header_size - VTF_RESOURCE_TABLE) / 8
    {
        return false;
    }
    data.get(VTF_RESOURCE_TABLE..header_size)
        .is_some_and(|table| {
            table
                .as_chunks::<8>()
                .0
                .iter()
                .take(count as usize)
                .any(|entry| entry.starts_with(&SHEET_RESOURCE))
        })
}

pub(crate) fn quad(center: [f32; 3], radius: f32, rotation: f32) -> Result<[[f32; 3]; 4], String> {
    if !radius.is_finite() || radius <= 0.0 || !rotation.is_finite() {
        return Err("Particle quad requires finite rotation and a positive finite radius".into());
    }
    let (sine, cosine) = (-rotation).sin_cos();
    let corners = [[-1.0, 1.0], [1.0, 1.0], [1.0, -1.0], [-1.0, -1.0]].map(|[u, v]| {
        [
            center[0] + radius * (cosine * u + sine * v),
            center[1] + radius * (sine * u - cosine * v),
            center[2],
        ]
    });
    if !corners
        .iter()
        .flatten()
        .all(|coordinate| coordinate.is_finite())
    {
        return Err("Particle quad corners must be finite".into());
    }
    Ok(corners)
}
