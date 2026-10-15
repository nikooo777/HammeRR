//! Valve Material Type (VMT) files: a shader name plus parameters.

mod compile;
pub use compile::CompileSemantics;

use std::io;

use crate::gamefs::{GameFs, normalize};
use crate::keyvalues::{self, Node, Value};

/// How many `patch` includes to follow before assuming a cycle.
const MAX_PATCH_DEPTH: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Syntax(#[from] keyvalues::ParseError),
    #[error("material has no shader block")]
    NoShader,
    #[error("patch material includes {0:?}, which doesn't exist")]
    MissingInclude(String),
    #[error("patch materials nest too deeply")]
    PatchTooDeep,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    /// Shader name, lowercase (`lightmappedgeneric`, `unlitgeneric`, ...).
    pub shader: String,
    /// Top-level parameters with lowercase keys, in file order.
    pub params: Vec<(String, String)>,
    /// Nested blocks such as `proxies` or DirectX-level fallbacks.
    pub blocks: Vec<Node>,
}

impl Material {
    /// Parses a VMT that isn't a `patch`; use [`Material::load`] to resolve
    /// patches through the filesystem.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let nodes = keyvalues::parse_lenient(strip_bom(text))?;
        let (shader, children) = shader_block(&nodes)?;
        let mut material = Self {
            shader,
            params: Vec::new(),
            blocks: Vec::new(),
        };
        material.apply(children, true);
        Ok(material)
    }

    /// Loads `materials/<name>.vmt`, following `patch` includes.
    pub fn load(fs: &GameFs, name: &str) -> Result<Option<Self>, Error> {
        Self::load_limited(fs, name, usize::MAX)
    }

    pub fn load_limited(fs: &GameFs, name: &str, maximum: usize) -> Result<Option<Self>, Error> {
        Self::load_depth(fs, &material_path(name), 0, maximum)
    }

    fn load_depth(fs: &GameFs, path: &str, depth: usize, maximum: usize) -> Result<Option<Self>, Error> {
        if depth > MAX_PATCH_DEPTH {
            return Err(Error::PatchTooDeep);
        }
        let Some(bytes) = fs.read_limited(path, maximum)? else {
            return Ok(None);
        };
        let text = String::from_utf8_lossy(&bytes);
        let nodes = keyvalues::parse_lenient(strip_bom(&text))?;
        let (shader, children) = shader_block(&nodes)?;
        if shader != "patch" {
            return Self::parse(&text).map(Some);
        }

        let include = children
            .iter()
            .find(|n| n.key.eq_ignore_ascii_case("include"))
            .and_then(Node::as_str)
            .ok_or(Error::NoShader)?;
        let mut base = Self::load_depth(fs, &normalize(include), depth + 1, maximum)?
            .ok_or_else(|| Error::MissingInclude(include.to_owned()))?;
        for node in children {
            match (&node.value, node.key.to_ascii_lowercase().as_str()) {
                (Value::Block(c), "replace") => base.apply(c, true),
                (Value::Block(c), "insert") => base.apply(c, false),
                _ => {}
            }
        }
        Ok(Some(base))
    }

    fn apply(&mut self, children: &[Node], overwrite: bool) {
        for node in children {
            match &node.value {
                Value::String(v) => {
                    let key = node.key.to_ascii_lowercase();
                    match self.params.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, existing)) if overwrite => *existing = v.clone(),
                        Some(_) => {}
                        None => self.params.push((key, v.clone())),
                    }
                }
                Value::Block(_) => self.blocks.push(node.clone()),
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    fn flag(&self, key: &str) -> bool {
        self.get(key)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .is_some_and(|v| v != 0.0)
    }

    pub fn compile_flag(&self, key: &str) -> bool {
        self.get(key).is_some_and(|value| {
            let value = value.trim();
            value == "1" || value.eq_ignore_ascii_case("true")
        })
    }

    pub fn compile_semantics(&self) -> CompileSemantics {
        CompileSemantics::from_material(self)
    }

    /// Textures the editor could show, best first, as `.vtf` game paths:
    /// `%tooltexture` (tool, water and blend materials), then `$basetexture`.
    /// Shipped games often omit tool textures, so use the first that exists.
    pub fn editor_textures(&self) -> Vec<String> {
        ["%tooltexture", "$basetexture"]
            .into_iter()
            .filter_map(|key| self.get(key))
            .filter(|v| !v.trim().is_empty())
            .map(texture_path)
            .collect()
    }

    pub fn translucent(&self) -> bool {
        self.flag("$translucent")
    }

    pub fn alpha_test(&self) -> bool {
        self.flag("$alphatest")
    }

    /// Whether this is an editor-only surface that is invisible in game
    /// (clip, trigger, nodraw, skybox, ...). Water and other visible
    /// `%compile*` materials are not tools.
    pub fn is_tool(&self) -> bool {
        let semantics = self.compile_semantics();
        !semantics.draws
            || semantics.surface & crate::bsp::surf::SKY != 0
            || self.shader == "sky"
            || self.compile_flag("%compileinvisible")
            || self.compile_flag("%compileblocklight")
    }
}

fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

fn shader_block(nodes: &[Node]) -> Result<(String, &[Node]), Error> {
    nodes
        .iter()
        .find_map(|n| Some((n.key.to_ascii_lowercase(), n.children()?)))
        .ok_or(Error::NoShader)
}

/// `de_cbble/CobbleRoad01` -> `materials/de_cbble/cobbleroad01.vmt`.
pub fn material_path(name: &str) -> String {
    asset_path(name, "vmt")
}

/// `de_cbble\cobbleroad01` -> `materials/de_cbble/cobbleroad01.vtf`.
pub fn texture_path(name: &str) -> String {
    asset_path(name, "vtf")
}

fn asset_path(name: &str, extension: &str) -> String {
    let mut path = normalize(name.trim());
    if !path.starts_with("materials/") {
        path.insert_str(0, "materials/");
    }
    let suffix = format!(".{extension}");
    if !path.ends_with(&suffix) {
        path.push_str(&suffix);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_material_load_checks_patch_includes() {
        use crate::gamefs::{Mount, MountedPath};
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("materials")).unwrap();
        std::fs::write(directory.path().join("materials/base.vmt"), "VertexLitGeneric { $alphatest 1 }".to_owned() + &" ".repeat(256)).unwrap();
        std::fs::write(directory.path().join("materials/patch.vmt"), "Patch { include materials/base.vmt replace { $alphatest 0 } }").unwrap();
        let game = GameFs {
            game_dir: directory.path().to_owned(),
            base_dir: directory.path().to_owned(),
            mounts: vec![MountedPath { ids: vec!["game".into()], mount: Mount::Directory(directory.path().to_owned()) }],
        };
        assert!(Material::load_limited(&game, "patch", 128).is_err());
        assert!(!Material::load_limited(&game, "patch", 512).unwrap().unwrap().alpha_test());
        assert!(Material::load_limited(&game, "missing", 128).unwrap().is_none());
    }

    #[test]
    fn parses_tool_material() {
        let m = Material::parse(
            "\"LightmappedGeneric\"\n{\n\t// comment\n\t\"$basetexture\" \"Tools/toolsnodraw\"\n\t\"%compilenodraw\" 1\n}\n",
        )
        .unwrap();
        assert_eq!(m.shader, "lightmappedgeneric");
        assert_eq!(m.get("$BaseTexture"), Some("Tools/toolsnodraw"));
        assert!(m.is_tool());
        let water = Material::parse("Water\n{\n\t%compilewater 1\n}\n").unwrap();
        assert!(!water.is_tool());
        assert_eq!(m.editor_textures(), ["materials/tools/toolsnodraw.vtf"]);
    }

    #[test]
    fn prefers_tool_texture_for_display() {
        let m = Material::parse(
            "Water { $basetexture \"water/base\" %tooltexture \"dev/water_tool\" $translucent 1 }",
        )
        .unwrap();
        assert_eq!(
            m.editor_textures(),
            ["materials/dev/water_tool.vtf", "materials/water/base.vtf"]
        );
        assert!(m.translucent());
        assert!(!m.alpha_test());
    }

    #[test]
    fn builds_asset_paths() {
        assert_eq!(
            material_path("DE_CBBLE/COBBLEROAD01"),
            "materials/de_cbble/cobbleroad01.vmt"
        );
        assert_eq!(texture_path("materials\\x\\y.vtf"), "materials/x/y.vtf");
    }
}
