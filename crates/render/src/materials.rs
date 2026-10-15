//! CPU-side material loading: VMT parameters plus the textures to display.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

use source_formats::gamefs::GameFs;
use source_formats::vmt::{Material, texture_path};
use source_formats::vtf::Vtf;

use crate::helpers::{HelperMaterial, bulb_material};

/// Texture size assumed for UV mapping when a material can't be loaded.
const FALLBACK_SIZE: u32 = 128;
pub const MAX_RELOAD_FAILURES: usize = 100;

#[derive(Debug)]
pub struct MaterialInfo {
    /// Material name as used in the map, lowercase (`de_cbble/cobbleroad01`).
    pub name: String,
    pub base: Option<Vtf>,
    /// Second layer of a blend material (`$basetexture2`), blended by
    /// displacement vertex alpha.
    pub blend: Option<Vtf>,
    /// Size of the displayed texture, used to normalize texture coordinates.
    pub width: u32,
    pub height: u32,
    pub alpha_test: bool,
    pub translucent: bool,
    pub tool: bool,
    pub sky: bool,
    /// No usable display texture is available.
    pub missing: bool,
    pub error: Option<String>,
    pub(crate) helper: Option<&'static HelperMaterial>,
}

impl MaterialInfo {
    pub fn load(fs: &GameFs, name: &str) -> Self {
        let name = name.to_ascii_lowercase();
        match Self::load_checked(fs, &name) {
            Ok(material) => material,
            Err(error) => {
                let mut material = Self::untextured(&name);
                material.width = FALLBACK_SIZE;
                material.height = FALLBACK_SIZE;
                material.missing = true;
                material.error = Some(error);
                material
            }
        }
    }

    fn load_checked(fs: &GameFs, name: &str) -> Result<Self, String> {
        let material =
            Material::load(fs, name).map_err(|error| format!("Material {name}: {error}"))?;
        let mut base = None;
        if let Some(material) = &material {
            for path in material.editor_textures() {
                if let Some(texture) = read_vtf(fs, &path)? {
                    base = Some(texture);
                    break;
                }
            }
        }
        let blend = material
            .as_ref()
            .filter(|m| m.shader == "worldvertextransition")
            .and_then(|m| m.get("$basetexture2"))
            .map(|texture| {
                let path = texture_path(texture);
                read_vtf(fs, &path)?.ok_or_else(|| format!("Blend texture {path} was not found"))
            })
            .transpose()?;
        let (width, height) = base
            .as_ref()
            .map_or((FALLBACK_SIZE, FALLBACK_SIZE), |v| (v.width, v.height));
        let sky = material.as_ref().is_some_and(|material| {
            material.compile_semantics().surface & source_formats::bsp::surf::SKY != 0
        }) || matches!(name, "tools/toolsskybox" | "tools/toolsskybox2d");

        Ok(Self {
            helper: None,
            error: None,
            missing: base.is_none(),
            alpha_test: material.as_ref().is_some_and(Material::alpha_test),
            translucent: material.as_ref().is_some_and(Material::translucent),
            tool: sky || material.as_ref().is_some_and(Material::is_tool),
            sky,
            name: name.to_owned(),
            base,
            blend,
            width,
            height,
        })
    }

    /// A flat-colored material for editor helpers such as entity boxes.
    pub fn untextured(name: &str) -> Self {
        Self {
            helper: None,
            name: name.to_owned(),
            base: None,
            blend: None,
            width: 1,
            height: 1,
            alpha_test: false,
            translucent: false,
            tool: false,
            sky: false,
            missing: false,
            error: None,
        }
    }
}

fn read_vtf(fs: &GameFs, path: &str) -> Result<Option<Vtf>, String> {
    fs.read(path)
        .map_err(|error| format!("{path}: {error}"))?
        .map(|bytes| Vtf::parse(bytes).map_err(|error| format!("{path}: {error}")))
        .transpose()
}

