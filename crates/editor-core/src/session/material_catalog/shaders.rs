use std::{collections::BTreeMap, io};

use source_formats::{
    gamefs::{GameFs, asset_relative},
    vmt::{Material, material_path},
};

use crate::materials::{
    MAX_CATALOG_ENTRIES, MAX_SHADER_BYTES, Provenance, ShaderClassification, ShaderFailure,
    ShaderScan, provenance,
};
use crate::session::material_source::{MAX_FILE_BYTES, MAX_PATH_BYTES, MAX_TOTAL_BYTES};

pub const MAX_SHADER_BATCH: usize = 128;
pub const MAX_SHADER_BATCH_BYTES: usize = 16 * 1024 * 1024;
const MAX_FAILURES: usize = 16;
const MAX_ERROR_CHARACTERS: usize = 512;
const MAX_SHADER_CACHE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
pub(in crate::session) struct ShaderIndex {
    epoch: Option<u64>,
    entries: BTreeMap<String, ShaderClassification>,
    bytes: usize,
}

fn entry_bytes(name: &str, entry: &ShaderClassification) -> usize {
    name.len()
        + entry.shader.as_ref().map_or(0, String::len)
        + entry.material_identity.as_ref().map_or(0, String::len)
        + entry.error.as_ref().map_or(0, String::len)
        + entry.provenance.source_id.as_ref().map_or(0, String::len)
        + entry.provenance.source.as_ref().map_or(0, |source| {
            source.mount.len()
                + source.file.as_ref().map_or(0, String::len)
                + source.storage_files.iter().map(String::len).sum::<usize>()
        })
}

fn classify(game: &GameFs, name: &str) -> (ShaderClassification, usize) {
    let root = material_path(name);
    let mut source = match game.asset_origin(&root) {
        Ok(origin) => origin.map(provenance).unwrap_or_default(),
        Err(error) => {
            return (
                ShaderClassification {
                    shader: None,
                    material_identity: None,
                    error: Some(
                        error
                            .to_string()
                            .chars()
                            .take(MAX_ERROR_CHARACTERS)
                            .collect(),
                    ),
                    provenance: Provenance::default(),
                },
                0,
            );
        }
    };
    let mut bytes = 0;
    let material = Material::load_with_reader(&root, &mut |path| {
        let path = asset_relative(path)?;
        if path.len() > MAX_PATH_BYTES {
            return Err(io::Error::other(
                "Shader classification source path exceeds its limit",
            ));
        }
        let remaining = MAX_TOTAL_BYTES - bytes;
        let asset = game.read_asset_limited(&path, remaining.min(MAX_FILE_BYTES))?;
        let Some(asset) = asset else {
            return Ok(None);
        };
        bytes += asset.data.len();
        if path == root {
            source = provenance(asset.origin);
        }
        Ok(Some(asset.data))
    });
    let result = match material {
        Ok(Some(material)) if material.shader.len() <= MAX_SHADER_BYTES => Ok(material),
        Ok(Some(_)) => Err(format!("Resolved shader exceeds {MAX_SHADER_BYTES} bytes")),
        Ok(None) => Err("Material VMT was not found in mounted content".into()),
        Err(error) => Err(error.to_string()),
    };
    let mut classification = ShaderClassification {
        shader: None,
        material_identity: None,
        error: None,
        provenance: source,
    };
    match result {
        Ok(material) => {
            classification.material_identity = Some(format!(
                "{:016x}",
                render::materials::material_identity(&material)
            ));
            classification.shader = Some(material.shader);
        }
        Err(error) => {
            classification.error = Some(error.chars().take(MAX_ERROR_CHARACTERS).collect())
        }
    }
    (classification, bytes)
}

impl ShaderIndex {
    pub(in crate::session) fn get(&self, name: &str) -> Option<&ShaderClassification> {
        self.entries.get(name)
    }

    pub(in crate::session) fn reset_epoch(&mut self, epoch: u64) {
        if self.epoch != Some(epoch) {
            self.entries.clear();
            self.bytes = 0;
            self.epoch = Some(epoch);
        }
    }

    pub(in crate::session) fn invalidate_changed_source(
        &mut self,
        name: &str,
        source: &Provenance,
    ) {
        if self
            .entries
            .get(name)
            .is_some_and(|entry| entry.provenance.source_id != source.source_id)
            && let Some(entry) = self.entries.remove(name)
        {
            self.bytes -= entry_bytes(name, &entry);
        }
    }

    pub(in crate::session) fn scan(
        &mut self,
        game: &GameFs,
        names: &[String],
    ) -> Result<ShaderScan, String> {
        if self.entries.len().saturating_add(names.len()) > MAX_CATALOG_ENTRIES {
            self.entries
                .retain(|name, _| names.binary_search(name).is_ok());
            self.bytes = self
                .entries
                .iter()
                .map(|(name, entry)| entry_bytes(name, entry))
                .sum();
        }
        let mut attempted = 0;
        let mut bytes = 0;
        for name in names {
            if self.entries.contains_key(name) {
                continue;
            }
            if attempted == MAX_SHADER_BATCH || MAX_SHADER_BATCH_BYTES - bytes < MAX_TOTAL_BYTES {
                break;
            }
            let (entry, used) = classify(game, name);
            bytes += used;
            let size = entry_bytes(name, &entry);
            if self.bytes.saturating_add(size) > MAX_SHADER_CACHE_BYTES {
                return Err("Shader classification cache exceeds 64 MiB of text. Narrow the name/source filters and Reload material assets before retrying".into());
            }
            self.bytes += size;
            self.entries.insert(name.clone(), entry);
            attempted += 1;
        }
        let mut scan = ShaderScan {
            required: true,
            candidates: names.len(),
            attempted_this_call: attempted,
            bytes_read_this_call: bytes,
            materials_per_call: MAX_SHADER_BATCH,
            bytes_per_call: MAX_SHADER_BATCH_BYTES,
            cache_policy: "Resolved declarations and their original winning VMT origins are cached until Reload material assets; content reload invalidates the index. Current catalog provenance is separate. Changed winning mounts invalidate observed cache entries. External VMT/include edits require Reload material assets.",
            ..ShaderScan::default()
        };
        for name in names {
            if let Some(entry) = self.entries.get(name) {
                scan.classified += 1;
                if let Some(error) = &entry.error {
                    scan.failed += 1;
                    if scan.failures.len() < MAX_FAILURES {
                        scan.failures.push(ShaderFailure {
                            material: name.clone(),
                            error: error.clone(),
                        });
                    }
                }
            }
        }
        scan.complete = scan.classified == scan.candidates;
        scan.matches_complete = scan.complete && scan.failed == 0;
        scan.failures_truncated = scan.failed > scan.failures.len();
        Ok(scan)
    }
}
