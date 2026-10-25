use std::collections::{BTreeMap, BTreeSet};
use std::io;

use source_formats::bsp::{Bsp, PakArchive, PakEntry, PakLimits};
use source_formats::gamefs::{AssetOrigin, GameFs};

use super::closure::{Roots, collect_closure};
use super::manifest::{StaticPropPackedUse, verify_bsp_size};
use super::{MAX_MATERIAL_BYTES, PACKED_STATIC_PROPS_MANIFEST_PATH, checkpoint, invalid};
use crate::Control;
use crate::asset_sources::{AssetSourceGuard, BuildMaterialSources, CapturedBuildAsset, hash};

#[derive(Debug, Clone)]
pub struct StaticPropAssetSourceSnapshot {
    path: String,
    sha256: String,
    origin: AssetOrigin,
    bytes: usize,
}

impl StaticPropAssetSourceSnapshot {
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn origin(&self) -> &AssetOrigin {
        &self.origin
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

#[derive(Debug)]
pub struct StaticPropAssetPlan {
    pub(super) roots: Roots,
    entries: Vec<PakEntry>,
    sources: Vec<StaticPropAssetSourceSnapshot>,
    guard: AssetSourceGuard,
    combined_pak_bytes: usize,
    pub(super) limits: PakLimits,
}

impl StaticPropAssetPlan {
    pub fn roots(&self) -> &[String] {
        &self.roots.models
    }

    pub fn models(&self) -> &[String] {
        &self.roots.models
    }

    pub fn uses(&self) -> &[StaticPropPackedUse] {
        &self.roots.uses
    }

    pub fn entries(&self) -> &[PakEntry] {
        &self.entries
    }

    pub fn sources(&self) -> &[StaticPropAssetSourceSnapshot] {
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
                .ok_or_else(|| {
                    invalid(format!("Static-prop source disappeared: {}", source.path))
                })?;
            if current.origin != source.origin
                || current.data.len() != source.bytes
                || hash(&current.data) != source.sha256
            {
                return Err(invalid(format!(
                    "Static-prop original source changed before publication: {}",
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

pub fn collect_static_prop_asset_plan(
    bsp: &Bsp,
    game: &GameFs,
    sources: &BuildMaterialSources,
    existing: &PakArchive,
    limits: PakLimits,
    control: &Control,
) -> io::Result<StaticPropAssetPlan> {
    checkpoint(control)?;
    let limits = crate::pak_limits::bounded(limits);
    let roots = Roots::from_bsp(bsp)?;
    let mut check = || checkpoint(control);
    existing.to_bytes_controlled(limits, &mut check)?;
    let existing_names = existing
        .entries
        .iter()
        .map(|entry| entry.name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    if existing_names.contains(PACKED_STATIC_PROPS_MANIFEST_PATH) {
        return Err(invalid(
            "Packed static-prop manifest collides with existing packed content",
        ));
    }
    let existing_bytes = existing
        .entries
        .iter()
        .map(|entry| entry.data.len())
        .sum::<usize>();
    let mut assets = BTreeMap::<String, CapturedBuildAsset>::new();
    let mut retained_bytes = 0usize;
    let closure = collect_closure(&roots, limits, control, &mut |path, maximum, original| {
        checkpoint(control)?;
        if existing_names.contains(&path.to_ascii_lowercase()) {
            return Err(invalid(format!(
                "Static-prop asset collides with existing packed content: {path}"
            )));
        }
        let retained = sources.retained(path)?;
        if original && retained.is_none() {
            return Err(invalid(format!(
                "Static-prop source was not retained by its original compiler read: {path}"
            )));
        }
        let remaining = limits
            .max_total_bytes
            .saturating_sub(existing_bytes)
            .saturating_sub(retained_bytes);
        let maximum = maximum.min(limits.max_entry_bytes);
        if !assets.contains_key(path)
            && assets.len() + existing.entries.len() >= limits.max_entries.saturating_sub(1)
        {
            return Err(invalid("Combined static-prop PAK exceeds its entry budget"));
        }
        let asset = if let Some(asset) = retained {
            Some(asset)
        } else {
            sources.read_asset(game, path, maximum.min(remaining))?
        };
        let Some(asset) = asset else {
            return Ok(None);
        };
        if asset.bytes().len() > maximum {
            return Err(invalid(format!(
                "Static-prop source exceeds its byte budget: {path}"
            )));
        }
        if let Some(old) = assets.get(path) {
            if old.bytes() != asset.bytes() || old.origin() != asset.origin() {
                return Err(invalid(format!(
                    "Conflicting original static-prop source observations: {path}"
                )));
            }
        } else {
            if asset.bytes().len() > remaining {
                return Err(invalid(
                    "Combined static-prop PAK exceeds its decoded byte budget",
                ));
            }
            retained_bytes += asset.bytes().len();
            assets.insert(path.to_owned(), asset.clone());
        }
        Ok(Some(asset.bytes().to_vec()))
    })?;
    if closure.iter().any(|path| !assets.contains_key(path)) || closure.len() != assets.len() {
        return Err(invalid(
            "Static-prop closure contains an unretained original asset",
        ));
    }
    let guard = sources.source_guard(game, control)?;
    let mut entries = Vec::with_capacity(assets.len());
    let mut snapshots = Vec::with_capacity(assets.len());
    for (path, asset) in assets {
        checkpoint(control)?;
        snapshots.push(StaticPropAssetSourceSnapshot {
            path: path.clone(),
            sha256: hash(asset.bytes()),
            origin: asset.origin().clone(),
            bytes: asset.bytes().len(),
        });
        entries.push(PakEntry {
            name: path,
            data: asset.bytes().to_vec(),
        });
    }
    let mut plan = StaticPropAssetPlan {
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
    verify_bsp_size(bsp, plan.combined_pak_bytes, limits, control)?;
    plan.verify_source_metadata()?;
    Ok(plan)
}
