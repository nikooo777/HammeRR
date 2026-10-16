use std::path::Path;

use serde::{Deserialize, Serialize};

pub const MANIFEST_PATH: &str = "hammerr/generated-cubemaps.json";
pub const MAX_SAMPLES: usize = 32;
pub const MAX_ALIASES: usize = 1024;
pub const MAX_CUBE_EDGE: u16 = 128;
pub const MAX_VTF_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_MATERIAL_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_GENERATED_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_ASSIGNMENT_WORK: usize = 4_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileContext {
    namespace: String,
}

impl CompileContext {
    pub fn new(namespace: impl Into<String>) -> Result<Self, String> {
        let namespace = namespace.into();
        if namespace.is_empty()
            || namespace.len() > 64
            || !namespace.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err("Reflection output basename must contain 1–64 ASCII letters, digits, '_' or '-'".into());
        }
        Ok(Self { namespace: namespace.to_ascii_lowercase() })
    }

    pub fn from_output(path: &Path) -> Result<Self, String> {
        Self::new(path.file_stem().and_then(|name| name.to_str()).ok_or("Reflection output has no UTF-8 basename")?)
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CubemapAssetPhase {
    Pending,
    Verified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CubemapSampleAsset {
    pub helper_id: u32,
    pub origin: [i32; 3],
    pub size_enum: u8,
    pub edge: u16,
    pub texture: String,
    pub bootstrap_sha256: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CubemapAssignment {
    pub face_id: u32,
    pub helper_id: u32,
    pub manual: bool,
    pub material: String,
    pub alias: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CubemapAliasAsset {
    pub material: String,
    pub alias: String,
    pub sample: usize,
    pub sha256: String,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CubemapSourceAsset {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CubemapAssetManifest {
    pub version: u32,
    pub phase: CubemapAssetPhase,
    pub logical_namespace: String,
    pub namespace: String,
    pub channel: String,
    pub samples: Vec<CubemapSampleAsset>,
    pub assignments: Vec<CubemapAssignment>,
    pub aliases: Vec<CubemapAliasAsset>,
    pub sources: Vec<CubemapSourceAsset>,
}

#[derive(Debug, Clone, Default)]
pub struct CubemapBindingQuery {
    pub helper_id: Option<u32>,
    pub offset: usize,
    pub limit: usize,
    pub query: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CubemapBindingReport {
    pub samples: Vec<CubemapSampleAsset>,
    pub total: usize,
    pub assignments: Vec<CubemapAssignment>,
}
