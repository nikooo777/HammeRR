//! What each material means to the compiler: brush contents, surface flags,
//! texture size and reflectivity.

use std::collections::HashMap;

use source_formats::bsp::{contents, surf};
use source_formats::gamefs::GameFs;
use source_formats::vmt::{self, Material};
use source_formats::vtf::Vtf;

/// Blocks line of sight for NPCs without being solid.
pub const CONTENTS_BLOCKLOS: i32 = 0x40;

#[derive(Debug, Clone, PartialEq)]
pub struct MaterialInfo {
    /// Uppercase, as stored in the BSP.
    pub name: String,
    /// Contents this side gives its brush; 0 for non-solid.
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
        };
        let Some(m) = material else {
            return info;
        };
        let flag = |key: &str| {
            m.get(key)
                .and_then(|v| v.trim().parse::<f64>().ok())
                .is_some_and(|v| v != 0.0)
        };
        let hide = |info: &mut Self| {
            info.draws = false;
            info.surface |= surf::NODRAW;
        };

        if flag("%compilenodraw") {
            hide(&mut info);
        }
        let mut clip = 0;
        if flag("%compileclip") {
            clip |= contents::PLAYERCLIP | contents::MONSTERCLIP;
        }
        if flag("%playerclip") || flag("%compileplayerclip") {
            clip |= contents::PLAYERCLIP;
        }
        if flag("%compilenpcclip") {
            clip |= contents::MONSTERCLIP;
        }
        if clip != 0 {
            info.contents = clip;
            hide(&mut info);
        }
        if flag("%compiletrigger") {
            info.surface |= surf::TRIGGER;
            hide(&mut info);
        }
        if flag("%compilesky") {
            info.surface |= surf::SKY | surf::NOLIGHT;
        }
        if flag("%compile2dsky") {
            info.surface |= surf::SKY | surf::SKY2D | surf::NOLIGHT;
        }
        if flag("%compilehint") {
            info.hint = true;
            info.contents = 0;
            info.surface |= surf::HINT;
            hide(&mut info);
        }
        if flag("%compileskip") {
            info.skip = true;
            info.contents = 0;
            info.surface |= surf::SKIP;
            hide(&mut info);
        }
        if flag("%compileorigin") {
            info.contents = contents::ORIGIN;
            hide(&mut info);
        }
        if flag("%compileareaportal") {
            info.contents = contents::AREAPORTAL;
            hide(&mut info);
        }
        if flag("%compileladder") {
            info.contents = contents::LADDER;
            hide(&mut info);
        }
        if flag("%compilefog") {
            info.contents = 0;
            hide(&mut info);
        }
        if flag("%compilewater") || m.shader == "water" {
            info.contents = contents::WATER;
            info.surface |= surf::WARP | surf::NOLIGHT;
        }
        if flag("%compileslime") {
            info.contents = contents::SLIME;
        }
        if flag("%compileblocklos") {
            info.contents = CONTENTS_BLOCKLOS;
        }
        if flag("%compilenonsolid") {
            info.contents = 0;
        }
        if flag("%compiledetail") && info.contents != 0 {
            info.contents |= contents::DETAIL;
        }
        if flag("%compilenolight") || m.shader == "unlitgeneric" {
            info.surface |= surf::NOLIGHT;
        }
        let alpha = m.get("$alpha").and_then(|v| v.trim().parse::<f64>().ok());
        if info.draws && (m.translucent() || alpha.is_some_and(|a| a < 1.0)) {
            info.surface |= surf::TRANS;
            if info.contents & contents::SOLID != 0 {
                info.contents = (info.contents & !contents::SOLID) | contents::WINDOW;
            }
            info.contents |= contents::TRANSLUCENT;
        }
        if info.lit() && m.get("$bumpmap").is_some() {
            info.surface |= surf::BUMPLIGHT;
        }
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
