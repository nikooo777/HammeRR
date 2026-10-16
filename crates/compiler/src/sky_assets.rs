use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use serde::Serialize;
use source_formats::gamefs::{AssetOrigin, GameFs, asset_relative};
use source_formats::vmf::Vmf;
use source_formats::vmt::{self, Material};
use source_formats::vtf::{ImageFormat, Vtf};

mod closure;
mod hdr_preview;
pub use hdr_preview::{HdrSkyFace, MAX_HDR_PREVIEW_TEXTURE_BYTES, load_hdr_sky_preview};
mod collection;
mod manifest;
pub use collection::{
    SkyAssetPlan, SkyAssetSourceGuard, SkyAssetSourceSnapshot, collect_sky_asset_plan,
};
pub use manifest::{
    PACKED_SKY_MANIFEST_PATH, SkyAssetManifest, SkyPackedAsset, read_packed_sky, verify_packed_sky,
};

const MAX_ASSETS: usize = 128;
const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_MATERIAL_BYTES: usize = 1024 * 1024;
const MAX_TEXTURE_SIZE: u32 = 4096;
const MAX_FAMILY_BYTES: usize = 256;
const MAX_PATH_BYTES: usize = 512;
const MAX_ISSUE_CHARACTERS: usize = 1024;
const MAX_ISSUES: usize = 16;
const MAX_SHADER_CHARACTERS: usize = 128;
const SUFFIXES: [&str; 6] = ["rt", "bk", "lf", "ft", "up", "dn"];
const HDR_PARAMETERS: [&str; 5] = [
    "$hdrbasetexture",
    "$hdrcompressedtexture",
    "$hdrcompressedtexture0",
    "$hdrcompressedtexture1",
    "$hdrcompressedtexture2",
];

