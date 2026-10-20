use std::io;

use serde::Serialize;
use source_formats::{
    gamefs::{AssetOrigin, asset_relative},
    vmt::{Material, material_path},
};

use super::Session;

pub const MAX_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_TOTAL_BYTES: usize = 4 * MAX_FILE_BYTES;
pub const MAX_SOURCE_BYTES: usize = 64 * 1024;
pub const MAX_FILES: usize = 9;
pub const MAX_PATH_BYTES: usize = 512;
const MAX_ORIGIN_CHARS: usize = 4096;
const MAX_ERROR_CHARS: usize = 2048;
const MAX_SHADER_CHARS: usize = 128;
const MAX_TEXTURE_CHARS: usize = 512;
const TEXTURE_PARAMETERS: [&str; 13] = [
    "%tooltexture",
    "$basetexture",
    "$albedo",
    "$basetexture2",
    "$bumpmap",
    "$normalmap",
    "$detail",
    "$envmap",
    "$envmapmask",
    "$blendmodulatetexture",
    "$flowmap",
    "$phongexponenttexture",
    "$lightwarptexture",
];

#[derive(Debug, Serialize)]
pub struct Origin {
    pub kind: &'static str,
    pub mount: String,
    pub file: Option<String>,
    pub storage_files: Vec<String>,
    pub paths_truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct SourceFile {
    pub path: String,
    pub origin: Option<Origin>,
    pub bytes: usize,
}

#[derive(Debug, Serialize)]
pub struct TextureParameter {
    pub parameter: &'static str,
    pub value: String,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct Resolved {
    pub shader: String,
    pub shader_truncated: bool,
    pub texture_parameters: Vec<TextureParameter>,
}

#[derive(Debug, Serialize)]
pub struct Limits {
    pub file_bytes: usize,
    pub total_bytes: usize,
    pub source_bytes: usize,
    pub files: usize,
    pub path_bytes: usize,
    pub origin_characters: usize,
    pub error_characters: usize,
    pub shader_characters: usize,
    pub texture_characters: usize,
    pub texture_parameters: usize,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub material: String,
    pub vmt_path: String,
    pub files: Vec<SourceFile>,
    pub raw_source: Option<String>,
    pub raw_source_bytes: usize,
    pub raw_source_truncated: bool,
    pub resolved: Option<Resolved>,
    pub error: Option<String>,
    pub error_truncated: bool,
    pub limits: Limits,
}

fn bounded(text: &str, maximum: usize) -> (String, bool) {
    (
        text.chars().take(maximum).collect(),
        text.chars().nth(maximum).is_some(),
    )
}

fn origin(source: AssetOrigin) -> Origin {
    let (kind, mount, file, files) = match source {
        AssetOrigin::Directory { mount, file } => ("directory", mount, Some(file), Vec::new()),
        AssetOrigin::Vpk { index, files } => ("vpk", index, None, files),
    };
    let (mount, mut truncated) = bounded(&mount.to_string_lossy(), MAX_ORIGIN_CHARS);
    let file = file.map(|file| {
        let (file, cut) = bounded(&file.to_string_lossy(), MAX_ORIGIN_CHARS);
        truncated |= cut;
        file
    });
    truncated |= files.len() > MAX_FILES;
    let storage_files = files
        .into_iter()
        .take(MAX_FILES)
        .map(|file| {
            let (file, cut) = bounded(&file.to_string_lossy(), MAX_ORIGIN_CHARS);
            truncated |= cut;
            file
        })
        .collect();
    Origin {
        kind,
        mount,
        file,
        storage_files,
        paths_truncated: truncated,
    }
}

fn path(name: &str) -> Result<(String, String), String> {
    if name.len() > MAX_PATH_BYTES {
        return Err(format!("Material path exceeds {MAX_PATH_BYTES} bytes"));
    }
    let name = name.trim();
    if name.ends_with(['/', '\\']) {
        return Err("Choose a material file rather than a directory".into());
    }
    let relative = asset_relative(name).map_err(|error| error.to_string())?;
    let material = relative.strip_prefix("materials/").unwrap_or(&relative);
    let material = material.strip_suffix(".vmt").unwrap_or(material);
    if material.is_empty() {
        return Err("Choose a material path first".into());
    }
    let vmt = if relative.starts_with("materials/") && relative.ends_with(".vmt") {
        relative.clone()
    } else {
        material_path(material)
    };
    if vmt.len() > MAX_PATH_BYTES {
        return Err(format!("Material VMT path exceeds {MAX_PATH_BYTES} bytes"));
    }
    Ok((material.to_owned(), vmt))
}

impl Session {
    pub fn inspect_material_source(&self, name: &str) -> Result<Report, String> {
        let (material, vmt_path) = path(name)?;
        let mut report = Report {
            material,
            vmt_path: vmt_path.clone(),
            files: Vec::new(),
            raw_source: None,
            raw_source_bytes: 0,
            raw_source_truncated: false,
            resolved: None,
            error: None,
            error_truncated: false,
            limits: Limits {
                file_bytes: MAX_FILE_BYTES,
                total_bytes: MAX_TOTAL_BYTES,
                source_bytes: MAX_SOURCE_BYTES,
                files: MAX_FILES,
                path_bytes: MAX_PATH_BYTES,
                origin_characters: MAX_ORIGIN_CHARS,
                error_characters: MAX_ERROR_CHARS,
                shader_characters: MAX_SHADER_CHARS,
                texture_characters: MAX_TEXTURE_CHARS,
                texture_parameters: TEXTURE_PARAMETERS.len(),
            },
        };
        let mut total = 0;
        let mut reads = 0;
        let result = Material::load_with_reader(&vmt_path, &mut |path| {
            reads += 1;
            if reads > MAX_FILES {
                return Err(io::Error::other(
                    "Material inspection exceeds its file lookup limit",
                ));
            }
            if path.len() > MAX_PATH_BYTES {
                return Err(io::Error::other(
                    "Included VMT path exceeds the inspection limit",
                ));
            }
            let path = asset_relative(path)?;
            let asset = self
                .game
                .read_asset_limited(&path, MAX_FILE_BYTES.min(MAX_TOTAL_BYTES - total))
                .map_err(|error| io::Error::new(error.kind(), format!("{path}: {error}")))?;
            let Some(asset) = asset else {
                report.files.push(SourceFile {
                    path,
                    origin: None,
                    bytes: 0,
                });
                return Ok(None);
            };
            total += asset.data.len();
            report.files.push(SourceFile {
                path: path.clone(),
                origin: Some(origin(asset.origin)),
                bytes: asset.data.len(),
            });
            if path == vmt_path && report.raw_source.is_none() {
                report.raw_source_bytes = asset.data.len();
            }
            let text = std::str::from_utf8(&asset.data)
                .map_err(|_| io::Error::other(format!("{path}: VMT source is not valid UTF-8")))?;
            if path == vmt_path && report.raw_source.is_none() {
                let mut end = text.len().min(MAX_SOURCE_BYTES);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                report.raw_source = Some(text[..end].to_owned());
                report.raw_source_truncated = end < text.len();
            }
            Ok(Some(asset.data))
        });
        match result {
            Ok(Some(material)) => {
                let (shader, shader_truncated) = bounded(&material.shader, MAX_SHADER_CHARS);
                report.resolved = Some(Resolved {
                    shader,
                    shader_truncated,
                    texture_parameters: TEXTURE_PARAMETERS
                        .into_iter()
                        .filter_map(|parameter| {
                            material.get(parameter).map(|value| {
                                let (value, truncated) = bounded(value, MAX_TEXTURE_CHARS);
                                TextureParameter {
                                    parameter,
                                    value,
                                    truncated,
                                }
                            })
                        })
                        .collect(),
                });
            }
            Ok(None) => {
                report.error = Some("Material VMT was not found in the mounted game content".into())
            }
            Err(error) => {
                let (error, truncated) = bounded(&error.to_string(), MAX_ERROR_CHARS);
                report.error = Some(error);
                report.error_truncated = truncated;
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests;