/// Index of a material in a [`MaterialLibrary`].
pub type MaterialId = usize;

/// Materials loaded so far, kept across scene rebuilds so edits only
/// rebuild geometry. Ids are stable for the library's lifetime.
#[derive(Debug)]
pub struct MaterialLibrary {
    generation: u64,
    materials: Vec<MaterialInfo>,
    by_name: HashMap<String, MaterialId>,
    skyboxes: HashMap<String, Arc<crate::sky::Skybox>>,
}

impl Default for MaterialLibrary {
    fn default() -> Self {
        Self {
            generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
            materials: vec![MaterialInfo::untextured(Self::EDITOR_NAME)],
            by_name: HashMap::from([(Self::EDITOR_NAME.to_owned(), Self::EDITOR)]),
            skyboxes: HashMap::new(),
        }
    }
}

#[derive(Debug, Default)]
pub struct ReloadReport {
    pub reloaded: usize,
    pub failed: usize,
    pub failures: Vec<(String, String)>,
}

impl ReloadReport {
    pub fn record_failure(&mut self, name: String, error: String) {
        self.failed += 1;
        if self.failures.len() < MAX_RELOAD_FAILURES {
            self.failures.push((name, error));
        }
    }
}

impl MaterialLibrary {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn reload(&mut self, fs: &GameFs) -> ReloadReport {
        let mut report = ReloadReport::default();
        for material in self.materials.iter_mut().skip(1) {
            if material.helper.is_some() {
                continue;
            }
            *material = MaterialInfo::load(fs, &material.name);
            report.reloaded += 1;
            if let Some(error) = &material.error {
                report.record_failure(material.name.clone(), error.clone());
            } else if material.missing {
                report.record_failure(
                    material.name.clone(),
                    "No usable display texture was found".into(),
                );
            }
        }
        let mut skies = self.skyboxes.keys().cloned().collect::<Vec<_>>();
        skies.sort();
        self.skyboxes.clear();
        for name in skies {
            let sky = self.skybox(fs, &name);
            for face in &sky.faces {
                if let Some(error) = &face.issue {
                    report.record_failure(face.material.clone(), error.clone());
                }
            }
        }
        self.generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        report
    }

    pub fn skybox(&mut self, fs: &GameFs, name: &str) -> Arc<crate::sky::Skybox> {
        let name = name.trim().to_ascii_lowercase();
        self.skyboxes
            .entry(name.clone())
            .or_insert_with(|| Arc::new(crate::sky::Skybox::load(fs, &name)))
            .clone()
    }

    /// Flat white, tinted by vertex color: entity boxes and other helpers.
    pub const EDITOR: MaterialId = 0;
    const EDITOR_NAME: &str = "__editor/flat";

    pub(crate) fn light_bulb(&mut self) -> MaterialId {
        const NAME: &str = "__editor/lightbulb";
        if let Some(&id) = self.by_name.get(NAME) {
            return id;
        }
        let mut material = MaterialInfo::untextured(NAME);
        material.helper = Some(bulb_material());
        material.width = 512;
        material.height = 512;
        let id = self.materials.len();
        self.materials.push(material);
        self.by_name.insert(NAME.to_owned(), id);
        id
    }

    pub fn load(&mut self, fs: &GameFs, name: &str) -> MaterialId {
        let key = name.to_ascii_lowercase();
        if let Some(&id) = self.by_name.get(&key) {
            return id;
        }
        self.materials.push(MaterialInfo::load(fs, &key));
        let id = self.materials.len() - 1;
        self.by_name.insert(key, id);
        id
    }

    pub fn find(&self, name: &str) -> Option<&MaterialInfo> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .map(|id| &self.materials[*id])
    }

    pub fn get(&self, id: MaterialId) -> &MaterialInfo {
        &self.materials[id]
    }

    pub fn len(&self) -> usize {
        self.materials.len()
    }

    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }
}
