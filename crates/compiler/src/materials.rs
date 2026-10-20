//! What each material means to the compiler: brush contents, surface flags,
//! texture size and reflectivity.

use std::collections::HashMap;

use source_formats::bsp::{contents, surf};
use source_formats::gamefs::GameFs;
use source_formats::vmt::{self, Material};
use source_formats::vtf::Vtf;

pub const MAX_GEOMETRY_MATERIAL_BYTES: usize = 256 * 1024;
pub const MAX_GEOMETRY_MATERIAL_TOTAL_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_GEOMETRY_MATERIAL_READS: usize = 128;
pub const MAX_GEOMETRY_MATERIAL_ALLOCATION_BYTES: usize = MAX_GEOMETRY_MATERIAL_TOTAL_BYTES * 128;

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
    pub source: MetricSource,
    pub width: i32,
    pub height: i32,
    pub reflectivity: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricSource {
    CompileDefault,
    TextureHeader,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MetricState {
    Deferred,
    Resolved(TextureMetrics),
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
    pub unsupported_flags: Vec<String>,
    pub warnings: Vec<String>,
    pub(crate) bottom_material: Option<String>,
    pub(crate) detail_type: Option<String>,
}

impl MaterialInfo {
    pub fn texture_metrics(&self) -> Result<TextureMetrics, String> {
        match self.metrics {
            MetricState::Resolved(metrics) => Ok(metrics),
            MetricState::Deferred => Err(format!(
                "Material {} has deferred texture metrics",
                self.name
            )),
        }
    }

    pub fn lit(&self) -> bool {
        self.draws && self.surface & (surf::NOLIGHT | surf::SKY | surf::SKY2D) == 0
    }

    fn from_material(name: &str, material: Option<&Material>) -> Self {
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
                source: MetricSource::CompileDefault,
                width: 64,
                height: 64,
                reflectivity: [0.5; 3],
            }),
            found: material.is_some(),
            unsupported_flags: Vec::new(),
            warnings: Vec::new(),
            bottom_material: None,
            detail_type: None,
        };
        let Some(m) = material else {
            return info;
        };
        let semantics = m.compile_semantics();
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
        info
    }
}

pub struct Materials<'a> {
    fs: &'a GameFs,
    requirements: Requirements,
    geometry_reads: GeometryReads,
    infos: Vec<MaterialInfo>,
    by_name: HashMap<String, usize>,
    prop_lighting_sources: Option<(PropSourceMode, crate::lighting::asset_tracking::SourceReads)>,
}

#[derive(Default)]
struct GeometryReads {
    operations: usize,
    bytes: usize,
}

impl GeometryReads {
    fn read(&mut self, fs: &GameFs, path: &str) -> std::io::Result<Option<Vec<u8>>> {
        if self.operations >= MAX_GEOMETRY_MATERIAL_READS {
            return Err(std::io::Error::other(format!(
                "Geometry material semantics exceed {MAX_GEOMETRY_MATERIAL_READS} source reads"
            )));
        }
        self.operations += 1;
        let remaining = MAX_GEOMETRY_MATERIAL_TOTAL_BYTES
            .checked_sub(self.bytes)
            .ok_or_else(|| std::io::Error::other("Geometry material byte accounting overflow"))?;
        let limit = MAX_GEOMETRY_MATERIAL_BYTES.min(remaining);
        let bytes = fs.read_limited(path, limit).map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("Geometry material {path} (read limit {limit} bytes): {error}"),
            )
        })?;
        self.bytes += bytes.as_ref().map_or(0, Vec::len);
        Ok(bytes)
    }
}

impl<'a> Materials<'a> {
    pub fn new(fs: &'a GameFs) -> Self {
        Self::with_requirements(fs, Requirements::Compile)
    }

    pub(crate) fn with_requirements(fs: &'a GameFs, requirements: Requirements) -> Self {
        Self {
            fs,
            requirements,
            geometry_reads: GeometryReads::default(),
            infos: Vec::new(),
            by_name: HashMap::new(),
            prop_lighting_sources: None,
        }
    }

    pub fn get(&self, index: usize) -> &MaterialInfo {
        &self.infos[index]
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
                if let Some((_, sources)) = &mut self.prop_lighting_sources {
                    Material::load_with_reader(&vmt::material_path(&key), &mut |path| {
                        sources.read(self.fs, path, MAX_GEOMETRY_MATERIAL_BYTES)
                    })
                } else {
                    Material::load(self.fs, &key)
                }
            }
            Requirements::GeometrySemantics => {
                Material::load_with_reader(&vmt::material_path(&key), &mut |path| {
                    self.geometry_reads.read(self.fs, path)
                })
            }
        }
        .map_err(|error| format!("Material {}: {error}", vmt::material_path(&key)))?;
        let mut info = MaterialInfo::from_material(name.trim(), material.as_ref());
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
        let Some(texture) = material.get("$basetexture") else {
            return Ok(());
        };
        let path = vmt::texture_path(texture);
        let bytes = match &mut self.prop_lighting_sources {
            Some((PropSourceMode::WithReflectivity, sources)) => {
                sources.read(self.fs, &path, usize::MAX)
            }
            _ => self.fs.read(&path),
        }
        .map_err(|error| format!("{path}: {error}"))?;
        let Some(bytes) = bytes else {
            info.warnings.push(format!("Base texture {path} was not found; dimensions and reflectivity could not be measured"));
            return Ok(());
        };
        let vtf = Vtf::parse(bytes).map_err(|error| format!("{path}: {error}"))?;
        if vtf.reflectivity.iter().any(|value| !value.is_finite()) {
            return Err(format!("{path}: texture reflectivity must be finite"));
        }
        info.metrics = MetricState::Resolved(TextureMetrics {
            source: MetricSource::TextureHeader,
            width: vtf.width as i32,
            height: vtf.height as i32,
            reflectivity: vtf.reflectivity,
        });
        Ok(())
    }
}

