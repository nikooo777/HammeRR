use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde::{Deserialize, Serialize};
use source_formats::bsp::{self, Bsp, PakArchive, PakEntry, PakLimits};
use source_formats::keyvalues::{FlatBlockLimits, parse_flat_blocks_controlled};

use super::closure::{pak_path, validate_family, verify_closure};
use super::collection::invalid;
use super::{MAX_ASSETS, MAX_MATERIAL_BYTES, SkyAssetPlan};
use crate::Control;
use crate::asset_sources::hash;

pub const PACKED_SKY_MANIFEST_PATH: &str = "hammerr/packed-sky.json";
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_ENTITY_TOKEN_BYTES: usize = 2047;
const ENTITY_LIMITS: FlatBlockLimits = FlatBlockLimits {
    max_bytes: 4 * 1024 * 1024,
    max_blocks: 65_536,
    max_pairs: 262_144,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkyPackedAsset {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkyAssetManifest {
    pub version: u32,
    pub family: String,
    pub channels: String,
    pub assets: Vec<SkyPackedAsset>,
}

impl SkyAssetPlan {
    pub fn manifest(&self) -> SkyAssetManifest {
        SkyAssetManifest {
            version: 1,
            family: self.family().into(),
            channels: channel(self.has_hdr()).into(),
            assets: self
                .sources()
                .iter()
                .map(|source| SkyPackedAsset {
                    path: source.path().into(),
                    sha256: source.sha256().into(),
                })
                .collect(),
        }
    }

    pub fn attach(&self, bsp: &mut Bsp, control: &Control) -> io::Result<SkyAssetManifest> {
        checkpoint(control)?;
        let manifest = self.manifest();
        validate_manifest(&manifest, self.limits)?;
        let data = serde_json::to_vec(&manifest).map_err(io::Error::other)?;
        if data.len() > MAX_MANIFEST_BYTES {
            return Err(invalid("Packed sky manifest exceeds 64 KiB"));
        }
        let mut check = || checkpoint(control);
        let mut archive = bsp.pak_data_controlled(self.limits, &mut check)?;
        let existing = archive
            .entries
            .iter()
            .map(|entry| entry.name.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        if existing.contains(PACKED_SKY_MANIFEST_PATH)
            || self
                .entries()
                .iter()
                .any(|entry| existing.contains(&entry.name.to_ascii_lowercase()))
        {
            return Err(invalid(
                "Packed sky assets or manifest collide with existing packed content",
            ));
        }
        archive.entries.extend(self.entries().iter().cloned());
        archive.entries.push(PakEntry {
            name: PACKED_SKY_MANIFEST_PATH.into(),
            data,
        });
        let encoded = archive.to_bytes_controlled(self.limits, &mut check)?;
        verify_bsp_size(bsp, encoded.len(), self.limits, control)?;
        verify_archive(bsp, &archive, &manifest, self.limits, control)?;
        checkpoint(control)?;
        bsp.set_pak_data_controlled(&archive, self.limits, &mut check)?;
        Ok(manifest)
    }
}

pub fn read_packed_sky(bsp: &Bsp, control: &Control) -> io::Result<Option<SkyAssetManifest>> {
    checkpoint(control)?;
    if bsp.lumps.len() != bsp::LUMP_COUNT {
        return Err(invalid("BSP must have exactly 64 lump slots"));
    }
    let limits = PakLimits::default();
    let mut check = || checkpoint(control);
    let archive = bsp.pak_data_controlled(limits, &mut check)?;
    let Some(entry) = archive
        .entries
        .iter()
        .find(|entry| entry.name.eq_ignore_ascii_case(PACKED_SKY_MANIFEST_PATH))
    else {
        return Ok(None);
    };
    if entry.name != PACKED_SKY_MANIFEST_PATH || entry.data.len() > MAX_MANIFEST_BYTES {
        return Err(invalid("Invalid packed sky manifest path or size"));
    }
    let manifest: SkyAssetManifest = serde_json::from_slice(&entry.data)
        .map_err(|error| invalid(format!("Invalid packed sky manifest: {error}")))?;
    verify_bsp_size(
        bsp,
        bsp.lumps[bsp::lump::PAKFILE].data.len(),
        limits,
        control,
    )?;
    verify_archive(bsp, &archive, &manifest, limits, control)?;
    Ok(Some(manifest))
}

pub fn verify_packed_sky(
    bsp: &Bsp,
    expected: &SkyAssetManifest,
    control: &Control,
) -> io::Result<()> {
    let actual =
        read_packed_sky(bsp, control)?.ok_or_else(|| invalid("Packed sky manifest is missing"))?;
    if &actual != expected {
        return Err(invalid(
            "Packed sky manifest differs from the expected sky plan",
        ));
    }
    Ok(())
}

fn verify_archive(
    bsp: &Bsp,
    archive: &PakArchive,
    manifest: &SkyAssetManifest,
    limits: PakLimits,
    control: &Control,
) -> io::Result<()> {
    checkpoint(control)?;
    validate_manifest(manifest, limits)?;
    if world_family(bsp, control)? != manifest.family {
        return Err(invalid(
            "Packed sky family differs from the BSP world skyname",
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
        let bytes = entries
            .get(asset.path.as_str())
            .ok_or_else(|| invalid(format!("Packed sky asset is missing: {}", asset.path)))?;
        if hash(bytes) != asset.sha256 {
            return Err(invalid(format!(
                "Packed sky asset hash differs: {}",
                asset.path
            )));
        }
        registered.insert(asset.path.clone());
    }
    let closure = verify_closure(&manifest.family, limits, control, &mut |path, material| {
        checkpoint(control)?;
        if !registered.contains(path) {
            return Err(invalid(format!(
                "Sky dependency is absent from its manifest: {path}"
            )));
        }
        let bytes = entries
            .get(path)
            .ok_or_else(|| invalid(format!("Packed sky dependency is missing: {path}")))?;
        if material && bytes.len() > MAX_MATERIAL_BYTES {
            return Err(invalid(format!(
                "{path}: packed sky material exceeds 1 MiB"
            )));
        }
        Ok(bytes.to_vec())
    })?;
    if closure.paths != registered {
        return Err(invalid(
            "Packed sky manifest does not equal its complete raw asset closure",
        ));
    }
    if manifest.channels != channel(closure.hdr) {
        return Err(invalid(
            "Packed sky channels differ from its complete material closure",
        ));
    }
    checkpoint(control)
}

fn validate_manifest(manifest: &SkyAssetManifest, limits: PakLimits) -> io::Result<()> {
    if manifest.version != 1 || !matches!(manifest.channels.as_str(), "ldr" | "ldr_hdr") {
        return Err(invalid(
            "Unsupported packed sky manifest version or channels",
        ));
    }
    validate_family(&manifest.family)?;
    if manifest.assets.is_empty() || manifest.assets.len() > MAX_ASSETS {
        return Err(invalid("Packed sky manifest requires 1–128 assets"));
    }
    let mut previous: Option<&str> = None;
    for asset in &manifest.assets {
        let extension = if asset.path.ends_with(".vmt") {
            "vmt"
        } else {
            "vtf"
        };
        if pak_path(&asset.path, extension, limits)? != asset.path
            || previous.is_some_and(|path| path >= asset.path.as_str())
        {
            return Err(invalid(
                "Packed sky asset paths must be canonical, sorted and unique",
            ));
        }
        if asset.sha256.len() != 64
            || !asset
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(invalid(
                "Packed sky asset hashes must be lowercase SHA-256 hex",
            ));
        }
        previous = Some(&asset.path);
    }
    Ok(())
}

fn world_family(bsp: &Bsp, control: &Control) -> io::Result<String> {
    checkpoint(control)?;
    let entity = bsp
        .lumps
        .get(bsp::lump::ENTITIES)
        .ok_or_else(|| invalid("BSP entity lump is missing"))?;
    if entity.version != 0
        || entity.fourcc != [0; 4]
        || entity.data.len() > ENTITY_LIMITS.max_bytes + 1
    {
        return Err(invalid(
            "Packed sky verification requires an uncompressed entity lump of at most 4 MiB",
        ));
    }
    if let Some(end) = entity.data.iter().position(|&byte| byte == 0)
        && entity.data[end..].iter().any(|&byte| byte != 0)
    {
        return Err(invalid(
            "BSP entity lump has nonzero bytes after its terminator",
        ));
    }
    let mut check = || checkpoint(control);
    let blocks = parse_flat_blocks_controlled(bsp.entities()?, ENTITY_LIMITS, &mut check)?;
    let mut family = None;
    let mut worlds = 0;
    for (index, block) in blocks.iter().enumerate() {
        checkpoint(control)?;
        for (key, value) in block {
            checkpoint(control)?;
            if key.len() > MAX_ENTITY_TOKEN_BYTES || value.len() > MAX_ENTITY_TOKEN_BYTES {
                return Err(invalid(
                    "Packed sky entity token exceeds HammeRR's 2047-byte admission limit",
                ));
            }
            if [key.as_str(), value.as_str()]
                .into_iter()
                .any(|token| token == "{" || token.starts_with('}'))
            {
                return Err(invalid(
                    "Packed sky entity tokens cannot be '{' or begin with '}'",
                ));
            }
            if key.eq_ignore_ascii_case("classname") && key != "classname" {
                return Err(invalid(
                    "Packed sky entity classname keys must use canonical lowercase spelling",
                ));
            }
            let trimmed = key.trim_end_matches(' ');
            if trimmed != key
                && (trimmed.eq_ignore_ascii_case("classname")
                    || trimmed.eq_ignore_ascii_case("skyname"))
            {
                return Err(invalid(
                    "Packed sky entity identity keys cannot contain trailing ASCII spaces",
                ));
            }
        }
        let classes = block.iter().filter(|(key, _)| key == "classname");
        let class_values = classes.map(|(_, value)| value.as_str()).collect::<Vec<_>>();
        if class_values
            .iter()
            .any(|value| value.eq_ignore_ascii_case("worldspawn"))
        {
            worlds += 1;
            if index != 0 || class_values.len() != 1 {
                return Err(invalid(
                    "Packed sky requires one first worldspawn with one classname",
                ));
            }
            let names = block
                .iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case("skyname"))
                .map(|(_, value)| value.as_str())
                .collect::<Vec<_>>();
            if names.len() != 1 {
                return Err(invalid(
                    "Packed sky worldspawn requires exactly one skyname",
                ));
            }
            family = Some(names[0].to_owned());
        }
    }
    if worlds != 1 {
        return Err(invalid("Packed sky requires exactly one first worldspawn"));
    }
    family.ok_or_else(|| invalid("Packed sky world skyname is missing"))
}

fn verify_bsp_size(
    bsp: &Bsp,
    pak_bytes: usize,
    limits: PakLimits,
    control: &Control,
) -> io::Result<()> {
    crate::bsp_size::verify(bsp, pak_bytes, None, limits.max_bsp_bytes, control)
        .map_err(|error| error.into_io("Packed sky"))
}

fn channel(hdr: bool) -> &'static str {
    if hdr { "ldr_hdr" } else { "ldr" }
}

fn checkpoint(control: &Control) -> io::Result<()> {
    control.checkpoint().map_err(io::Error::other)
}

#[cfg(test)]
mod tests;
