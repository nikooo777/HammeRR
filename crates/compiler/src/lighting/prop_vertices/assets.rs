use std::io;

use source_formats::{model::Model, vmt::Material};

use super::super::asset_tracking::Tracker;
use super::super::bake::{Allocation, Budget};
use crate::prop_lighting_assets::SourceAsset;
use crate::static_prop_assets::{MaterialDependency, material_dependency};

const MAX_MATERIAL_BYTES: usize = 64 * 1024;
type MaterialReader<'a> = dyn FnMut(&str, bool) -> io::Result<Option<Vec<u8>>> + 'a;

pub(super) struct Assets<'a> {
    tracker: Tracker<'a>,
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

impl<'a> Assets<'a> {
    pub fn new(tracker: Tracker<'a>) -> Self {
        Self { tracker }
    }

    pub fn required(&mut self, path: &str, budget: &mut Budget<'_>) -> Result<Vec<u8>, String> {
        self.tracker
            .read(path, source_formats::mdl::MAX_MODEL_BYTES, budget)?
            .ok_or_else(|| format!("Static-prop vertex source {path} was not found"))
    }

    pub fn material(
        &mut self,
        model: &Model,
        index: usize,
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        let name = model
            .materials
            .get(index)
            .ok_or("Static-prop skin material is unavailable")?;
        let name = name.strip_suffix(".vmt").unwrap_or(name);
        let _decoded = budget.reserve_bytes(MAX_MATERIAL_BYTES * 128)?;
        for candidate in
            source_formats::model::material_candidates(name, &model.material_directories)
        {
            let path = format!("materials/{candidate}.vmt");
            let material = load_material_observed(&path, &mut |path, charge| {
                let data = self
                    .tracker
                    .read(path, MAX_MATERIAL_BYTES, budget)
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
            if material.is_some() {
                return Ok(());
            }
        }
        Err(format!("Static-prop vertex material {name} was not found"))
    }

    pub fn finish(self) -> (Vec<SourceAsset>, Vec<String>, Allocation) {
        self.tracker.finish()
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
