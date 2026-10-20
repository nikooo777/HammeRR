use serde::Serialize;

pub const MAX_MATERIAL_BYTES: usize = 1024 * 1024;
pub const MAX_MATERIAL_TOTAL_BYTES: usize = 4 * MAX_MATERIAL_BYTES;
pub const MAX_MATERIAL_READS: usize = 9;
pub const MAX_ASSETS: usize = 64;
pub const MAX_EDGES: usize = 128;
pub const MAX_ISSUES: usize = 16;
pub const MAX_TEXTURE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PHYSICAL_PATHS: usize = 8192;
pub const MAX_PATH_BYTES: usize = 512;
pub const MAX_TEXTURE_AXIS: u32 = 4096;
pub(crate) const MAX_KEY_BYTES: usize = 128;
pub(crate) const MAX_DEPTH: usize = 8;
pub(crate) const MAX_MESSAGE_CHARS: usize = 2048;
pub(crate) const MAX_ORIGIN_CHARS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WaterDependencyState {
    NotApplicable,
    SupportedStaticInventory,
    Unsupported,
    UnavailableSource,
    Limited,
}

impl WaterDependencyState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::SupportedStaticInventory => "supported_static_inventory",
            Self::Unsupported => "unsupported",
            Self::UnavailableSource => "unavailable_source",
            Self::Limited => "limited",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WaterDependencyReport {
    pub state: WaterDependencyState,
    pub model_basis: &'static str,
    pub scope: &'static str,
    pub root_material: String,
    pub assets: Vec<DependencyAsset>,
    pub edges: Vec<DependencyEdge>,
    pub issues: Vec<DependencyIssue>,
    pub counters: InspectionCounters,
    pub limits: InspectionLimits,
}

impl WaterDependencyReport {
    pub fn not_applicable(root: &str) -> Self {
        let mut report = Self::new(root);
        report.state = WaterDependencyState::NotApplicable;
        report
    }

    pub fn unavailable_source(root: &str, message: &str) -> Self {
        let mut report = Self::new(root);
        report.state = WaterDependencyState::UnavailableSource;
        report.issues.push(DependencyIssue {
            state: report.state,
            code: "root_source",
            owner: report.root_material.clone(),
            key: None,
            path: Some(report.root_material.clone()),
            message: message.chars().take(MAX_MESSAGE_CHARS).collect(),
        });
        report
    }

    pub(crate) fn new(root: &str) -> Self {
        Self {
            state: WaterDependencyState::SupportedStaticInventory,
            model_basis: "css_sdk_dx90_static_water",
            scope: "One-material SDK-qualified dependency inventory and existing VTF parser observations; not rendering readiness, map portability, packing or a durable source token",
            root_material: root
                .chars()
                .scan(0, |bytes, character| {
                    *bytes += character.len_utf8();
                    (*bytes <= MAX_PATH_BYTES).then_some(character)
                })
                .collect(),
            assets: Vec::new(),
            edges: Vec::new(),
            issues: Vec::new(),
            counters: InspectionCounters::default(),
            limits: InspectionLimits::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Material,
    Texture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetState {
    Found,
    Missing,
    Unavailable,
    UnsupportedTexture,
}

#[derive(Debug, Clone, Serialize)]
pub struct DependencyAsset {
    pub path: String,
    pub kind: AssetKind,
    pub state: AssetState,
    pub bytes: usize,
    pub sha256: Option<String>,
    pub origin: Option<DependencyOrigin>,
    pub shader: Option<String>,
    pub texture: Option<TextureObservation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DependencyOrigin {
    pub kind: &'static str,
    pub mount: String,
    pub file: Option<String>,
    pub storage_files: Vec<String>,
    pub paths_truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TextureObservation {
    pub layout_scope: &'static str,
    pub version: [u32; 2],
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub authored_depth: Option<u16>,
    pub authored_frames: u16,
    pub first_frame: u16,
    pub faces: u8,
    pub mip_count: u8,
    pub flags: u32,
    pub format: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyRole {
    PatchInclude,
    NormalTexture,
    ColorTexture,
    CubemapTexture,
    CompilerMaterial,
    RuntimeMaterial,
    EditorPreviewTexture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclarationOrigin {
    Authored,
    PatchEffective,
    SdkDefault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeState {
    Mounted,
    RuntimeResource,
    Disabled,
    Unsupported,
    Missing,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
pub struct DependencyEdge {
    pub owner: String,
    pub key: String,
    pub reference: String,
    pub target: Option<String>,
    pub role: DependencyRole,
    pub origin: DeclarationOrigin,
    pub state: EdgeState,
    pub evidence_basis: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct DependencyIssue {
    pub state: WaterDependencyState,
    pub code: &'static str,
    pub owner: String,
    pub key: Option<String>,
    pub path: Option<String>,
    pub message: String,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct InspectionCounters {
    pub asset_lookups: usize,
    pub material_reads: usize,
    pub material_bytes: usize,
    pub texture_reads: usize,
    pub texture_bytes: usize,
    pub source_bytes: usize,
    pub edges: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct InspectionLimits {
    pub material_bytes: usize,
    pub material_total_bytes: usize,
    pub material_reads: usize,
    pub assets: usize,
    pub edges: usize,
    pub issues: usize,
    pub texture_bytes: usize,
    pub source_bytes: usize,
    pub physical_paths: usize,
    pub path_bytes: usize,
    pub texture_axis: u32,
    pub material_depth: usize,
}

impl Default for InspectionLimits {
    fn default() -> Self {
        Self {
            material_bytes: MAX_MATERIAL_BYTES,
            material_total_bytes: MAX_MATERIAL_TOTAL_BYTES,
            material_reads: MAX_MATERIAL_READS,
            assets: MAX_ASSETS,
            edges: MAX_EDGES,
            issues: MAX_ISSUES,
            texture_bytes: MAX_TEXTURE_BYTES,
            source_bytes: MAX_SOURCE_BYTES,
            physical_paths: MAX_PHYSICAL_PATHS,
            path_bytes: MAX_PATH_BYTES,
            texture_axis: MAX_TEXTURE_AXIS,
            material_depth: MAX_DEPTH,
        }
    }
}
