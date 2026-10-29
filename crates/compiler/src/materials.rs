//! What each material means to the compiler: brush contents, surface flags,
//! texture size and reflectivity.

use std::collections::HashMap;

use source_formats::bsp::{contents, surf};
use source_formats::gamefs::GameFs;
use source_formats::vmt::{self, Material};
use source_formats::vtf::Vtf;

pub(crate) mod metrics;
mod read_budget;
use metrics::{
    DimensionProvider, MetricPlan, NATIVE_NO_PREVIEW_DIMENSION, ReflectivityProvider, TextureHeader,
};
use read_budget::{ReadBudget, ReadLimits};

pub const MAX_COMPILE_MATERIAL_BYTES: usize = 1024 * 1024;
pub const MAX_COMPILE_MATERIAL_TOTAL_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_COMPILE_MATERIAL_READS: usize = 4096;
pub const MAX_METRIC_TEXTURE_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_METRIC_TEXTURE_TOTAL_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_METRIC_TEXTURE_READS: usize = 4096;
pub const MAX_GEOMETRY_MATERIAL_BYTES: usize = 256 * 1024;
pub const MAX_GEOMETRY_MATERIAL_TOTAL_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_GEOMETRY_MATERIAL_READS: usize = 128;
pub const MAX_GEOMETRY_MATERIAL_ALLOCATION_BYTES: usize = MAX_GEOMETRY_MATERIAL_TOTAL_BYTES * 128;
const MAX_REFLECTIVITY_VALUE_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirements {
    Compile,
    GeometrySemantics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PropSourceMode {
    Direct,
    WithReflectivity,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextureMetrics {
    pub dimension_source: DimensionSource,
    pub reflectivity_source: ReflectivitySource,
    pub width: i32,
    pub height: i32,
    pub reflectivity: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DimensionSource {
    CompileDefault,
    DiffuseTextureHeader,
    ToolTextureHeader,
    NativeNoPreview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflectivitySource {
    CompileDefault,
    TextureHeader,
    AuthoredMaterial,
}

impl ReflectivitySource {
    pub fn is_measured_or_authored(self) -> bool {
        matches!(self, Self::TextureHeader | Self::AuthoredMaterial)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MetricState {
    Deferred,
    Resolved(TextureMetrics),
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaterialInfo {
    /// Uppercase, as stored in the BSP.
    pub name: String,
    /// Contents this side gives its brush; 0 for hint/skip.
    pub contents: i32,
    pub surface: i32,
    pub surface_prop: String,
    pub(crate) surface_prop2: Option<String>,
    /// Whether sides with this material become visible faces.
    pub draws: bool,
    pub hint: bool,
    pub skip: bool,
    pub metrics: MetricState,
    pub found: bool,
    pub(crate) water: bool,
    pub unsupported_flags: Vec<String>,
    pub warnings: Vec<String>,
    pub(crate) bottom_material: Option<String>,
    pub(crate) detail_type: Option<String>,
}

impl MaterialInfo {
    pub fn texture_metrics(&self) -> Result<TextureMetrics, String> {
        match &self.metrics {
            MetricState::Resolved(metrics) => Ok(*metrics),
            MetricState::Unavailable(reason) => Err(reason.clone()),
            MetricState::Deferred => Err(format!(
                "Material {} has deferred texture metrics",
                self.name
            )),
        }
    }

    pub fn lit(&self) -> bool {
        self.draws && self.surface & (surf::NOLIGHT | surf::SKY | surf::SKY2D) == 0
    }

    fn from_material(name: &str, material: Option<&Material>) -> Result<Self, String> {
        let mut info = Self {
            name: name.to_ascii_uppercase(),
            contents: contents::SOLID,
            surface: 0,
            surface_prop: "default".into(),
            surface_prop2: None,
            draws: true,
            hint: false,
            skip: false,
            metrics: MetricState::Resolved(TextureMetrics {
                dimension_source: DimensionSource::CompileDefault,
                reflectivity_source: ReflectivitySource::CompileDefault,
                width: 64,
                height: 64,
                reflectivity: [0.5; 3],
            }),
            found: material.is_some(),
            water: false,
            unsupported_flags: Vec::new(),
            warnings: Vec::new(),
            bottom_material: None,
            detail_type: None,
        };
        let Some(m) = material else {
            return Ok(info);
        };
        info.water = crate::water_dependencies::is_water_shader(&m.shader);
        let semantics = vmt::CompileSemantics::checked_from_material(m)?;
        info.contents = semantics.contents;
        info.surface = semantics.surface;
        info.draws = semantics.draws;
        info.hint = semantics.hint;
        info.skip = semantics.skip;
        if let Some(value) = m.get("$surfaceprop") {
            info.surface_prop = value.trim().to_ascii_lowercase();
        }
        info.surface_prop2 = m
            .get("$surfaceprop2")
            .map(|value| value.trim().to_ascii_lowercase());
        info.bottom_material = m
            .get("$bottommaterial")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        info.detail_type = m
            .get("%detailtype")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        info.unsupported_flags = semantics.unsupported_flags;
        Ok(info)
    }
}

pub struct Materials<'a> {
    fs: &'a GameFs,
    requirements: Requirements,
    geometry_reads: ReadBudget,
    compile_reads: ReadBudget,
    texture_reads: ReadBudget,
    infos: Vec<MaterialInfo>,
    by_name: HashMap<String, usize>,
    prop_lighting_sources: Option<(PropSourceMode, crate::lighting::asset_tracking::SourceReads)>,
}

impl<'a> Materials<'a> {
    pub fn new(fs: &'a GameFs) -> Self {
        Self::with_requirements(fs, Requirements::Compile)
    }

    pub(crate) fn with_requirements(fs: &'a GameFs, requirements: Requirements) -> Self {
        Self {
            fs,
            requirements,
            geometry_reads: ReadBudget::new(ReadLimits {
                label: "Geometry material semantics",
                file_bytes: MAX_GEOMETRY_MATERIAL_BYTES,
                total_bytes: MAX_GEOMETRY_MATERIAL_TOTAL_BYTES,
                operations: MAX_GEOMETRY_MATERIAL_READS,
            }),
            compile_reads: ReadBudget::new(ReadLimits {
                label: "Compile material",
                file_bytes: MAX_COMPILE_MATERIAL_BYTES,
                total_bytes: MAX_COMPILE_MATERIAL_TOTAL_BYTES,
                operations: MAX_COMPILE_MATERIAL_READS,
            }),
            texture_reads: ReadBudget::new(ReadLimits {
                label: "Metric texture",
                file_bytes: MAX_METRIC_TEXTURE_BYTES,
                total_bytes: MAX_METRIC_TEXTURE_TOTAL_BYTES,
                operations: MAX_METRIC_TEXTURE_READS,
            }),
            infos: Vec::new(),
            by_name: HashMap::new(),
            prop_lighting_sources: None,
        }
    }

    pub(crate) fn with_build_sources(
        fs: &'a GameFs,
        requirements: Requirements,
        sources: &crate::asset_sources::BuildMaterialSources,
    ) -> Self {
        let mut materials = Self::with_requirements(fs, requirements);
        materials.geometry_reads.track_build_sources(sources);
        materials.compile_reads.track_build_sources(sources);
        materials.texture_reads.track_build_sources(sources);
        materials
    }

    pub(crate) fn uses_build_sources(
        &self,
        sources: &crate::asset_sources::BuildMaterialSources,
    ) -> bool {
        self.geometry_reads.uses_build_sources(sources)
            && self.compile_reads.uses_build_sources(sources)
            && self.texture_reads.uses_build_sources(sources)
    }

    pub fn get(&self, index: usize) -> &MaterialInfo {
        &self.infos[index]
    }

    pub(crate) fn cached(&self, canonical_name: &str) -> Option<usize> {
        self.by_name.get(canonical_name).copied()
    }

    pub(crate) fn track_prop_lighting_sources(
        &mut self,
        mode: PropSourceMode,
    ) -> Result<(), String> {
        if !self.infos.is_empty() || self.prop_lighting_sources.is_some() {
            return Err(
                "Prop lighting source mode must be selected before loading materials".into(),
            );
        }
        self.prop_lighting_sources = Some((mode, Default::default()));
        Ok(())
    }

    pub(crate) fn take_prop_lighting_sources(
        &mut self,
    ) -> Option<crate::lighting::asset_tracking::SourceReads> {
        self.prop_lighting_sources
            .take()
            .map(|(_, sources)| sources)
    }

    pub(crate) fn prop_lighting_sources_mut(
        &mut self,
    ) -> Option<&mut crate::lighting::asset_tracking::SourceReads> {
        self.prop_lighting_sources
            .as_mut()
            .map(|(_, sources)| sources)
    }

    pub fn iter(&self) -> impl Iterator<Item = &MaterialInfo> {
        self.infos.iter()
    }

    pub(crate) fn add_alias(&mut self, source: usize, name: &str) -> Result<usize, String> {
        let key = name.to_ascii_lowercase();
        if self.by_name.contains_key(&key) {
            return Err(format!(
                "Generated reflection material collides with an existing material: {name}"
            ));
        }
        let mut info = self.infos[source].clone();
        info.name = name.to_ascii_uppercase();
        let index = self.infos.len();
        self.infos.push(info);
        self.by_name.insert(key, index);
        Ok(index)
    }

    pub fn load(&mut self, name: &str) -> Result<usize, String> {
        let key = name.trim().replace('\\', "/").to_ascii_lowercase();
        if let Some(&index) = self.by_name.get(&key) {
            return Ok(index);
        }
        let material = match self.requirements {
            Requirements::Compile => {
                Material::load_with_reader(&vmt::material_path(&key), &mut |path| {
                    let maximum = if self.prop_lighting_sources.is_some() {
                        MAX_GEOMETRY_MATERIAL_BYTES
                    } else {
                        MAX_COMPILE_MATERIAL_BYTES
                    };
                    self.compile_reads.read(
                        self.fs,
                        path,
                        maximum,
                        self.prop_lighting_sources
                            .as_mut()
                            .map(|(_, sources)| sources),
                    )
                })
            }
            Requirements::GeometrySemantics => {
                Material::load_with_reader(&vmt::material_path(&key), &mut |path| {
                    self.geometry_reads
                        .read(self.fs, path, MAX_GEOMETRY_MATERIAL_BYTES, None)
                })
            }
        }
        .map_err(|error| format!("Material {}: {error}", vmt::material_path(&key)))?;
        let mut info = MaterialInfo::from_material(name.trim(), material.as_ref())
            .map_err(|error| format!("Material {}: {error}", vmt::material_path(&key)))?;
        match self.requirements {
            Requirements::Compile => {
                if let Some(material) = &material {
                    self.measure(material, &mut info)?;
                }
            }
            Requirements::GeometrySemantics => {
                if material.is_none() {
                    return Err(format!(
                        "Material {} is unavailable; geometry semantics cannot be determined",
                        vmt::material_path(&key)
                    ));
                }
                info.metrics = MetricState::Deferred;
            }
        }
        let index = self.infos.len();
        self.infos.push(info);
        self.by_name.insert(key, index);
        Ok(index)
    }

    fn measure(&mut self, material: &Material, info: &mut MaterialInfo) -> Result<(), String> {
        let material_path = vmt::material_path(&info.name);
        let key = material
            .primary_base_texture_key()
            .map_err(|error| format!("Material {material_path}: {error}"))?;
        if key != Some("$albedo") && (!info.draws || info.surface & (surf::SKY | surf::SKY2D) != 0)
        {
            return Ok(());
        }
        let plan = MetricPlan::new(material, key)
            .map_err(|error| format!("Material {material_path}: {error}"))?;
        if let Some(reason) = plan.unavailable() {
            info.metrics = MetricState::Unavailable(format!("Material {material_path}: {reason}"));
            return Ok(());
        }
        let primary_path = key
            .and_then(|key| material.get(key))
            .map(|reference| vmt::texture_path(reference.trim()));
        let mut headers: Vec<(String, TextureHeader)> = Vec::with_capacity(2);
        for provider in plan.headers().into_iter().flatten() {
            if headers.iter().any(|(path, _)| *path == provider.path) {
                continue;
            }
            let sources = self
                .prop_lighting_sources
                .as_mut()
                .and_then(|(mode, sources)| {
                    (*mode == PropSourceMode::WithReflectivity).then_some(sources)
                });
            let bytes = self
                .texture_reads
                .read(self.fs, &provider.path, MAX_METRIC_TEXTURE_BYTES, sources)
                .map_err(|error| {
                    format!(
                        "Material {material_path}: selected {} texture {}: {error}",
                        provider.key, provider.path,
                    )
                })?;
            let Some(bytes) = bytes else {
                let mut reason = format!(
                    "Material {material_path}: selected {} texture {} was not found; required {} are unavailable",
                    provider.key,
                    provider.path,
                    plan.demands(&provider.path),
                );
                if matches!(plan.reflectivity, ReflectivityProvider::Authored(_)) {
                    reason
                        .push_str("; authored $reflectivity does not replace required dimensions");
                }
                if plan
                    .headers()
                    .into_iter()
                    .flatten()
                    .any(|required| required.path == provider.path && required.key == "$albedo")
                {
                    return Err(reason);
                }
                info.metrics = MetricState::Unavailable(reason);
                return Ok(());
            };
            let vtf = Vtf::parse(bytes).map_err(|error| {
                format!(
                    "Material {material_path}: selected {} texture {}: {error}",
                    provider.key, provider.path,
                )
            })?;
            if provider.key != "%tooltexture"
                || primary_path.as_deref() == Some(provider.path.as_str())
            {
                vtf.validate_native_mips().map_err(|error| {
                    format!(
                        "Material {material_path}: selected {} texture {}: {error}",
                        provider.key, provider.path
                    )
                })?;
            }
            headers.push((
                provider.path.clone(),
                TextureHeader {
                    width: vtf.width as i32,
                    height: vtf.height as i32,
                    reflectivity: vtf.reflectivity,
                },
            ));
        }
        let header = |path: &str| {
            headers
                .iter()
                .find(|(candidate, _)| candidate == path)
                .map(|(_, header)| *header)
                .ok_or_else(|| {
                    format!(
                        "Material {material_path}: planned texture header {path} is unavailable"
                    )
                })
        };
        let (dimension_source, width, height) = match &plan.dimensions {
            DimensionProvider::CompileDefault => (DimensionSource::CompileDefault, 64, 64),
            DimensionProvider::Header(provider, source) => {
                let data = header(&provider.path)?;
                (*source, data.width, data.height)
            }
            DimensionProvider::NativeNoPreview => (
                DimensionSource::NativeNoPreview,
                NATIVE_NO_PREVIEW_DIMENSION,
                NATIVE_NO_PREVIEW_DIMENSION,
            ),
        };
        let (reflectivity_source, reflectivity) = match &plan.reflectivity {
            ReflectivityProvider::CompileDefault => (ReflectivitySource::CompileDefault, [0.5; 3]),
            ReflectivityProvider::Authored(value) => (ReflectivitySource::AuthoredMaterial, *value),
            ReflectivityProvider::Header(provider) => {
                let data = header(&provider.path)?;
                if data.reflectivity.iter().any(|value| !value.is_finite()) {
                    return Err(format!(
                        "{}: texture reflectivity must be finite",
                        provider.path
                    ));
                }
                (ReflectivitySource::TextureHeader, data.reflectivity)
            }
        };
        info.metrics = MetricState::Resolved(TextureMetrics {
            dimension_source,
            reflectivity_source,
            width,
            height,
            reflectivity,
        });
        Ok(())
    }
}

pub(crate) fn parse_reflectivity(raw: &str) -> Result<[f32; 3], String> {
    let invalid = || {
        format!(
            "$reflectivity requires an ASCII square-bracket triple of finite linear components from 0 to 1, at most {MAX_REFLECTIVITY_VALUE_BYTES} bytes"
        )
    };
    if raw.len() > MAX_REFLECTIVITY_VALUE_BYTES || !raw.is_ascii() {
        return Err(invalid());
    }
    let inner = raw
        .trim_ascii()
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or_else(invalid)?;
    if inner.split_ascii_whitespace().take(4).count() != 3 {
        return Err(invalid());
    }
    let values = source_formats::fgd::vector(inner, 3).ok_or_else(invalid)?;
    if values.iter().any(|value| !(0.0..=1.0).contains(value)) {
        return Err(invalid());
    }
    Ok([values[0] as f32, values[1] as f32, values[2] as f32])
}
