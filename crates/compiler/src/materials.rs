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
            draws: true,
            hint: false,
            skip: false,
            width: 64,
            height: 64,
            reflectivity: [0.5; 3],
            found: material.is_some(),
            unsupported_flags: Vec::new(),
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

    pub fn load(&mut self, name: &str) -> usize {
        let key = name.trim().replace('\\', "/").to_ascii_lowercase();
        if let Some(&index) = self.by_name.get(&key) {
            return index;
        }
        let material = Material::load(self.fs, &key).ok().flatten();
        let mut info = MaterialInfo::from_material(name.trim(), material.as_ref());
        if let Some(material) = &material {
            self.measure(material, &mut info);
        }
        let index = self.infos.len();
        self.infos.push(info);
        self.by_name.insert(key, index);
        index
    }

    fn measure(&self, material: &Material, info: &mut MaterialInfo) {
        let Some(texture) = material.get("$basetexture") else {
            return;
        };
        let Ok(Some(bytes)) = self.fs.read(&vmt::texture_path(texture)) else {
            return;
        };
        let Ok(vtf) = Vtf::parse(bytes) else {
            return;
        };
        info.width = vtf.width as i32;
        info.height = vtf.height as i32;
        if vtf.reflectivity.iter().all(|r| r.is_finite()) {
            info.reflectivity = vtf.reflectivity;
        }
    }
}
