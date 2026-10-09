//! CPU-side material loading: VMT parameters plus the textures to display.

use source_formats::gamefs::GameFs;
use source_formats::vmt::{Material, texture_path};
use source_formats::vtf::Vtf;

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
    /// Neither the VMT nor its texture could be found.
    pub missing: bool,
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
            missing: base.is_none(),
            alpha_test: material.as_ref().is_some_and(Material::alpha_test),
            translucent: material.as_ref().is_some_and(Material::translucent),
            tool: material.as_ref().is_some_and(Material::is_tool),
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
            name: name.to_owned(),
            base: None,
            blend: None,
            width: 1,
            height: 1,
            alpha_test: false,
            translucent: false,
            tool: false,
            missing: false,
        }
    }
}
