use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use source_formats::{
    gamefs::{AssetOrigin, GameFs, asset_relative},
    keyvalues,
    vmt::{material_path, texture_path},
    vtf::Vtf,
};

use crate::materials::MaterialInfo;

pub const MAX_ASSETS: usize = 64;
pub const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_INSTANCES: usize = 256;
pub const MAX_AXIS: u32 = 512;
pub const MAX_MATERIAL_BYTES: usize = 64 * 1024;
pub const MAX_TEXTURE_BYTES: usize = 4 * 1024 * 1024;
pub const LIMITS: &str = "Literal one-argument iconsprite only; direct Sprite material with vp_parallel and centered spriteorigin. Static single-frame, single-face, depth-one LDR VTF, at most 512 pixels per axis. Material 64 KiB, texture 4 MiB, 64 cached success/failure entries and 32 MiB encoded bytes per material generation, 256 visible icons. Independent editor image: 32 Source-unit long axis with texture aspect ratio, live camera right/up, unlit RGBA alpha, no fog. Native Sprite render modes and animation are not evaluated. Sprite resource guides report their separate positive-scale policy and share these asset/image budgets; models, specialized lights and ropes remain independent. Original box bounds, picking and framing remain unchanged. Reload material assets clears cached admission and failures.";
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    Directory { mount: String, file: String },
    Vpk { index: String, files: Vec<String> },
}
impl Source {
    pub fn summary(&self) -> String {
        let path = match self {
            Self::Directory { file, .. } => file,
            Self::Vpk { index, .. } => index,
        };
        let mut summary = path.chars().take(256).collect::<String>();
        if path.chars().nth(256).is_some() {
            summary.push('…');
        }
        summary
    }
}
impl From<AssetOrigin> for Source {
    fn from(value: AssetOrigin) -> Self {
        match value {
            AssetOrigin::Directory { mount, file } => Self::Directory {
                mount: mount.display().to_string(),
                file: file.display().to_string(),
            },
            AssetOrigin::Vpk { index, files } => Self::Vpk {
                index: index.display().to_string(),
                files: files
                    .iter()
                    .map(|file| file.display().to_string())
                    .collect(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub status: &'static str,
    pub material: String,
    pub source: Option<Source>,
    pub texture: Option<String>,
    pub texture_source: Option<Source>,
    pub width: u32,
    pub height: u32,
    pub world_size: [f32; 2],
    pub orientation_policy: &'static str,
    pub rendering_policy: &'static str,
    pub size_policy: &'static str,
    pub error: Option<String>,
    pub displayed: bool,
    pub suppression_reason: Option<&'static str>,
}
impl Report {
    pub fn failure(material: &str, status: &'static str, error: impl Into<String>) -> Self {
        Self {
            status,
            material: material.chars().take(256).collect(),
            source: None,
            texture: None,
            texture_source: None,
            width: 0,
            height: 0,
            world_size: [0.0; 2],
            orientation_policy: "live_camera_right_up",
            rendering_policy: "unlit_rgba_editor_image",
            size_policy: "32_source_unit_long_axis",
            error: Some(error.into().chars().take(512).collect()),
            displayed: false,
            suppression_reason: None,
        }
    }
}
#[derive(Debug)]
pub struct Asset {
    pub id: u64,
    pub report: Report,
    pub material: Option<MaterialInfo>,
}
#[derive(Debug, Clone)]
pub struct Instance {
    pub center: [f32; 3],
    pub world_size: [f32; 2],
    pub asset: Arc<Asset>,
}
#[derive(Debug, Default)]
struct Cache {
    entries: HashMap<String, Arc<Asset>>,
    bytes: usize,
}
#[derive(Debug, Default)]
pub struct Library(RefCell<Cache>);
impl Library {
    pub fn clear(&mut self) {
        self.0.get_mut().entries.clear();
        self.0.get_mut().bytes = 0;
    }
    pub fn load(&self, fs: &GameFs, name: &str) -> Arc<Asset> {
        if name.len() > 256 {
            return Arc::new(Asset {
                id: 0,
                report: Report::failure(name, "invalid", "Icon material path exceeds 256 bytes"),
                material: None,
            });
        }
        self.load_path(fs, name, canonical(name, false))
    }
    pub fn load_material_path(&self, fs: &GameFs, path: &str) -> Arc<Asset> {
        if path.len() > 256 {
            return Arc::new(Asset {
                id: 0,
                report: Report::failure(path, "invalid", "Editor image VMT path exceeds 256 bytes"),
                material: None,
            });
        }
        let canonical = asset_relative(path)
            .map_err(|error| error.to_string())
            .and_then(|path| {
                if path.len() > 256 || !path.starts_with("materials/") || !path.ends_with(".vmt") {
                    return Err("Editor image requires a bounded full virtual VMT path".into());
                }
                Ok(path)
            });
        self.load_path(fs, path, canonical)
    }
    fn load_path(&self, fs: &GameFs, name: &str, path: Result<String, String>) -> Arc<Asset> {
        let key = path
            .as_ref()
            .cloned()
            .unwrap_or_else(|_| format!("invalid:{}", name.to_ascii_lowercase()));
        let mut cache = self.0.borrow_mut();
        if let Some(asset) = cache.entries.get(&key) {
            return asset.clone();
        }
        if cache.entries.len() >= MAX_ASSETS || cache.bytes >= MAX_CACHE_BYTES {
            return Arc::new(Asset {
                id: 0,
                report: Report::failure(
                    name,
                    "unavailable",
                    "Icon asset cache limit reached; use Reload material assets to clear it",
                ),
                material: None,
            });
        }
        let mut report = Report::failure(name, "unavailable", "Icon asset unavailable");
        let result =
            path.and_then(|path| load(fs, &path, &mut report, MAX_CACHE_BYTES - cache.bytes));
        let (material, bytes) = match result {
            Ok((material, bytes)) => {
                report.status = "ready";
                report.error = None;
                (Some(material), bytes)
            }
            Err(error) => {
                report.error = Some(error.chars().take(512).collect());
                (None, 0)
            }
        };
        cache.bytes += bytes;
        let asset = Arc::new(Asset {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            report,
            material,
        });
        cache.entries.insert(key, asset.clone());
        asset
    }
}

fn canonical(name: &str, texture: bool) -> Result<String, String> {
    if name.len() > 256 || name.trim() != name || name.is_empty() {
        return Err(
            "Icon asset path must contain 1–256 bytes without surrounding whitespace".into(),
        );
    }
    asset_relative(name).map_err(|error| error.to_string())?;
    if name.to_ascii_lowercase().ends_with(".spr") {
        return Err("Legacy .spr aliases are outside literal iconsprite support".into());
    }
    let path = if texture {
        texture_path(name)
    } else {
        material_path(name)
    };
    asset_relative(&path).map_err(|error| error.to_string())
}
fn load(
    fs: &GameFs,
    name: &str,
    report: &mut Report,
    budget: usize,
) -> Result<(MaterialInfo, usize), String> {
    let path = name;
    report.material = path.into();
    let mounted = fs
        .read_asset_limited(path, MAX_MATERIAL_BYTES.min(budget))
        .map_err(|error| format!("{path}: {error}"))?
        .ok_or_else(|| format!("Missing icon material {path}"))?;
    report.source = Some(mounted.origin.into());
    let material_bytes = mounted.data.len();
    let text = std::str::from_utf8(&mounted.data).map_err(|_| "Icon material must be UTF-8")?;
    let nodes = keyvalues::parse_bounded(text, 2, 32)
        .map_err(|error| format!("Icon material requires complete bounded KeyValues: {error}"))?;
    if nodes.len() != 1 || !nodes[0].key.eq_ignore_ascii_case("Sprite") {
        return Err("Only a direct Sprite icon material is supported".into());
    }
    let children = nodes[0]
        .children()
        .ok_or("Sprite material requires a parameter block")?;
    let mut params = HashMap::new();
    let mut keys = HashSet::new();
    for node in children {
        let key = node.key.to_ascii_lowercase();
        if ![
            "$basetexture",
            "$spriteorientation",
            "$spriteorigin",
            "$no_fullbright",
        ]
        .contains(&key.as_str())
            || !keys.insert(key.clone())
        {
            return Err(format!(
                "Unsupported or duplicate Sprite parameter {}",
                node.key.chars().take(128).collect::<String>()
            ));
        }
        params.insert(
            key,
            node.as_str()
                .ok_or("Nested Sprite blocks are unsupported")?,
        );
    }
    if !params
        .get("$spriteorientation")
        .is_some_and(|value| value.eq_ignore_ascii_case("vp_parallel"))
    {
        return Err("Icon requires explicit $spriteorientation vp_parallel".into());
    }
    let origin = params
        .get("$spriteorigin")
        .ok_or("Icon requires explicit centered $spriteorigin")?
        .trim();
    let values = origin
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or("Icon requires bracketed centered $spriteorigin")?
        .split_whitespace()
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Invalid $spriteorigin")?;
    if !(values == [0.5, 0.5] || values == [0.5, 0.5, 0.0]) {
        return Err("Only centered $spriteorigin [0.5 0.5] is supported".into());
    }
    if params
        .get("$no_fullbright")
        .is_some_and(|value| *value != "1")
    {
        return Err("Only $no_fullbright 1 is admitted; editor icons remain unlit".into());
    }
    let texture_path = canonical(
        params
            .get("$basetexture")
            .ok_or("Icon material has no $basetexture")?,
        true,
    )?;
    report.texture = Some(texture_path.clone());
    let texture = fs
        .read_asset_limited(
            &texture_path,
            MAX_TEXTURE_BYTES.min(budget.saturating_sub(material_bytes)),
        )
        .map_err(|error| format!("{texture_path}: {error}"))?
        .ok_or_else(|| format!("Missing icon texture {texture_path}"))?;
    report.texture_source = Some(texture.origin.into());
    let bytes = material_bytes + texture.data.len();
    let vtf = Vtf::parse(texture.data).map_err(|error| format!("{texture_path}: {error}"))?;
    if vtf.authored_frames == 0 || vtf.authored_depth == Some(0) {
        return Err("Icon VTF has zero frames or depth".into());
    }
    if vtf.width == 0
        || vtf.height == 0
        || vtf.width > MAX_AXIS
        || vtf.height > MAX_AXIS
        || vtf.frames != 1
        || vtf.faces != 1
        || vtf.depth != 1
        || matches!(
            vtf.format,
            source_formats::vtf::ImageFormat::Uv88
                | source_formats::vtf::ImageFormat::Uvwq8888
                | source_formats::vtf::ImageFormat::Rgba16161616F
                | source_formats::vtf::ImageFormat::Rgba16161616
                | source_formats::vtf::ImageFormat::Uvlx8888
        )
    {
        return Err("Icon requires single-frame single-face depth-one LDR VTF, axes 1–512".into());
    }
    for mip in 0..vtf.mip_count {
        vtf.decode_rgba8(mip, 0, 0)
            .map_err(|error| format!("Icon VTF mip {mip}: {error}"))?;
    }
    report.width = vtf.width;
    report.height = vtf.height;
    let axis = vtf.width.max(vtf.height) as f32;
    report.world_size = [
        32.0 * vtf.width as f32 / axis,
        32.0 * vtf.height as f32 / axis,
    ];
    let mut material = MaterialInfo::untextured(path);
    material.width = vtf.width;
    material.height = vtf.height;
    material.base = Some(vtf);
    material.translucent = true;
    Ok((material, bytes))
}

#[cfg(test)]
mod tests;
