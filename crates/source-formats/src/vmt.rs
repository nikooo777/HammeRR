//! Valve Material Type (VMT) files: a shader name plus parameters.

mod texture_transform;
pub use texture_transform::{
    DuplicateOperations, TransformError, texture_transform, texture_transform_matrix,
};

mod compile;
pub use compile::CompileSemantics;

use std::io;

use crate::gamefs::{GameFs, normalize};
use crate::keyvalues::{self, Node, Value};

/// How many `patch` includes to follow before assuming a cycle.
const MAX_PATCH_DEPTH: usize = 8;
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
    pub fn load_with_reader(
        path: &str,
        read: &mut dyn FnMut(&str) -> io::Result<Option<Vec<u8>>>,
    ) -> Result<Option<Self>, Error> {
        Self::load_depth(&normalize(path), 0, read)
    }

    fn load_depth(
        path: &str,
        depth: usize,
        read: &mut dyn FnMut(&str) -> io::Result<Option<Vec<u8>>>,
    ) -> Result<Option<Self>, Error> {
        if depth > MAX_PATCH_DEPTH {
            return Err(Error::PatchTooDeep);
        }
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
        let mut base = Self::load_depth(&normalize(include), depth + 1, read)?
            .ok_or_else(|| Error::MissingInclude(include.to_owned()))?;
        for (section, only_existing) in [("insert", false), ("replace", true)] {
            if let Some(children) = children
                .iter()
                .find(|node| node.key.eq_ignore_ascii_case(section))
                .and_then(Node::children)
            {
                base.apply(children, only_existing);
            }
        }
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
    fn scalar_patch_presence_matches_runtime_insert_and_replace() {
        for (section, existing, expected) in [
            ("insert", true, Some("1")),
            ("insert", false, Some("1")),
            ("replace", true, Some("1")),
            ("replace", false, None),
        ] {
            let base = format!(
                "LightmappedGeneric {{ {} }}",
                if existing { "%compilenodraw 0" } else { "" }
            );
            let patch =
                format!("Patch {{ include materials/base.vmt {section} {{ %compilenodraw 1 }} }}");
            let material = Material::load_with_reader("materials/patch.vmt", &mut |path| {
                Ok(Some(if path == "materials/base.vmt" {
                    base.as_bytes().to_vec()
                } else {
                    patch.as_bytes().to_vec()
                }))
            })
            .unwrap()
            .unwrap();
            assert_eq!(material.get("%compilenodraw"), expected);
            assert_eq!(material.compile_semantics().draws, expected != Some("1"));
        }
        for sections in [
            "insert { $value inserted } replace { $value replaced }",
            "replace { $value replaced } insert { $value inserted }",
        ] {
            let patch = format!("Patch {{ include materials/base.vmt {sections} }}");
            let material = Material::load_with_reader("materials/patch.vmt", &mut |path| {
                Ok(Some(if path == "materials/base.vmt" {
                    b"LightmappedGeneric { $value original }".to_vec()
                } else {
                    patch.as_bytes().to_vec()
                }))
            })
            .unwrap()
            .unwrap();
            assert_eq!(material.get("$value"), Some("replaced"));
        }
    }

    #[test]
    fn alpha_testing_changes_contents_without_translucent_sorting() {
        use crate::bsp::{contents, surf};
        for (parameters, contents, sorting) in [
            ("", contents::SOLID, false),
            (
                "$alphatest 1",
                contents::WINDOW | contents::TRANSLUCENT,
                false,
            ),
            (
                "$translucent 1",
                contents::WINDOW | contents::TRANSLUCENT,
                true,
            ),
            ("$alpha 0.5", contents::WINDOW | contents::TRANSLUCENT, true),
            ("$alphatest 1 %compilenonsolid 1", contents::OPAQUE, false),
            ("$alphatest 1 %compileblocklos 1", contents::BLOCKLOS, false),
            (
                "$alphatest 1 %compileplayerclip 1",
                contents::PLAYERCLIP,
                false,
            ),
        ] {
            let material =
                Material::parse(&format!("LightmappedGeneric {{ {parameters} }}")).unwrap();
            let semantics = material.compile_semantics();
            assert_eq!(semantics.contents, contents, "{parameters}");
            assert_eq!(
                semantics.surface & surf::TRANS != 0,
                sorting,
                "{parameters}"
            );
        }
    }

    fn nested_material(depth: usize) -> String {
        "UnlitGeneric { ".to_owned()
            + &"nested { ".repeat(depth - 1)
            + "$basetexture test "
            + &"} ".repeat(depth)
    }

    fn many_parameter_material(count: usize) -> String {
        let mut text = "UnlitGeneric { ".to_owned();
        for index in 0..count {
            text.push_str(&format!("$parameter_{index} value "));
        }
        text.push('}');
        text
    }

    fn syntax_kind(result: Result<Material, Error>) -> keyvalues::ParseErrorKind {
        let Err(Error::Syntax(error)) = result else {
            panic!("expected a bounded material syntax error");
        };
        error.kind
    }

    #[test]
    fn bounded_material_parse_rejects_deep_blocks_and_excessive_parameters() {
        assert!(Material::parse(&nested_material(MAX_MATERIAL_DEPTH)).is_ok());
        assert_eq!(
            syntax_kind(Material::parse(&nested_material(100_000))),
            keyvalues::ParseErrorKind::NestingLimit
        );
        let parameters = many_parameter_material(MAX_MATERIAL_NODES - 1);
        assert_eq!(
            Material::parse(&parameters).unwrap().params.len(),
            MAX_MATERIAL_NODES - 1
        );
        assert_eq!(
            syntax_kind(Material::parse(&many_parameter_material(100_000))),
            keyvalues::ParseErrorKind::NodeLimit
        );
    }

    #[test]
    fn bounded_material_parse_retains_bom_comments_and_unclosed_blocks() {
        let material = Material::parse(
            "\u{feff}UnlitGeneric { /* ignored { $bad 1 } */ $basetexture test // comment\nProxies { AnimatedTexture { animatedtexturevar $basetexture",
        ).unwrap();
        assert_eq!(material.shader, "unlitgeneric");
        assert_eq!(material.get("$basetexture"), Some("test"));
        assert_eq!(material.get("$bad"), None);
        let proxies = material.blocks[0].children().unwrap();
        assert_eq!(material.blocks[0].key, "Proxies");
        assert_eq!(proxies[0].key, "AnimatedTexture");
        assert_eq!(
            proxies[0].children().unwrap()[0].as_str(),
            Some("$basetexture")
        );
    }

    #[test]
    fn bounded_material_load_checks_every_patch_file_and_preserves_patch_overrides() {
        let patch = b"Patch { /* comment */ include materials/base.vmt replace { $basetexture replacement } insert { $basetexture ignored $new added";
        let mut reads = Vec::new();
        let material = Material::load_with_reader("materials/patch.vmt", &mut |path| {
            reads.push(path.to_owned());
            Ok(Some(match path {
                "materials/patch.vmt" => patch.to_vec(),
                "materials/base.vmt" => {
                    b"UnlitGeneric { $basetexture original $unchanged yes".to_vec()
                }
                _ => panic!("unexpected material path {path}"),
            }))
        })
        .unwrap()
        .unwrap();
        assert_eq!(reads, ["materials/patch.vmt", "materials/base.vmt"]);
        assert_eq!(material.shader, "unlitgeneric");
        assert_eq!(material.get("$basetexture"), Some("replacement"));
        assert_eq!(material.get("$unchanged"), Some("yes"));
        assert_eq!(material.get("$new"), Some("added"));
        for (text, expected) in [
            (
                nested_material(100_000),
                keyvalues::ParseErrorKind::NestingLimit,
            ),
            (
                many_parameter_material(100_000),
                keyvalues::ParseErrorKind::NodeLimit,
            ),
        ] {
            for invalid_path in ["materials/patch.vmt", "materials/base.vmt"] {
                let result = Material::load_with_reader("materials/patch.vmt", &mut |path| {
                    Ok(Some(if path == invalid_path {
                        text.as_bytes().to_vec()
                    } else {
                        patch.to_vec()
                    }))
                })
                .and_then(|material| material.ok_or(Error::NoShader));
                assert_eq!(syntax_kind(result), expected);
            }
        }
    }

    #[test]
    fn bounded_material_load_checks_patch_includes() {
        use crate::gamefs::{Mount, MountedPath};
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("materials")).unwrap();
        std::fs::write(
            directory.path().join("materials/base.vmt"),
            "VertexLitGeneric { $alphatest 1 }".to_owned() + &" ".repeat(256),
        )
        .unwrap();
        std::fs::write(
            directory.path().join("materials/patch.vmt"),
            "Patch { include materials/base.vmt replace { $alphatest 0 } }",
        )
        .unwrap();
        let game = GameFs {
            game_dir: directory.path().to_owned(),
            base_dir: directory.path().to_owned(),
            mounts: vec![MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Directory(directory.path().to_owned()),
            }],
        };
        assert!(Material::load_limited(&game, "patch", 128).is_err());
        assert!(
            !Material::load_limited(&game, "patch", 512)
                .unwrap()
                .unwrap()
                .alpha_test()
        );
        assert!(
            Material::load_limited(&game, "missing", 128)
                .unwrap()
                .is_none()
        );
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
