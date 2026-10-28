//! Valve Material Type (VMT) files: a shader name plus parameters.

mod texture_transform;
pub use texture_transform::{
    DuplicateOperations, TransformError, texture_transform, texture_transform_matrix,
};

mod compile;
pub use compile::CompileSemantics;

use std::io;

use crate::gamefs::{GameFs, asset_relative, normalize};
use crate::keyvalues::{self, Node, Value};

/// Maximum number of Patch include edges followed from a root material.
pub const MAX_PATCH_DEPTH: usize = 8;
const MAX_MATERIAL_DEPTH: usize = 64;
const MAX_MATERIAL_NODES: usize = 4096;

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

#[derive(Default)]
struct PatchEdits {
    insert: Vec<Node>,
    replace: Vec<Node>,
}

impl Material {
    /// Parses a VMT that isn't a `patch`; use [`Material::load`] to resolve
    /// patches through the filesystem.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let nodes = keyvalues::parse_lenient_bounded(
            strip_bom(text),
            MAX_MATERIAL_DEPTH,
            MAX_MATERIAL_NODES,
        )?;
        let (shader, children) = shader_block(&nodes)?;
        Ok(Self::from_shader(shader, children))
    }

    fn from_shader(shader: String, children: &[Node]) -> Self {
        let mut material = Self {
            shader,
            params: Vec::new(),
            blocks: Vec::new(),
        };
        material.apply(children, false);
        material
    }

    /// Loads `materials/<name>.vmt`, following `patch` includes.
    pub fn load(fs: &GameFs, name: &str) -> Result<Option<Self>, Error> {
        Self::load_limited(fs, name, usize::MAX)
    }

    pub fn load_limited(fs: &GameFs, name: &str, maximum: usize) -> Result<Option<Self>, Error> {
        Self::load_with_reader(&material_path(name), &mut |path| {
            fs.read_limited(path, maximum)
        })
    }

    /// Loads a game-relative VMT path and its Patch includes through the reader.
    /// Sections accumulate outer to inner; all inserts apply before replacements.
    pub fn load_with_reader(
        path: &str,
        read: &mut dyn FnMut(&str) -> io::Result<Option<Vec<u8>>>,
    ) -> Result<Option<Self>, Error> {
        let mut edits = PatchEdits::default();
        let Some(mut material) = Self::load_depth(&asset_relative(path)?, 0, read, &mut edits)?
        else {
            return Ok(None);
        };
        material.apply(&edits.insert, false);
        material.apply(&edits.replace, true);
        Ok(Some(material))
    }

    fn load_depth(
        path: &str,
        depth: usize,
        read: &mut dyn FnMut(&str) -> io::Result<Option<Vec<u8>>>,
        edits: &mut PatchEdits,
    ) -> Result<Option<Self>, Error> {
        if depth > MAX_PATCH_DEPTH {
            return Err(Error::PatchTooDeep);
        }
        let include = {
            let Some(bytes) = read(path)? else {
                return Ok(None);
            };
            let text = String::from_utf8_lossy(&bytes);
            let nodes = keyvalues::parse_lenient_bounded(
                strip_bom(&text),
                MAX_MATERIAL_DEPTH,
                MAX_MATERIAL_NODES,
            )?;
            let (shader, children) = shader_block(&nodes)?;
            if shader != "patch" {
                return Ok(Some(Self::from_shader(shader, children)));
            }
            let include = children
                .iter()
                .find(|n| n.key.eq_ignore_ascii_case("include"))
                .and_then(Node::as_str)
                .ok_or(Error::NoShader)?;
            for (section, destination) in [
                ("insert", &mut edits.insert),
                ("replace", &mut edits.replace),
            ] {
                if let Some(children) = children
                    .iter()
                    .find(|node| node.key.eq_ignore_ascii_case(section))
                    .and_then(Node::children)
                {
                    destination.extend_from_slice(children);
                }
            }
            include.to_owned()
        };
        let base = Self::load_depth(&asset_relative(&include)?, depth + 1, read, edits)?
            .ok_or(Error::MissingInclude(include))?;
        Ok(Some(base))
    }

    fn apply(&mut self, children: &[Node], only_existing: bool) {
        for node in children {
            match &node.value {
                Value::String(v) => {
                    let key = node.key.to_ascii_lowercase();
                    match self.params.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, existing)) => *existing = v.clone(),
                        None if !only_existing => self.params.push((key, v.clone())),
                        None => {}
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

    pub fn primary_base_texture_key(&self) -> Result<Option<&'static str>, String> {
        let base = self.get("$basetexture");
        if !compile::is_lightmapped_shader(&self.shader) {
            return Ok(base.map(|_| "$basetexture"));
        }
        let (Some(base), Some(bump), Some(albedo)) =
            (base, self.get("$bumpmap"), self.get("$albedo"))
        else {
            return Ok(base.map(|_| "$basetexture"));
        };
        match self.get("$nodiffusebumplighting").map(str::trim) {
            Some("1") => return Ok(Some("$basetexture")),
            None | Some("0") => {}
            Some(_) => {
                return Err(
                    "Active $albedo selection requires $nodiffusebumplighting literal 0 or 1"
                        .into(),
                );
            }
        }
        for (key, value) in [
            ("$basetexture", base),
            ("$bumpmap", bump),
            ("$albedo", albedo),
        ] {
            if value.trim().is_empty() {
                return Err(format!(
                    "Active $albedo selection requires a nonblank {key} declaration"
                ));
            }
        }
        asset_relative(albedo.trim())
            .map_err(|error| format!("Selected $albedo texture path: {error}"))?;
        Ok(Some("$albedo"))
    }

    pub fn primary_base_texture(&self) -> Result<Option<&str>, String> {
        Ok(self
            .primary_base_texture_key()?
            .and_then(|key| self.get(key)))
    }

    /// Editor display candidates: intentional tool override, then effective base.
    pub fn editor_textures(&self) -> Result<Vec<String>, String> {
        Ok([self.get("%tooltexture"), self.primary_base_texture()?]
            .into_iter()
            .flatten()
            .filter(|v| !v.trim().is_empty())
            .map(texture_path)
            .collect())
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