#[cfg(test)]
#[path = "materials/prop_sources_tests.rs"]
mod prop_sources_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_semantics_bounds_patch_reads_and_bytes_without_changing_compilation() {
        use source_formats::gamefs::{Mount, MountedPath};

        let directory = tempfile::tempdir().unwrap();
        let materials = directory.path().join("materials");
        std::fs::create_dir(&materials).unwrap();
        let material = "LightmappedGeneric { %compile2dsky 1 }";
        std::fs::write(materials.join("base.vmt"), material).unwrap();
        std::fs::write(
            materials.join("patch.vmt"),
            "Patch { include materials/base.vmt }",
        )
        .unwrap();
        let mut large = " ".repeat(MAX_GEOMETRY_MATERIAL_BYTES);
        large.push_str(material);
        std::fs::write(materials.join("large.vmt"), large).unwrap();
        let mut game = crate::test_common::empty_game();
        game.mounts.push(MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.path().to_owned()),
        });

        let mut geometry = Materials::with_requirements(&game, Requirements::GeometrySemantics);
        geometry.geometry_reads.operations = MAX_GEOMETRY_MATERIAL_READS - 1;
        let error = geometry.load("patch").unwrap_err();
        assert!(error.contains("128 source reads"), "{error}");
        assert_eq!(geometry.iter().count(), 0);

        let mut geometry = Materials::with_requirements(&game, Requirements::GeometrySemantics);
        geometry.geometry_reads.bytes = MAX_GEOMETRY_MATERIAL_TOTAL_BYTES - material.len() + 1;
        let error = geometry.load("base").unwrap_err();
        assert!(error.contains("byte read limit"), "{error}");
        assert_eq!(geometry.iter().count(), 0);

        let mut geometry = Materials::with_requirements(&game, Requirements::GeometrySemantics);
        let error = geometry.load("large").unwrap_err();
        assert!(error.contains("byte read limit"), "{error}");

        let mut geometry = Materials::with_requirements(&game, Requirements::GeometrySemantics);
        geometry.geometry_reads.operations = MAX_GEOMETRY_MATERIAL_READS - 1;
        let index = geometry.load("base").unwrap();
        assert_eq!(geometry.load("BASE").unwrap(), index);
        assert!(
            geometry
                .load("patch")
                .unwrap_err()
                .contains("128 source reads")
        );

        let mut compile = Materials::new(&game);
        for name in ["base", "patch", "large"] {
            let index = compile.load(name).unwrap();
            assert_ne!(compile.get(index).surface & surf::SKY2D, 0);
        }
    }

    #[test]
    fn geometry_semantics_defers_metrics_without_relaxing_compile_dependencies() {
        use source_formats::gamefs::{Mount, MountedPath};
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("materials")).unwrap();
        std::fs::write(
            directory.path().join("materials/test.vmt"),
            "UnlitGeneric { %compile2dsky 1 $basetexture broken }",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("materials/broken.vtf"),
            b"malformed texture",
        )
        .unwrap();
        let mut game = crate::test_common::empty_game();
        game.mounts.push(MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.path().to_owned()),
        });
        let mut geometry = Materials::with_requirements(&game, Requirements::GeometrySemantics);
        let index = geometry.load("test").unwrap();
        let info = geometry.get(index);
        assert_eq!(info.metrics, MetricState::Deferred);
        assert!(info.texture_metrics().is_err());
        assert_ne!(info.surface & surf::SKY2D, 0);
        let surface = info.surface;
        assert!(Materials::new(&game).load("test").is_err());
        assert!(geometry.load("missing").is_err());
        let mut compile = Materials::new(&game);
        let missing = compile.load("missing").unwrap();
        let metrics = compile.get(missing).texture_metrics().unwrap();
        assert_eq!(metrics.source, MetricSource::CompileDefault);
        assert_eq!(
            (metrics.width, metrics.height, metrics.reflectivity),
            (64, 64, [0.5; 3])
        );
        std::fs::remove_file(directory.path().join("materials/broken.vtf")).unwrap();
        let mut compile = Materials::new(&game);
        let index = compile.load("test").unwrap();
        assert_eq!(compile.get(index).surface, surface);
        assert_eq!(
            compile.get(index).texture_metrics().unwrap().source,
            MetricSource::CompileDefault
        );
        assert!(!compile.get(index).warnings.is_empty());
    }

    #[test]
    fn secondary_surface_properties_preserve_missing_and_explicit_empty_values() {
        for (text, expected) in [
            ("WorldVertexTransition { $surfaceprop ice }", None),
            (
                "WorldVertexTransition { $surfaceprop ice $surfaceprop2 \" RUBBER \" }",
                Some("rubber"),
            ),
            (
                "WorldVertexTransition { $surfaceprop ice $surfaceprop2 \" \" }",
                Some(""),
            ),
        ] {
            let material = Material::parse(text).unwrap();
            let info = MaterialInfo::from_material("blend", Some(&material));
            assert_eq!(info.surface_prop, "ice");
            assert_eq!(info.surface_prop2.as_deref(), expected);
        }
    }
}
