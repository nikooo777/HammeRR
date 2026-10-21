use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde::{Deserialize, Serialize};
use source_formats::bsp::{self, Bsp, PakArchive, PakEntry, PakLimits};
use source_formats::gamefs::asset_relative;

use super::closure::{Roots, canonical_model_path, collect_closure};
use super::{MAX_MANIFEST_BYTES, StaticPropAssetPlan, checkpoint, invalid};
use crate::Control;
use crate::asset_sources::hash;

pub const PACKED_STATIC_PROPS_MANIFEST_PATH: &str = "hammerr/packed-static-props.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticPropPackedUse {
    pub model: u16,
    pub skin: i32,
    pub solid: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticPropPackedAsset {
    pub path: String,
    pub sha256: String,
    pub bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticPropAssetManifest {
    pub version: u32,
    pub static_props_version: u16,
    pub static_props_sha256: String,
    pub models: Vec<String>,
    pub uses: Vec<StaticPropPackedUse>,
    pub assets: Vec<StaticPropPackedAsset>,
}

impl StaticPropAssetPlan {
    pub fn manifest(&self) -> StaticPropAssetManifest {
        StaticPropAssetManifest {
            version: 1,
            static_props_version: self.roots.version,
            static_props_sha256: self.roots.sha256.clone(),
            models: self.roots.models.clone(),
            uses: self.roots.uses.clone(),
            assets: self
                .sources()
                .iter()
                .map(|source| StaticPropPackedAsset {
                    path: source.path().into(),
                    sha256: source.sha256().into(),
                    bytes: source.bytes(),
                })
                .collect(),
        }
    }

    pub(super) fn manifest_entry(&self) -> io::Result<PakEntry> {
        let manifest = self.manifest();
        validate_manifest(&manifest, self.limits)?;
        let data = serde_json::to_vec(&manifest).map_err(io::Error::other)?;
        if data.len() > MAX_MANIFEST_BYTES {
            return Err(invalid("Packed static-prop manifest exceeds 2 MiB"));
        }
        Ok(PakEntry {
            name: PACKED_STATIC_PROPS_MANIFEST_PATH.into(),
            data,
        })
    }

    pub fn attach(&self, bsp: &mut Bsp, control: &Control) -> io::Result<StaticPropAssetManifest> {
        checkpoint(control)?;
        self.verify_source_metadata()?;
        let mut check = || checkpoint(control);
        let mut archive = bsp.pak_data_controlled(self.limits, &mut check)?;
        let existing = archive
            .entries
            .iter()
            .map(|entry| entry.name.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        if existing.contains(PACKED_STATIC_PROPS_MANIFEST_PATH)
            || self
                .entries()
                .iter()
                .any(|entry| existing.contains(&entry.name.to_ascii_lowercase()))
        {
            return Err(invalid(
                "Packed static-prop assets or manifest collide with existing packed content",
            ));
        }
        archive.entries.extend(self.entries().iter().cloned());
        archive.entries.push(self.manifest_entry()?);
        let encoded = archive.to_bytes_controlled(self.limits, &mut check)?;
        verify_bsp_size(bsp, encoded.len(), self.limits, control)?;
        let manifest = self.manifest();
        verify_archive(bsp, &archive, &manifest, self.limits, control)?;
        checkpoint(control)?;
        bsp.set_pak_data_controlled(&archive, self.limits, &mut check)?;
        Ok(manifest)
    }
}

pub fn read_packed_static_props(
    bsp: &Bsp,
    control: &Control,
) -> io::Result<Option<StaticPropAssetManifest>> {
    checkpoint(control)?;
    if bsp.lumps.len() != bsp::LUMP_COUNT {
        return Err(invalid("BSP must have exactly 64 lump slots"));
    }
    let limits = PakLimits::default();
    let mut check = || checkpoint(control);
    let archive = bsp.pak_data_controlled(limits, &mut check)?;
    let Some(entry) = archive.entries.iter().find(|entry| {
        entry
            .name
            .eq_ignore_ascii_case(PACKED_STATIC_PROPS_MANIFEST_PATH)
    }) else {
        return Ok(None);
    };
    if entry.name != PACKED_STATIC_PROPS_MANIFEST_PATH || entry.data.len() > MAX_MANIFEST_BYTES {
        return Err(invalid("Invalid packed static-prop manifest path or size"));
    }
    let manifest: StaticPropAssetManifest = serde_json::from_slice(&entry.data)
        .map_err(|error| invalid(format!("Invalid packed static-prop manifest: {error}")))?;
    verify_bsp_size(
        bsp,
        bsp.lumps[bsp::lump::PAKFILE].data.len(),
        limits,
        control,
    )?;
    verify_archive(bsp, &archive, &manifest, limits, control)?;
    Ok(Some(manifest))
}

pub fn verify_packed_static_props(
    bsp: &Bsp,
    expected: &StaticPropAssetManifest,
    control: &Control,
) -> io::Result<()> {
    let actual = read_packed_static_props(bsp, control)?
        .ok_or_else(|| invalid("Packed static-prop manifest is missing"))?;
    if &actual != expected {
        return Err(invalid(
            "Packed static-prop manifest differs from the expected original asset plan",
        ));
    }
    Ok(())
}

fn validate_manifest(manifest: &StaticPropAssetManifest, limits: PakLimits) -> io::Result<()> {
    if manifest.version != 1 || !matches!(manifest.static_props_version, 0 | 4 | 5 | 6 | 10) {
        return Err(invalid("Unsupported packed static-prop manifest version"));
    }
    digest(&manifest.static_props_sha256)?;
    if manifest.models.len() > bsp::MAX_STATIC_PROP_MODELS
        || manifest.uses.len() > bsp::MAX_STATIC_PROPS
        || manifest.assets.len() >= limits.max_entries
    {
        return Err(invalid(
            "Packed static-prop manifest exceeds its model, record or asset budget",
        ));
    }
    let mut models = BTreeSet::new();
    for model in &manifest.models {
        canonical_model_path(model)?;
        if model.len() > limits.max_name_bytes || !models.insert(model) {
            return Err(invalid(
                "Packed static-prop model dictionary must be canonical and unique",
            ));
        }
    }
    for usage in &manifest.uses {
        if usize::from(usage.model) >= manifest.models.len()
            || usage.skin < 0
            || !matches!(usage.solid, 0 | 2 | 6)
        {
            return Err(invalid(
                "Invalid packed static-prop model, skin or solid use",
            ));
        }
    }
    let mut previous: Option<&str> = None;
    let mut bytes = 0usize;
    for asset in &manifest.assets {
        if asset.path.is_empty()
            || asset.path.len() > limits.max_name_bytes
            || asset_relative(&asset.path)? != asset.path
            || !(asset.path.starts_with("models/")
                && [".mdl", ".vvd", ".dx90.vtx", ".phy"]
                    .iter()
                    .any(|suffix| asset.path.ends_with(suffix))
                || asset.path.starts_with("materials/")
                    && (asset.path.ends_with(".vmt") || asset.path.ends_with(".vtf")))
            || previous.is_some_and(|old| old >= asset.path.as_str())
            || asset.bytes > limits.max_entry_bytes
        {
            return Err(invalid(
                "Packed static-prop assets must be canonical, sorted, unique and bounded",
            ));
        }
        bytes = bytes
            .checked_add(asset.bytes)
            .filter(|bytes| *bytes <= limits.max_total_bytes)
            .ok_or_else(|| invalid("Packed static-prop assets exceed their total byte budget"))?;
        digest(&asset.sha256)?;
        previous = Some(&asset.path);
    }
    Ok(())
}

fn digest(value: &str) -> io::Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(invalid(
            "Packed static-prop hashes must be lowercase SHA-256 hex",
        ));
    }
    Ok(())
}

