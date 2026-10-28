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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Control;

    const ROOT: &str = "materials/test/root.vmt";
    const TERMINAL: &str = "VertexLitGeneric { $basetexture test/mask $alphatest 1 }";

    fn resolve(sources: &[(&str, &str)]) -> Result<Material, String> {
        load_prop_vertex_material(ROOT, &mut |path| {
            assert!(path.ends_with(".vmt"));
            let Some((_, text)) = sources.iter().find(|(candidate, _)| *candidate == path) else {
                return Ok(None);
            };
            Ok(Some(text.as_bytes().to_vec()))
        })?
        .ok_or_else(|| "missing root".into())
    }

    #[test]
    fn cutout_receivers_require_the_original_plain_literal_terminal() {
        for text in [
            TERMINAL,
            "vertexlitgeneric { $AlphaTest \"1\" $BaseTexture test/mask }",
        ] {
            assert!(resolve(&[(ROOT, text)]).is_ok(), "{text}");
        }
        for body in [
            "$basetexture test/mask $alphatest 1.0",
            "$basetexture test/mask $alphatest +1",
            "$basetexture test/mask $alphatest 1e0",
            "$basetexture test/mask $alphatest \" 1 \"",
            "$basetexture test/mask $alphatest 1 $AlphaTest 1",
            "$basetexture test/mask $BaseTexture test/other $alphatest 1",
            "$basetexture test/mask $alphatest 1 $alphatestreference 0.5",
            "$basetexture test/mask $alphatest 1 $alphatestreference 0",
            "$basetexture test/mask $alphatest 1 $nocull 0",
            "$basetexture test/mask $alphatest 1 $model 1",
            "$basetexture test/mask $alphatest 1 $translucent 0",
            "$basetexture test/mask $alphatest 1 $basetexturetransform \"scale 1 1\"",
            "$basetexture test/mask $alphatest 1 Proxies {}",
            "$basetexture test/mask $alphatest 1 [$WIN32]",
            "$basetexture Test/Mask $alphatest 1",
            "$basetexture test/mask.vtf $alphatest 1",
        ] {
            let text = format!("VertexLitGeneric {{ {body} }}");
            assert!(resolve(&[(ROOT, &text)]).is_err(), "{text}");
        }
        for text in [
            "VertexLitGeneric { $basetexture test/mask $alphatest 1",
            "VertexLitGeneric { $basetexture test/mask $alphatest 1 } UnlitGeneric {}",
            "UnlitGeneric { $basetexture test/mask $alphatest 1 }",
        ] {
            assert!(resolve(&[(ROOT, text)]).is_err(), "{text}");
        }
    }

    #[test]
    fn cutout_patch_proof_reuses_packing_grammar_and_shared_nested_resolution() {
        for operation in ["insert", "replace"] {
            let patch = format!(
                "Patch {{ include materials/test/base.vmt {operation} {{ $basetexture test/replacement }} }}"
            );
            assert!(resolve(&[(ROOT, &patch), ("materials/test/base.vmt", TERMINAL)]).is_ok());
            assert!(
                resolve(&[
                    (ROOT, "Patch { include materials/test/middle.vmt }"),
                    ("materials/test/middle.vmt", &patch),
                    ("materials/test/base.vmt", TERMINAL),
                ])
                .is_ok()
            );
            assert!(
                resolve(&[
                    (ROOT, &patch),
                    (
                        "materials/test/base.vmt",
                        "Patch { include materials/test/terminal.vmt }",
                    ),
                    ("materials/test/terminal.vmt", TERMINAL),
                ])
                .is_ok()
            );
        }
        for (outer, inner, expected) in [
            ("insert", "replace", "test/inner"),
            ("replace", "insert", "test/outer"),
            ("insert", "insert", "test/inner"),
            ("replace", "replace", "test/inner"),
        ] {
            let outer = format!(
                "Patch {{ include materials/test/middle.vmt {outer} {{ $basetexture test/outer }} }}"
            );
            let inner = format!(
                "Patch {{ include materials/test/base.vmt {inner} {{ $basetexture test/inner }} }}"
            );
            let material = resolve(&[
                (ROOT, &outer),
                ("materials/test/middle.vmt", &inner),
                ("materials/test/base.vmt", TERMINAL),
            ])
            .unwrap();
            assert_eq!(material.get("$basetexture"), Some(expected));
            assert_eq!(material.get("$alphatest"), Some("1"));
        }
        for depth in [8, 9] {
            let mut files = vec![(
                ROOT.to_owned(),
                "Patch { include materials/test/0.vmt }".to_owned(),
            )];
            for index in 0..depth {
                let text = if index + 1 == depth {
                    TERMINAL.to_owned()
                } else {
                    format!(
                        "Patch {{ include materials/test/{}.vmt replace {{ $basetexture test/inner }} }}",
                        index + 1
                    )
                };
                files.push((format!("materials/test/{index}.vmt"), text));
            }
            let sources: Vec<_> = files
                .iter()
                .map(|(path, text)| (path.as_str(), text.as_str()))
                .collect();
            assert_eq!(resolve(&sources).is_ok(), depth == 8);
        }
        assert!(
            resolve(&[
                (
                    ROOT,
                    "Patch { include materials/test/base.vmt insert { $alphatest 1 } }"
                ),
                (
                    "materials/test/base.vmt",
                    "VertexLitGeneric { $basetexture test/mask }"
                ),
            ])
            .is_err()
        );
        for middle in [
            "Patch { include materials/test/base.vmt replace { $basetexture test/inner $BaseTexture test/duplicate } }",
            "Patch { include materials/test/base.vmt replace { $basetexture test/inner } insert { $basetexture test/other } }",
            "Patch { include materials/test/base.vmt insert { $alphatest 1 } }",
            "Patch { include materials/test/base.vmt replace { $basetexture test/inner } Proxies {} }",
            "Patch { include materials/test/base.vmt replace { $basetexture test/inner } [$WIN32] }",
            "Patch { include materials/test/base.vmt replace { $basetexture Test/Inner } }",
        ] {
            assert!(
                resolve(&[
                    (ROOT, "Patch { include materials/test/middle.vmt insert { $basetexture test/outer } }"),
                    ("materials/test/middle.vmt", middle),
                    ("materials/test/base.vmt", TERMINAL),
                ])
                .is_err(),
                "accepted {middle}"
            );
        }
        for include in [
            ROOT,
            "materials/test/middle.vmt",
            "materials/test/missing.vmt",
        ] {
            let middle =
                format!("Patch {{ include {include} replace {{ $basetexture test/inner }} }}");
            assert!(
                resolve(&[
                    (ROOT, "Patch { include materials/test/middle.vmt insert { $basetexture test/outer } }"),
                    ("materials/test/middle.vmt", &middle),
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn strict_cutout_proof_does_not_tighten_existing_opaque_materials() {
        for text in [
            "VertexLitGeneric {}",
            "VertexLitGeneric { $nocull 1 $model 1 $surfaceprop metal }",
            "VertexLitGeneric { $alpha 1.0 $alphatest 0.0 $translucent -0.0 }",
            "VertexLitGeneric { $basetexture test/mask $BaseTexture test/other }",
        ] {
            assert!(resolve(&[(ROOT, text)]).is_ok(), "{text}");
        }
        assert!(
            resolve(&[
                (
                    ROOT,
                    "Patch { include materials/test/base.vmt insert { $nocull 1 } }"
                ),
                ("materials/test/base.vmt", "VertexLitGeneric {}"),
            ])
            .is_ok()
        );
        assert!(
            resolve(&[
                (ROOT, "Patch { include materials/test/middle.vmt insert { $nocull 1 } }"),
                ("materials/test/middle.vmt", "Patch { include materials/test/base.vmt replace { $basetexture test/ignored } }"),
                ("materials/test/base.vmt", "VertexLitGeneric { $surfaceprop metal }"),
            ])
            .is_ok()
        );
        assert!(
            resolve(&[
                (
                    ROOT,
                    "Patch { include materials/test/base.vmt replace { $alphatest 0 } }"
                ),
                ("materials/test/base.vmt", TERMINAL),
            ])
            .is_ok()
        );
    }

    #[test]
    fn cutout_proof_charges_work_and_observes_cancellation_before_parsing() {
        let control = Control::default();
        let mut budget = Budget::new(&control);
        load_material_observed(ROOT, &mut |_path: &str, charge| {
            if charge {
                budget
                    .consume(TERMINAL.len().div_ceil(64) + 1)
                    .map_err(io::Error::other)?;
            }
            Ok(Some(TERMINAL.as_bytes().to_vec()))
        })
        .unwrap();
        assert_eq!(budget.work(), TERMINAL.len().div_ceil(64) + 1);
        control.cancel().unwrap();
        assert!(
            load_material_observed(ROOT, &mut |_path: &str, charge| {
                if charge {
                    budget
                        .consume(TERMINAL.len().div_ceil(64) + 1)
                        .map_err(io::Error::other)?;
                }
                Ok(Some(TERMINAL.as_bytes().to_vec()))
            })
            .unwrap_err()
            .to_string()
            .contains("cancelled")
        );
    }
}
