use std::collections::{BTreeMap, BTreeSet};
use std::io;

use super::{checkpoint, invalid};
use crate::Control;
use crate::cubemap_assets::{CubemapAliasAsset, CubemapAssetManifest};
use crate::water_dependencies::{GeneratedLiquidAssets, collect_water_closure};

pub(super) struct LiquidReflections<'a> {
    generated: GeneratedLiquidAssets,
    aliases: BTreeMap<String, Vec<&'a CubemapAliasAsset>>,
    originals: BTreeMap<String, String>,
    sources: BTreeMap<&'a str, &'a str>,
    manifest: Option<&'a CubemapAssetManifest>,
    paths: BTreeSet<String>,
}

impl<'a> LiquidReflections<'a> {
    pub fn new(manifest: Option<&'a CubemapAssetManifest>) -> Self {
        let mut result = Self {
            generated: GeneratedLiquidAssets::default(),
            aliases: BTreeMap::new(),
            originals: BTreeMap::new(),
            sources: BTreeMap::new(),
            manifest,
            paths: BTreeSet::new(),
        };
        let Some(manifest) = manifest else {
            return result;
        };
        if manifest.hdr_samples.is_empty() {
            return result;
        }
        for source in &manifest.sources {
            result.sources.insert(&source.path, &source.sha256);
        }
        for sample in manifest.samples.iter().chain(&manifest.hdr_samples) {
            result.generated.cubemaps.insert(sample.texture.clone());
            result.paths.insert(sample.texture.clone());
        }
        for alias in &manifest.aliases {
            let source = source_formats::vmt::material_path(&alias.material);
            result.generated.materials.insert(source.clone());
            result
                .aliases
                .entry(source.clone())
                .or_default()
                .push(alias);
            let path = source_formats::vmt::material_path(&alias.alias);
            result.originals.insert(path.clone(), source);
            result.paths.insert(path);
        }
        result
    }

    pub fn contains(&self, path: &str) -> bool {
        self.paths.contains(path)
    }

    pub fn is_runtime_source(&self, path: &str) -> bool {
        self.generated.materials.contains(path)
    }

    pub fn original(&self, path: &str) -> Option<&str> {
        self.originals.get(path).map(String::as_str)
    }

    pub fn verify_source(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        if let Some(expected) = self.sources.get(path)
            && crate::asset_sources::hash(bytes) != *expected
        {
            return Err(invalid(format!(
                "Liquid and reflection original source observations differ: {path}"
            )));
        }
        Ok(())
    }

    pub fn collect<F>(
        &self,
        root: &str,
        control: &Control,
        read: &mut F,
    ) -> io::Result<BTreeSet<String>>
    where
        F: FnMut(&str, bool, usize) -> io::Result<Vec<u8>>,
    {
        let closure = collect_water_closure(root, control, &self.generated, read)?;
        let mut paths = closure.paths;
        for material in closure.runtime_materials {
            checkpoint(control)?;
            if !self.sources.contains_key(material.as_str()) {
                return Err(invalid(format!(
                    "Runtime slime has no original reflection source receipt: {material}"
                )));
            }
            let aliases = self.aliases.get(&material).ok_or_else(|| {
                invalid(format!(
                    "Runtime slime has no bound reflection alias: {material}"
                ))
            })?;
            for alias in aliases {
                checkpoint(control)?;
                let sample = &self.manifest.unwrap().samples[alias.sample];
                let target = sample
                    .texture
                    .strip_prefix("materials/")
                    .unwrap()
                    .strip_suffix(".vtf")
                    .unwrap();
                if alias.envmaps.len() != 1 || alias.envmaps[0] != target {
                    return Err(invalid(format!(
                        "Runtime slime alias does not use its assigned reflection sample: {}",
                        alias.alias
                    )));
                }
                let generated = collect_water_closure(
                    &source_formats::vmt::material_path(&alias.alias),
                    control,
                    &self.generated,
                    read,
                )?;
                if !generated.runtime_materials.is_empty() {
                    return Err(invalid(
                        "Generated slime alias has unresolved runtime resources",
                    ));
                }
                paths.extend(generated.paths);
            }
        }
        Ok(paths)
    }
}
