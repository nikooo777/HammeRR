//! What each material means to the compiler: brush contents, surface flags,
//! texture size and reflectivity.

use std::collections::HashMap;

use source_formats::bsp::{contents, surf};
use source_formats::gamefs::GameFs;
use source_formats::vmt::{self, Material};
use source_formats::vtf::Vtf;

#[derive(Debug, Clone, PartialEq)]
pub struct MaterialInfo {
    /// Uppercase, as stored in the BSP.
    pub name: String,
    /// Contents this side gives its brush; 0 for hint/skip.
    pub contents: i32,
    pub surface: i32,
    pub surface_prop: String,
    /// Whether sides with this material become visible faces.
    pub draws: bool,
    pub hint: bool,
    pub skip: bool,
    pub width: i32,
    pub height: i32,
    /// Average linear albedo of the base texture, from its VTF header.
    pub reflectivity: [f32; 3],
    pub found: bool,
    pub unsupported_flags: Vec<String>,
    pub warnings: Vec<String>,
}

impl MaterialInfo {
    pub fn lit(&self) -> bool {
        self.draws && self.surface & (surf::NOLIGHT | surf::SKY | surf::SKY2D) == 0
    }

    fn from_material(name: &str, material: Option<&Material>) -> Self {
        let mut info = Self {
            name: name.to_ascii_uppercase(),
            contents: contents::SOLID,
            surface: 0,
            surface_prop: "default".into(),
            draws: true,
            hint: false,
            skip: false,
            width: 64,
            height: 64,
            reflectivity: [0.5; 3],
            found: material.is_some(),
            unsupported_flags: Vec::new(),
            warnings: Vec::new(),
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
        info.unsupported_flags = semantics.unsupported_flags;
        info
    }
}

pub struct Materials<'a> {
    fs: &'a GameFs,
    infos: Vec<MaterialInfo>,
    by_name: HashMap<String, usize>,
}

impl<'a> Materials<'a> {
    pub fn new(fs: &'a GameFs) -> Self {
        Self {
            fs,
            infos: Vec::new(),
            by_name: HashMap::new(),
        }
    }

    pub fn get(&self, index: usize) -> &MaterialInfo {
        &self.infos[index]
    }

    pub fn iter(&self) -> impl Iterator<Item = &MaterialInfo> {
        self.infos.iter()
    }

    pub fn load(&mut self, name: &str) -> Result<usize, String> {
        let key = name.trim().replace('\\', "/").to_ascii_lowercase();
        if let Some(&index) = self.by_name.get(&key) {
            return Ok(index);
        }
        let material = Material::load(self.fs, &key)
            .map_err(|error| format!("Material {}: {error}", vmt::material_path(&key)))?;
        let mut info = MaterialInfo::from_material(name.trim(), material.as_ref());
        if let Some(material) = &material {
            self.measure(material, &mut info)?;
        }
        let index = self.infos.len();
        self.infos.push(info);
        self.by_name.insert(key, index);
        Ok(index)
    }

    fn measure(&self, material: &Material, info: &mut MaterialInfo) -> Result<(), String> {
        let Some(texture) = material.get("$basetexture") else {
            return Ok(());
        };
        let path = vmt::texture_path(texture);
        let Some(bytes) = self
            .fs
            .read(&path)
            .map_err(|error| format!("{path}: {error}"))?
        else {
            info.warnings.push(format!("Base texture {path} was not found; dimensions and reflectivity could not be measured"));
            return Ok(());
        };
        let vtf = Vtf::parse(bytes).map_err(|error| format!("{path}: {error}"))?;
        if vtf.reflectivity.iter().any(|value| !value.is_finite()) {
            return Err(format!("{path}: texture reflectivity must be finite"));
        }
        info.width = vtf.width as i32;
        info.height = vtf.height as i32;
        info.reflectivity = vtf.reflectivity;
        Ok(())
    }
}
