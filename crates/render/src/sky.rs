use source_formats::gamefs::GameFs;
use source_formats::vmt::{Material, texture_path};
use source_formats::vtf::Vtf;

pub const SUFFIXES: [&str; 6] = ["rt", "bk", "lf", "ft", "up", "dn"];
const MAX_TEXTURE_BYTES: usize = 64 * 1024 * 1024;
const MAX_TEXTURE_SIZE: u32 = 4096;

#[derive(Debug)]
pub struct Face {
    pub material: String,
    pub texture: Option<Vtf>,
    pub issue: Option<String>,
    pub transform: [f32; 4],
}

#[derive(Debug)]
pub struct Skybox {
    pub name: String,
    pub faces: [Face; 6],
}

impl Skybox {
    pub fn load(fs: &GameFs, name: &str) -> Self {
        let name = name.trim().to_ascii_lowercase();
        Self {
            faces: SUFFIXES.map(|suffix| Face::load(fs, &format!("skybox/{name}{suffix}"))),
            name,
        }
    }

    pub fn ready(&self) -> bool {
        self.faces.iter().all(|face| face.texture.is_some())
    }

    pub fn summary(&self) -> String {
        if self.ready() {
            return format!("{} · six LDR faces ready", self.name);
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
    fn load(fs: &GameFs, name: &str) -> Self {
        let result = load_texture(fs, name);
        let (texture, issue) = match result {
            Ok((texture, transform)) => {
                return Self {
                    material: name.to_owned(),
                    texture: Some(texture),
                    issue: None,
                    transform,
                };
            }
            Err(issue) => (None, Some(issue)),
        };
        Self {
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