fn verify_archive(
    bsp: &Bsp,
    archive: &PakArchive,
    manifest: &StaticPropAssetManifest,
    limits: PakLimits,
    control: &Control,
) -> io::Result<()> {
    checkpoint(control)?;
    validate_manifest(manifest, limits)?;
    let roots = Roots::from_bsp(bsp)?;
    if roots.version != manifest.static_props_version
        || roots.sha256 != manifest.static_props_sha256
        || roots.models != manifest.models
        || roots.uses != manifest.uses
    {
        return Err(invalid(
            "Packed static-prop manifest differs from the exact emitted static-prop records",
        ));
    }
    let entries = archive
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.data.as_slice()))
        .collect::<BTreeMap<_, _>>();
    let mut registered = BTreeSet::new();
    for asset in &manifest.assets {
        checkpoint(control)?;
        let bytes = entries.get(asset.path.as_str()).ok_or_else(|| {
            invalid(format!(
                "Packed static-prop asset is missing: {}",
                asset.path
            ))
        })?;
        if bytes.len() != asset.bytes || hash(bytes) != asset.sha256 {
            return Err(invalid(format!(
                "Packed static-prop original asset identity differs: {}",
                asset.path
            )));
        }
        registered.insert(asset.path.clone());
    }
    let closure = collect_closure(&roots, limits, control, &mut |path, maximum, _original| {
        checkpoint(control)?;
        let Some(bytes) = entries.get(path) else {
            return Ok(None);
        };
        if !registered.contains(path) || bytes.len() > maximum {
            return Err(invalid(format!(
                "Static-prop dependency is absent from its manifest or exceeds its budget: {path}"
            )));
        }
        Ok(Some(bytes.to_vec()))
    })?;
    if closure != registered {
        return Err(invalid(
            "Packed static-prop manifest does not equal its complete original asset closure",
        ));
    }
    checkpoint(control)
}

pub(super) fn verify_bsp_size(
    bsp: &Bsp,
    pak_bytes: usize,
    limits: PakLimits,
    control: &Control,
) -> io::Result<()> {
    if bsp.lumps.len() != bsp::LUMP_COUNT {
        return Err(invalid("BSP must have exactly 64 lump slots"));
    }
    let mut size = 8usize + bsp::LUMP_COUNT * 16 + 4;
    for (index, lump) in bsp.lumps.iter().enumerate() {
        checkpoint(control)?;
        size = size
            .checked_add(3)
            .map(|value| value & !3)
            .ok_or_else(|| invalid("Packed static-prop BSP size overflow"))?;
        let bytes = if index == bsp::lump::PAKFILE {
            pak_bytes
        } else if index == bsp::lump::GAME_LUMP {
            let directory = bsp
                .game_lumps
                .len()
                .checked_mul(16)
                .and_then(|value| value.checked_add(4))
                .ok_or_else(|| invalid("Packed static-prop game-lump size overflow"))?;
            bsp.game_lumps
                .iter()
                .try_fold(directory, |size, lump| size.checked_add(lump.data.len()))
                .ok_or_else(|| invalid("Packed static-prop game-lump size overflow"))?
        } else {
            lump.data.len()
        };
        size = size
            .checked_add(bytes)
            .ok_or_else(|| invalid("Packed static-prop BSP size overflow"))?;
        if size > limits.max_bsp_bytes {
            return Err(invalid("Packed static-prop BSP exceeds its byte limit"));
        }
    }
    Ok(())
}
