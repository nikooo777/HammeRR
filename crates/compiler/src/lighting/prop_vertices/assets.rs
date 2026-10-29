use std::{io, rc::Rc, sync::Arc};

use glam::DVec3;
use source_formats::{gamefs::GameFs, model::Model, vmt::Material, vtf::Vtf};

use super::super::asset_tracking::Tracker;
use super::super::bake::{Allocation, Budget};
use crate::materials::metrics::{HeaderProvider, ReflectivityProvider};
use crate::static_prop_assets::{MaterialDependency, material_dependency};

const MAX_MATERIAL_BYTES: usize = 64 * 1024;
type MaterialReader<'a> = dyn FnMut(&str, bool) -> io::Result<Option<Vec<u8>>> + 'a;

pub(super) struct Assets<'fs, 'tracking> {
    inner: super::super::props::Assets<'fs, 'tracking>,
}

pub(super) struct SourceMaterial {
    pub(super) name: String,
    pub(super) base: String,
    pub(super) reflectivity: DVec3,
    pub(super) alpha: Option<Arc<super::super::props::alpha::Material>>,
    pub(super) two_sided: bool,
    _allocation: Allocation,
}

#[derive(Default)]
struct CutoutProof {
    terminal: bool,
    failure: Option<io::Error>,
}

impl CutoutProof {
    fn observe_source(&mut self, path: &str, bytes: &[u8]) {
        if self.failure.is_some() {
            return;
        }
        match material_dependency(path, bytes) {
            Ok(MaterialDependency::Texture(_)) => self.terminal = true,
            Ok(_) => {}
            Err(error) => self.failure = Some(error),
        }
    }

    fn validate(self, material: &Material) -> Result<(), String> {
        if material.get("$alphatest") != Some("1") {
            return validate_material(material);
        }
        if let Some(error) = self.failure {
            return Err(format!("Baked prop cutout material: {error}"));
        }
        if !self.terminal {
            return Err("Baked prop cutout material has no checked plain terminal".into());
        }
        Ok(())
    }
}

pub fn load_prop_vertex_material(
    path: &str,
    read: &mut dyn FnMut(&str) -> io::Result<Option<Vec<u8>>>,
) -> Result<Option<Material>, String> {
    load_material_observed(path, &mut |path, _| read(path))
}

fn load_material_observed(
    path: &str,
    read: &mut MaterialReader<'_>,
) -> Result<Option<Material>, String> {
    let mut cutout = CutoutProof::default();
    let material = Material::load_with_reader(path, &mut |path| {
        if !path.starts_with("materials/") || !path.ends_with(".vmt") {
            return Err(io::Error::other(
                "Static-prop material includes must name materials/*.vmt",
            ));
        }
        let data = read(path, cutout.failure.is_none())?;
        if let Some(data) = &data {
            if data.len() > MAX_MATERIAL_BYTES {
                return Err(io::Error::other(
                    "Static-prop material exceeds its byte budget",
                ));
            }
            let text = std::str::from_utf8(data).map_err(io::Error::other)?;
            let raw = Material::parse(text).map_err(io::Error::other)?;
            if raw.shader == "patch"
                && (raw.params.iter().any(|(key, _)| key != "include")
                    || raw.blocks.iter().any(|block| {
                        !matches!(
                            block.key.to_ascii_lowercase().as_str(),
                            "insert" | "replace"
                        )
                    }))
            {
                return Err(io::Error::other(
                    "Baked prop materials require plain Patch include/insert/replace semantics",
                ));
            }
            cutout.observe_source(path, data);
        }
        Ok(data)
    })
    .map_err(|error| format!("Static-prop vertex material {path}: {error}"))?;
    if let Some(material) = &material {
        cutout
            .validate(material)
            .map_err(|error| format!("{path}: {error}"))?;
    }
    Ok(material)
}

impl<'fs, 'tracking> Assets<'fs, 'tracking> {
    pub fn new(tracker: &'tracking mut Tracker<'fs>, fs: &'fs GameFs) -> Self {
        Self {
            inner: super::super::props::Assets::for_props(fs, None, Some(tracker)),
        }
    }

    pub fn required(&mut self, path: &str, budget: &mut Budget<'_>) -> Result<Vec<u8>, String> {
        self.inner.read(path, budget)
    }

    pub fn material(
        &mut self,
        model: &Model,
        index: usize,
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        let _decoded = budget.reserve_bytes(MAX_MATERIAL_BYTES * 128)?;
        self.find_material(model, index, budget).map(|_| ())
    }