#[derive(Debug, Clone, Serialize)]
pub struct SkyAssetsReport {
    pub readiness_scope: &'static str,
    pub authored_name: Option<String>,
    pub authored_name_truncated: bool,
    pub report_truncated: bool,
    pub ldr_ready: bool,
    pub hdr_ready: bool,
    pub faces: Vec<SkyFaceReport>,
    pub issues: Vec<String>,
    pub assets_read: usize,
    pub bytes_read: usize,
    pub limits: SkyInspectionLimits,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkyInspectionLimits {
    pub asset_lookups: usize,
    pub total_bytes: usize,
    pub material_bytes: usize,
    pub texture_axis: u32,
    pub hdr_base_formats: [&'static str; 2],
    pub path_bytes: usize,
    pub issue_characters: usize,
    pub issues_per_scope: usize,
    pub shader_characters: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkyFaceReport {
    pub suffix: String,
    pub material: String,
    pub source: Option<SkyAssetSource>,
    pub material_sources: Vec<SkyMaterialSource>,
    pub shader: Option<String>,
    pub shader_truncated: bool,
    pub ldr: SkyChannelReport,
    pub hdr: SkyChannelReport,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SkyChannelReport {
    pub ready: bool,
    pub textures: Vec<SkyTextureReport>,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkyTextureReport {
    pub parameter: String,
    pub reference: String,
    pub reference_truncated: bool,
    pub path: String,
    pub source: Option<SkyAssetSource>,
    pub size: Option<[u32; 2]>,
    pub format: Option<String>,
    pub bytes: usize,
    pub ready: bool,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkyMaterialSource {
    pub path: String,
    pub source: SkyAssetSource,
    pub bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkyAssetSource {
    Directory { mount: PathBuf, file: PathBuf },
    Vpk { index: PathBuf, files: Vec<PathBuf> },
}

impl From<&AssetOrigin> for SkyAssetSource {
    fn from(origin: &AssetOrigin) -> Self {
        match origin {
            AssetOrigin::Directory { mount, file } => Self::Directory {
                mount: mount.clone(),
                file: file.clone(),
            },
            AssetOrigin::Vpk { index, files } => Self::Vpk {
                index: index.clone(),
                files: files.clone(),
            },
        }
    }
}

pub fn inspect_sky_assets(
    vmf: &Vmf,
    game: &GameFs,
    availability_error: Option<&str>,
) -> SkyAssetsReport {
    let mut names = vmf.world.properties.get_all("skyname");
    let mut report = inspect(game, names.next());
    if names.next().is_some() {
        report.issues.push(
            "World has duplicate skyname keys; resolve them before relying on this report".into(),
        );
    }
    if let Some(error) = availability_error {
        report
            .issues
            .push(format!("Game content is unavailable: {error}"));
    }
    if !report.issues.is_empty() {
        report.ldr_ready = false;
        report.hdr_ready = false;
    }
    bound_report(&mut report);
    report
}

#[derive(Debug)]
struct Asset {
    data: Vec<u8>,
    source: SkyAssetSource,
}

struct Reader<'a> {
    game: &'a GameFs,
    assets: BTreeMap<String, Result<Option<Asset>, String>>,
    bytes: usize,
}

impl<'a> Reader<'a> {
    fn new(game: &'a GameFs) -> Self {
        Self {
            game,
            assets: BTreeMap::new(),
            bytes: 0,
        }
    }

    fn read(&mut self, path: &str, maximum: usize) -> Result<Option<&Asset>, String> {
        if path.len() > MAX_PATH_BYTES {
            return Err(format!(
                "Sky dependency path exceeds {MAX_PATH_BYTES} bytes"
            ));
        }
        let path = asset_relative(path).map_err(|error| error.to_string())?;
        if !self.assets.contains_key(&path) {
            if self.assets.len() >= MAX_ASSETS {
                return Err(format!("Sky inspection exceeds {MAX_ASSETS} asset lookups"));
            }
            let remaining = MAX_TOTAL_BYTES.saturating_sub(self.bytes);
            let result = self
                .game
                .read_asset_limited(&path, maximum.min(remaining))
                .map(|asset| {
                    asset.map(|asset| {
                        self.bytes += asset.data.len();
                        Asset {
                            source: SkyAssetSource::from(&asset.origin),
                            data: asset.data,
                        }
                    })
                })
                .map_err(|error| {
                    format!(
                        "{path}: {error}; sky inspection allows 64 MiB total and 1 MiB per material"
                    )
                });
            self.assets.insert(path.clone(), result);
        }
        match self.assets.get(&path).unwrap() {
            Ok(Some(asset)) if asset.data.len() > maximum => {
                Err(format!("{path}: asset exceeds its byte read limit"))
            }
            Ok(asset) => Ok(asset.as_ref()),
            Err(error) => Err(error.clone()),
        }
    }

    fn material(&mut self, path: &str) -> (Result<Material, String>, Vec<SkyMaterialSource>) {
        let mut sources = Vec::new();
        let result = Material::load_with_reader(path, &mut |path| {
            let asset = self
                .read(path, MAX_MATERIAL_BYTES)
                .map_err(io::Error::other)?;
            Ok(asset.map(|asset| {
                if !sources.iter().any(|source: &SkyMaterialSource| {
                    source.path == source_formats::gamefs::normalize(path)
                }) {
                    sources.push(SkyMaterialSource {
                        path: source_formats::gamefs::normalize(path),
                        source: asset.source.clone(),
                        bytes: asset.data.len(),
                    });
                }
                asset.data.clone()
            }))
        })
        .map_err(|error| error.to_string())
        .and_then(|material| material.ok_or_else(|| format!("Missing sky material: {path}")));
        (result, sources)
    }

    fn texture(&mut self, parameter: &str, reference: &str, hdr: bool) -> SkyTextureReport {
        let (display_reference, reference_truncated) = bounded(reference, MAX_PATH_BYTES);
        let mut report = SkyTextureReport {
            parameter: parameter.into(),
            reference: display_reference,
            reference_truncated,
            path: String::new(),
            source: None,
            size: None,
            format: None,
            bytes: 0,
            ready: false,
            issues: Vec::new(),
        };
        if reference.len() > MAX_PATH_BYTES {
            report.issues.push(format!(
                "Texture reference exceeds {MAX_PATH_BYTES} bytes; no asset was looked up"
            ));
            return report;
        }
        report.path = vmt::texture_path(reference);
        if reference.trim().is_empty()
            || reference.starts_with('_')
            || reference.eq_ignore_ascii_case("env_cubemap")
        {
            report.issues.push(format!(
                "{parameter} must name a static mounted sky texture"
            ));
            return report;
        }
        if let Err(error) = asset_relative(reference) {
            report.issues.push(format!("{parameter}: {error}"));
            return report;
        }
        let asset = match self.read(&report.path, MAX_TOTAL_BYTES) {
            Ok(Some(asset)) => asset,
            Ok(None) => {
                report
                    .issues
                    .push(format!("Missing texture: {}", report.path));
                return report;
            }
            Err(error) => {
                report.issues.push(error);
                return report;
            }
        };
        report.source = Some(asset.source.clone());
        report.bytes = asset.data.len();
        let texture = match Vtf::parse(asset.data.clone()) {
            Ok(texture) => texture,
            Err(error) => {
                report.issues.push(format!("{}: {error}", report.path));
                return report;
            }
        };
        report.size = Some([texture.width, texture.height]);
        report.format = Some(format!("{:?}", texture.format));
        report.issues = texture_issues(&texture, parameter, hdr);
        report.ready = report.issues.is_empty();
        report
    }

    fn face(&mut self, name: &str, suffix: &str) -> SkyFaceReport {
        let material_name = format!("skybox/{name}{suffix}");
        let path = vmt::material_path(&material_name);
        let (material, sources) = self.material(&path);
        let mut report = SkyFaceReport {
            suffix: suffix.into(),
            material: material_name,
            source: sources.first().map(|source| source.source.clone()),
            material_sources: sources,
            shader: None,
            shader_truncated: false,
            ldr: SkyChannelReport::default(),
            hdr: SkyChannelReport::default(),
            issues: Vec::new(),
        };
        let material = match material {
            Ok(material) => material,
            Err(error) => {
                report.issues.push(error);
                return report;
            }
        };
        let (shader, truncated) = bounded(&material.shader, MAX_SHADER_CHARACTERS);
        report.shader = Some(shader);
        report.shader_truncated = truncated;
        if !matches!(material.shader.as_str(), "sky" | "unlitgeneric") {
            report
                .issues
                .push(format!("Unsupported sky shader: {}", material.shader));
        }
        if !material.blocks.is_empty() {
            report.issues.push(
                "Nested sky material effects, animation and conditional branches are unsupported"
                    .into(),
            );
        }
        for (key, _) in &material.params {
            if !supported_parameter(key) {
                report
                    .issues
                    .push(format!("Unsupported sky material parameter: {key}"));
            }
        }
        if let Some(reference) = material.get("$basetexture") {
            report
                .ldr
                .textures
                .push(self.texture("$basetexture", reference, false));
        } else {
            report
                .ldr
                .issues
                .push("No LDR $basetexture is declared".into());
        }
        for parameter in HDR_PARAMETERS {
            if let Some(reference) = material.get(parameter) {
                report
                    .hdr
                    .textures
                    .push(self.texture(parameter, reference, true));
                if parameter != "$hdrbasetexture" {
                    report.hdr.issues.push(format!("{parameter}: compressed HDR sky encodings are unsupported by this inspection"));
                }
            }
        }
        if material.shader != "sky" {
            report
                .hdr
                .issues
                .push("HDR channel inspection requires the Sky shader".into());
        }
        if report.hdr.textures.is_empty() {
            report.hdr.issues.push(
                "No HDR sky texture is declared; an _hdr family is not selected automatically"
                    .into(),
            );
        }
        report.ldr.ready = report.issues.is_empty()
            && report.ldr.issues.is_empty()
            && !report.ldr.textures.is_empty()
            && report.ldr.textures.iter().all(|texture| texture.ready);
        report.hdr.ready = report.issues.is_empty()
            && report.hdr.issues.is_empty()
            && !report.hdr.textures.is_empty()
            && report.hdr.textures.iter().all(|texture| texture.ready);
        report
    }
}

fn texture_issues(texture: &Vtf, parameter: &str, hdr: bool) -> Vec<String> {
    let mut issues = Vec::new();
    if texture.width > MAX_TEXTURE_SIZE || texture.height > MAX_TEXTURE_SIZE {
        issues.push(format!(
            "Texture exceeds {MAX_TEXTURE_SIZE} pixels per axis"
        ));
    }
    if texture.frames != 1 || texture.faces != 1 || texture.depth != 1 {
        issues.push("Animated, cubemap and volume sky textures are unsupported".into());
    }
    if hdr && parameter == "$hdrbasetexture" {
        if !matches!(
            texture.format,
            ImageFormat::Rgba16161616F | ImageFormat::Rgba16161616
        ) {
            issues.push(
                "This inspection supports HDR base textures only in RGBA16F or RGBA16 format"
                    .into(),
            );
        }
    } else if !hdr && texture.format == ImageFormat::Rgba16161616F {
        issues.push("This inspection does not support RGBA16F as an LDR base texture".into());
    }
    issues
}

fn supported_parameter(key: &str) -> bool {
    HDR_PARAMETERS.contains(&key)
        || matches!(
            key,
            "$basetexture"
                | "$basetexturetransform"
                | "$nofog"
                | "$ignorez"
                | "$surfaceprop"
                | "$surfaceprop2"
                | "$srgb"
                | "%tooltexture"
                | "%keywords"
        )
}

fn inspect(game: &GameFs, name: Option<&str>) -> SkyAssetsReport {
    let display_name = name.map(|name| bounded(name, MAX_FAMILY_BYTES));
    let authored_name_truncated = display_name
        .as_ref()
        .is_some_and(|(_, truncated)| *truncated);
    let mut report = SkyAssetsReport {
        readiness_scope: "Mounted material dependencies and VTF layout only; editor preview support, HDR pixel radiance and the game's active channel are not established",
        authored_name: display_name.as_ref().map(|(name, _)| name.clone()),
        authored_name_truncated,
        report_truncated: authored_name_truncated,
        ldr_ready: false,
        hdr_ready: false,
        faces: Vec::new(),
        issues: Vec::new(),
        assets_read: 0,
        bytes_read: 0,
        limits: SkyInspectionLimits {
            asset_lookups: MAX_ASSETS,
            total_bytes: MAX_TOTAL_BYTES,
            material_bytes: MAX_MATERIAL_BYTES,
            texture_axis: MAX_TEXTURE_SIZE,
            hdr_base_formats: ["Rgba16161616F", "Rgba16161616"],
            path_bytes: MAX_PATH_BYTES,
            issue_characters: MAX_ISSUE_CHARACTERS,
            issues_per_scope: MAX_ISSUES,
            shader_characters: MAX_SHADER_CHARACTERS,
        },
    };
    let Some(name) = name.filter(|name| !name.is_empty()) else {
        report
            .issues
            .push("Set world skyname to inspect a mounted sky family".into());
        return report;
    };
    if name.len() > MAX_FAMILY_BYTES
        || name.trim() != name
        || name.chars().any(char::is_control)
        || asset_relative(name).is_err()
    {
        report.issues.push(format!("skyname must be a relative family name of at most {MAX_FAMILY_BYTES} bytes without surrounding whitespace or control characters"));
        return report;
    }
    let mut reader = Reader::new(game);
    report.faces = SUFFIXES.map(|suffix| reader.face(name, suffix)).into();
    report.ldr_ready = report.faces.iter().all(|face| face.ldr.ready);
    report.hdr_ready = report.faces.iter().all(|face| face.hdr.ready);
    report.bytes_read = reader.bytes;
    report.assets_read = reader
        .assets
        .values()
        .filter(|asset| matches!(asset, Ok(Some(_))))
        .count();
    bound_report(&mut report);
    report
}

fn bounded(text: &str, maximum: usize) -> (String, bool) {
    (
        text.chars().take(maximum).collect(),
        text.chars().nth(maximum).is_some(),
    )
}

fn bound_issues(issues: &mut Vec<String>) -> bool {
    let mut truncated = issues.len() > MAX_ISSUES;
    if truncated {
        let omitted = issues.len() - (MAX_ISSUES - 1);
        issues.truncate(MAX_ISSUES - 1);
        issues.push(format!("{omitted} additional issues omitted"));
    }
    for issue in issues {
        let (text, cut) = bounded(issue, MAX_ISSUE_CHARACTERS);
        *issue = text;
        if cut {
            issue.push_str(" [truncated]");
        }
        truncated |= cut;
    }
    truncated
}

fn bound_report(report: &mut SkyAssetsReport) {
    let truncated = bound_issues(&mut report.issues);
    report.report_truncated |= report.authored_name_truncated || truncated;
    for face in &mut report.faces {
        let truncated = bound_issues(&mut face.issues);
        report.report_truncated |= face.shader_truncated || truncated;
        for channel in [&mut face.ldr, &mut face.hdr] {
            report.report_truncated |= bound_issues(&mut channel.issues);
            for texture in &mut channel.textures {
                let truncated = bound_issues(&mut texture.issues);
                report.report_truncated |= texture.reference_truncated || truncated;
            }
        }
    }
}

#[cfg(test)]
mod tests;
