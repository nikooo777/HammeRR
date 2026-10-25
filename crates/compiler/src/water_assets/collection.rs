use std::collections::{BTreeMap, BTreeSet};
use std::io;

use source_formats::bsp::{PakArchive, PakEntry, PakLimits};
use source_formats::gamefs::{AssetOrigin, GameFs};

use super::{MAX_ROOTS, PACKED_WATER_MANIFEST_PATH, checkpoint, invalid};
use crate::Control;
use crate::asset_sources::{AssetSourceGuard, BuildMaterialSources, CapturedBuildAsset, hash};
use crate::water_dependencies::{
    MAX_MATERIAL_BYTES, canonical_material_path, collect_static_water_closure,
};

#[derive(Debug, Clone)]
pub struct WaterAssetSourceSnapshot {
    path: String,
    sha256: String,
    origin: AssetOrigin,
}

impl WaterAssetSourceSnapshot {
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn origin(&self) -> &AssetOrigin {
        &self.origin
    }
}

#[derive(Debug)]
pub struct WaterAssetPlan {
    roots: Vec<String>,
    entries: Vec<PakEntry>,
    sources: Vec<WaterAssetSourceSnapshot>,
    guard: AssetSourceGuard,
    combined_pak_bytes: usize,
    pub(super) limits: PakLimits,
}

impl WaterAssetPlan {
    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    pub fn entries(&self) -> &[PakEntry] {
        &self.entries
    }

    pub fn sources(&self) -> &[WaterAssetSourceSnapshot] {
        &self.sources
    }

    pub fn asset_bytes(&self) -> usize {
        self.entries.iter().map(|entry| entry.data.len()).sum()
    }

    pub fn combined_pak_bytes(&self) -> usize {
        self.combined_pak_bytes
    }

    pub fn verify_source_metadata(&self) -> io::Result<()> {
        self.guard.verify_metadata()
    }

    pub fn verify_sources(&self, game: &GameFs, control: &Control) -> io::Result<()> {
        checkpoint(control)?;
        self.guard.verify_sources(game, control)?;
        for source in &self.sources {
            checkpoint(control)?;
            let maximum = if source.path.ends_with(".vmt") {
                MAX_MATERIAL_BYTES.min(self.limits.max_entry_bytes)
            } else {
                self.limits.max_entry_bytes
            };
            let current = game
                .read_asset_limited(&source.path, maximum)?
                .ok_or_else(|| invalid(format!("Water source disappeared: {}", source.path)))?;
            if current.origin != source.origin || hash(&current.data) != source.sha256 {
                return Err(invalid(format!(
                    "Water original source changed before publication: {}",
                    source.path
                )));
            }
        }
        checkpoint(control)?;
        self.guard.verify_sources(game, control)
    }

    pub fn capture_source_guard(
        &self,
        game: &GameFs,
        control: &Control,
    ) -> io::Result<AssetSourceGuard> {
        self.verify_sources(game, control)?;
        Ok(self.guard.clone())
    }
}

pub fn collect_water_asset_plan<'a>(
    roots: impl IntoIterator<Item = &'a str>,
    game: &GameFs,
    sources: &BuildMaterialSources,
    existing: &PakArchive,
    limits: PakLimits,
    control: &Control,
) -> io::Result<WaterAssetPlan> {
    checkpoint(control)?;
    let limits = crate::pak_limits::bounded(limits);
    let mut check = || checkpoint(control);
    existing.to_bytes_controlled(limits, &mut check)?;
    let mut selected = BTreeSet::new();
    for root in roots {
        checkpoint(control)?;
        canonical_material_path(root)?;
        if selected.len() >= MAX_ROOTS && !selected.contains(root) {
            return Err(invalid("Water packing exceeds 1024 root materials"));
        }
        if sources.retained(root)?.is_none() {
            return Err(invalid(format!(
                "Water root was not retained by the original compiler material read: {root}"
            )));
        }
        selected.insert(root.to_owned());
    }
    let roots = selected.into_iter().collect::<Vec<_>>();
    let existing_names = existing
        .entries
        .iter()
        .map(|entry| entry.name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    if existing_names.contains(PACKED_WATER_MANIFEST_PATH) {
        return Err(invalid(
            "Packed Water manifest collides with existing packed content",
        ));
    }
    let existing_bytes = existing
        .entries
        .iter()
        .map(|entry| entry.data.len())
        .sum::<usize>();
    let mut assets = BTreeMap::<String, CapturedBuildAsset>::new();
    let mut retained_bytes = 0usize;
    for root in &roots {
        let closure =
            collect_static_water_closure(root, control, &mut |path, _material, maximum| {
                checkpoint(control)?;
                if path.len() > limits.max_name_bytes {
                    return Err(invalid(format!(
                        "Water source exceeds the PAK name budget: {path}"
                    )));
                }
                let remaining = limits
                    .max_total_bytes
                    .saturating_sub(existing_bytes)
                    .saturating_sub(retained_bytes);
                let maximum = maximum.min(limits.max_entry_bytes);
                let asset = if let Some(asset) = sources.retained(path)? {
                    asset
                } else {
                    sources
                        .read_asset(game, path, maximum.min(remaining))?
                        .ok_or_else(|| invalid(format!("Missing Water source: {path}")))?
                };
                if asset.bytes().len() > maximum {
                    return Err(invalid(format!(
                        "Water source exceeds its byte budget: {path}"
                    )));
                }
                if let Some(original) = assets.get(path) {
                    if original.bytes() != asset.bytes() || original.origin() != asset.origin() {
                        return Err(invalid(format!(
                            "Conflicting original Water source observations: {path}"
                        )));
                    }
                } else {
                    if existing_names.contains(&path.to_ascii_lowercase()) {
                        return Err(invalid(format!(
                            "Water asset collides with existing packed content: {path}"
                        )));
                    }
                    if asset.bytes().len() > remaining || assets.len() >= limits.max_entries {
                        return Err(invalid(
                            "Combined Water PAK exceeds its asset or byte budget",
                        ));
                    }
                    retained_bytes += asset.bytes().len();
                    assets.insert(path.into(), asset.clone());
                }
                Ok(asset.bytes().to_vec())
            })?;
        if closure.paths.iter().any(|path| !assets.contains_key(path)) {
            return Err(invalid(
                "Water closure contains an unretained original asset",
            ));
        }
    }
    let guard = sources.source_guard(game, control)?;
    let mut entries = Vec::with_capacity(assets.len());
    let mut snapshots = Vec::with_capacity(assets.len());
    for (path, asset) in assets {
        checkpoint(control)?;
        snapshots.push(WaterAssetSourceSnapshot {
            path: path.clone(),
            sha256: hash(asset.bytes()),
            origin: asset.origin().clone(),
        });
        entries.push(PakEntry {
            name: path,
            data: asset.bytes().to_vec(),
        });
    }
    let mut plan = WaterAssetPlan {
        roots,
        entries,
        sources: snapshots,
        guard,
        combined_pak_bytes: 0,
        limits,
    };
    let mut combined = existing.clone();
    combined.entries.extend(plan.entries.iter().cloned());
    combined.entries.push(plan.manifest_entry()?);
    plan.combined_pak_bytes = combined.to_bytes_controlled(limits, &mut check)?.len();
    plan.verify_source_metadata()?;
    Ok(plan)
}