    fn find_material(
        &mut self,
        model: &Model,
        index: usize,
        budget: &mut Budget<'_>,
    ) -> Result<(String, Material), String> {
        let name = model
            .materials
            .get(index)
            .ok_or("Static-prop skin material is unavailable")?;
        let name = name.strip_suffix(".vmt").unwrap_or(name);
        for candidate in
            source_formats::model::material_candidates(name, &model.material_directories)
        {
            let path = format!("materials/{candidate}.vmt");
            let material = load_material_observed(&path, &mut |path, charge| {
                let data = self
                    .inner
                    .read_material(path, budget)
                    .map_err(io::Error::other)?;
                if let Some(data) = &data
                    && charge
                {
                    budget
                        .consume(data.len().div_ceil(64) + 1)
                        .map_err(io::Error::other)?;
                }
                Ok(data)
            })?;
            if let Some(material) = material {
                return Ok((path, material));
            }
        }
        Err(format!("Static-prop vertex material {name} was not found"))
    }

    pub(super) fn source_material(
        &mut self,
        model: &Model,
        index: usize,
        budget: &mut Budget<'_>,
    ) -> Result<Rc<SourceMaterial>, String> {
        let _decoded = budget.reserve_bytes(MAX_MATERIAL_BYTES * 128)?;
        let (name, material) = self.find_material(model, index, budget)?;
        budget.consume(material.params.len() + 32)?;
        let primary = material
            .primary_base_texture_key()
            .map_err(|error| error.to_string())?;
        let key =
            primary.ok_or("Reflected prop sources require a selected original base texture")?;
        let base = HeaderProvider::new(&material, key)?;
        if base.runtime() {
            return Err(format!(
                "Reflected prop source {name} uses a runtime base texture"
            ));
        }
        let reflectivity = match ReflectivityProvider::new(&material, primary)? {
            ReflectivityProvider::CompileDefault => {
                return Err(format!(
                    "Reflected prop source {name} requires measured VTF or authored reflectivity"
                ));
            }
            ReflectivityProvider::Authored(value) => value,
            ReflectivityProvider::Header(provider) => {
                if provider.runtime() {
                    return Err(format!(
                        "Reflected prop source {name} uses runtime reflectivity"
                    ));
                }
                let bytes = self.inner.read(&provider.path, budget)?;
                budget.consume(32)?;
                let texture =
                    Vtf::parse(bytes).map_err(|error| format!("{}: {error}", provider.path))?;
                texture
                    .validate_native_mips()
                    .map_err(|error| format!("{}: {error}", provider.path))?;
                texture.reflectivity
            }
        };
        if reflectivity
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err(format!(
                "Reflected prop source {name} needs reflectivity between zero and one"
            ));
        }
        let alpha = super::super::props::alpha::prepare(&material, &mut self.inner, budget, false)?;
        let two_sided = material
            .get("$nocull")
            .is_some_and(|value| value.trim().parse::<f64>() == Ok(1.0));
        let allocation = budget.reserve_bytes(
            size_of::<SourceMaterial>() + 2 * size_of::<usize>() + name.len() + base.path.len(),
        )?;
        Ok(Rc::new(SourceMaterial {
            name,
            base: base.path,
            reflectivity: DVec3::from_array(reflectivity.map(f64::from)),
            alpha,
            two_sided,
            _allocation: allocation,
        }))
    }

    pub fn finish(self) -> Vec<Allocation> {
        self.inner.into_allocations()
    }
}

pub(super) fn validate_material(material: &Material) -> Result<(), String> {
    if material.shader != "vertexlitgeneric" || !material.blocks.is_empty() {
        return Err("Baked prop vertices require plain VertexLitGeneric without conditional blocks or proxies".into());
    }
    for (key, value) in &material.params {
        match key.as_str() {
            "$basetexture"
            | "$basetexturetransform"
            | "$surfaceprop"
            | "$surfaceprop2"
            | "$model"
            | "%keywords"
            | "%tooltexture" => {}
            "$reflectivity" => {
                crate::materials::parse_reflectivity(value)?;
            }
            "$alpha" if value.trim().parse::<f64>().ok() == Some(1.0) => {}
            "$nocull" if matches!(value.trim().parse::<f64>().ok(), Some(0.0 | 1.0)) => {}
            "$translucent" | "$alphatest" | "$additive" | "$selfillum" | "$vertexalpha"
            | "$vertexcolor" | "$halflambert" | "$phong" | "$rimlight"
                if value.trim().parse::<f64>().ok() == Some(0.0) => {}
            _ => {
                return Err(format!(
                    "Baked prop vertices require a plain opaque material; unsupported parameter {key}"
                ));
            }
        }
    }
    Ok(())
}
