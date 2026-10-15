//! CPU-side material loading: VMT parameters plus the textures to display.

use std::collections::HashMap;
use std::sync::Arc;

use source_formats::gamefs::GameFs;
use source_formats::vmt::{Material, texture_path};
use source_formats::vtf::Vtf;

use crate::helpers::{HelperMaterial, bulb_material};

/// Texture size assumed for UV mapping when a material can't be loaded.
const FALLBACK_SIZE: u32 = 128;

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
    /// Neither the VMT nor its texture could be found.
    pub missing: bool,
    pub(crate) helper: Option<&'static HelperMaterial>,
}

impl MaterialInfo {
    pub fn load(fs: &GameFs, name: &str) -> Self {
        let name = name.to_ascii_lowercase();
        let material = Material::load(fs, &name).ok().flatten();
        let read_vtf = |path: &str| -> Option<Vtf> { Vtf::parse(fs.read(path).ok()??).ok() };

        let base = material
            .as_ref()
            .and_then(|m| m.editor_textures().iter().find_map(|t| read_vtf(t)));
        let blend = material
            .as_ref()
            .filter(|m| m.shader == "worldvertextransition")
            .and_then(|m| m.get("$basetexture2"))
            .and_then(|t| read_vtf(&texture_path(t)));
        let (width, height) = base
            .as_ref()
            .map_or((FALLBACK_SIZE, FALLBACK_SIZE), |v| (v.width, v.height));

        Self {
            helper: None,
            missing: base.is_none(),
            alpha_test: material.as_ref().is_some_and(Material::alpha_test),
            translucent: material.as_ref().is_some_and(Material::translucent),
            tool: material.as_ref().is_some_and(Material::is_tool),
            sky: material.as_ref().is_some_and(|material| material.compile_semantics().surface & source_formats::bsp::surf::SKY != 0)
                || matches!(name.as_str(), "tools/toolsskybox" | "tools/toolsskybox2d"),
            name,
            base,
            blend,
            width,
            height,
        }
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
        }
    }
}

/// Index of a material in a [`MaterialLibrary`].
pub type MaterialId = usize;

/// Materials loaded so far, kept across scene rebuilds so edits only
/// rebuild geometry. Ids are stable for the library's lifetime.
#[derive(Debug)]
pub struct MaterialLibrary {
    materials: Vec<MaterialInfo>,
    by_name: HashMap<String, MaterialId>,
    skyboxes: HashMap<String, Arc<crate::sky::Skybox>>,
}

impl Default for MaterialLibrary {
    fn default() -> Self {
        Self {
            materials: vec![MaterialInfo::untextured(Self::EDITOR_NAME)],
            by_name: HashMap::from([(Self::EDITOR_NAME.to_owned(), Self::EDITOR)]),
            skyboxes: HashMap::new(),
        }
    }
}

impl MaterialLibrary {
    pub fn skybox(&mut self, fs: &GameFs, name: &str) -> Arc<crate::sky::Skybox> {
        let name = name.trim().to_ascii_lowercase();
        self.skyboxes.entry(name.clone()).or_insert_with(|| Arc::new(crate::sky::Skybox::load(fs, &name))).clone()
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
