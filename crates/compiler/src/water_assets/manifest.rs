use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde::{Deserialize, Serialize};
use source_formats::bsp::{self, Bsp, PakArchive, PakEntry, PakLimits};
use source_formats::gamefs::asset_relative;

use super::reflections::LiquidReflections;
use super::{MAX_MANIFEST_BYTES, MAX_ROOTS, WaterAssetPlan, checkpoint, invalid};
use crate::Control;
use crate::asset_sources::hash;
use crate::water_dependencies::canonical_material_path;

pub const PACKED_WATER_MANIFEST_PATH: &str = "hammerr/packed-water.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaterPackedAsset {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaterAssetManifest {
    pub version: u32,
    pub roots: Vec<String>,
    pub channels: String,
    pub assets: Vec<WaterPackedAsset>,
}

impl WaterAssetPlan {
    pub fn manifest(&self) -> WaterAssetManifest {
        WaterAssetManifest {
            version: 1,
            roots: self.roots().to_vec(),
            channels: "ldr_hdr".into(),
            assets: self
                .sources()
                .iter()
                .map(|source| WaterPackedAsset {
                    path: source.path().into(),
                    sha256: source.sha256().into(),
                })
                .collect(),
        }
    }

    pub(super) fn manifest_entry(&self) -> io::Result<PakEntry> {
        let manifest = self.manifest();
        validate_manifest(&manifest, self.limits)?;
        let data = serde_json::to_vec(&manifest).map_err(io::Error::other)?;
        if data.len() > MAX_MANIFEST_BYTES {
            return Err(invalid("Packed Water manifest exceeds 2 MiB"));
        }
        Ok(PakEntry {
            name: PACKED_WATER_MANIFEST_PATH.into(),
            data,
        })
    }

    pub fn attach(&self, bsp: &mut Bsp, control: &Control) -> io::Result<WaterAssetManifest> {
        checkpoint(control)?;
        self.verify_source_metadata()?;
        let mut check = || checkpoint(control);
        let mut archive = bsp.pak_data_controlled(self.limits, &mut check)?;
        let existing = archive
            .entries
            .iter()
            .map(|entry| entry.name.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        if existing.contains(PACKED_WATER_MANIFEST_PATH)
            || self
                .entries()
                .iter()
                .any(|entry| existing.contains(&entry.name.to_ascii_lowercase()))
        {
            return Err(invalid(
                "Packed Water assets or manifest collide with existing packed content",
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

pub fn read_packed_water(bsp: &Bsp, control: &Control) -> io::Result<Option<WaterAssetManifest>> {
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
        .find(|entry| entry.name.eq_ignore_ascii_case(PACKED_WATER_MANIFEST_PATH))
    else {
        return Ok(None);
    };
    if entry.name != PACKED_WATER_MANIFEST_PATH || entry.data.len() > MAX_MANIFEST_BYTES {
        return Err(invalid("Invalid packed Water manifest path or size"));
    }
    let manifest: WaterAssetManifest = serde_json::from_slice(&entry.data)
        .map_err(|error| invalid(format!("Invalid packed Water manifest: {error}")))?;
    verify_bsp_size(
        bsp,
        bsp.lumps[bsp::lump::PAKFILE].data.len(),
        limits,
        control,
    )?;
    verify_archive(bsp, &archive, &manifest, limits, control)?;
    Ok(Some(manifest))
}

pub fn verify_packed_water(
    bsp: &Bsp,
    expected: &WaterAssetManifest,
    control: &Control,
) -> io::Result<()> {
    let actual = read_packed_water(bsp, control)?
        .ok_or_else(|| invalid("Packed Water manifest is missing"))?;
    if &actual != expected {
        return Err(invalid(
            "Packed Water manifest differs from the expected Water plan",
        ));
    }
    Ok(())
}

fn validate_manifest(manifest: &WaterAssetManifest, limits: PakLimits) -> io::Result<()> {
    if manifest.version != 1 || manifest.channels != "ldr_hdr" {
        return Err(invalid(
            "Unsupported packed Water manifest version or channels",
        ));
    }
    if manifest.roots.len() > MAX_ROOTS || manifest.assets.len() >= limits.max_entries {
        return Err(invalid(
            "Packed Water manifest exceeds its root or asset budget",
        ));
    }
    let mut previous: Option<&str> = None;
    for root in &manifest.roots {
        canonical_material_path(root)?;
        if root.len() > limits.max_name_bytes || previous.is_some_and(|old| old >= root.as_str()) {
            return Err(invalid(
                "Packed Water roots must be canonical, sorted and unique",
            ));
        }
        previous = Some(root);
    }
    previous = None;
    for asset in &manifest.assets {
        if asset.path.is_empty()
            || asset.path.len() > limits.max_name_bytes
            || asset_relative(&asset.path)? != asset.path
            || !asset.path.starts_with("materials/")
            || !asset.path.ends_with(".vmt") && !asset.path.ends_with(".vtf")
            || previous.is_some_and(|old| old >= asset.path.as_str())
        {
            return Err(invalid(
                "Packed Water asset paths must be canonical, sorted and unique",
            ));
        }
        if asset.sha256.len() != 64
            || !asset
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(invalid(
                "Packed Water asset hashes must be lowercase SHA-256 hex",
            ));
        }
        previous = Some(&asset.path);
    }
    Ok(())
}

fn verify_archive(
    bsp: &Bsp,
    archive: &PakArchive,
    manifest: &WaterAssetManifest,
    limits: PakLimits,
    control: &Control,
) -> io::Result<()> {
    checkpoint(control)?;
    validate_manifest(manifest, limits)?;
    let entries = archive
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.data.as_slice()))
        .collect::<BTreeMap<_, _>>();
    let generated = crate::cubemap_assets::read_generated_cubemaps(bsp, control)?;
    let reflections = LiquidReflections::new(generated.as_ref());
    let mut registered = BTreeSet::new();
    for asset in &manifest.assets {
        checkpoint(control)?;
        let bytes = entries
            .get(asset.path.as_str())
            .ok_or_else(|| invalid(format!("Packed Water asset is missing: {}", asset.path)))?;
        if hash(bytes) != asset.sha256 {
            return Err(invalid(format!(
                "Packed Water asset hash differs: {}",
                asset.path
            )));
        }
        registered.insert(asset.path.clone());
    }
    let mut roots = manifest.roots.iter().cloned().collect::<BTreeSet<_>>();
    let surfaces = water_surface_materials(bsp, control)?;
    if surfaces
        .iter()
        .any(|path| reflections.is_runtime_source(path))
    {
        return Err(invalid(
            "Runtime slime surface has no generated reflection material binding",
        ));
    }
    if surfaces.iter().any(|path| {
        reflections
            .original(path)
            .is_some_and(|original| !roots.contains(original))
    }) {
        return Err(invalid(
            "Generated slime surface has no registered original liquid root",
        ));
    }
    roots.extend(surfaces);
    let mut closure_paths = BTreeSet::new();
    for root in roots {
        let closure = reflections.collect(&root, control, &mut |path, _material, maximum| {
            checkpoint(control)?;
            if !registered.contains(path) && !reflections.contains(path) {
                return Err(invalid(format!(
                    "Water dependency is absent from its manifest: {path}"
                )));
            }
            let bytes = entries
                .get(path)
                .ok_or_else(|| invalid(format!("Packed Water dependency is missing: {path}")))?;
            if bytes.len() > maximum {
                return Err(invalid(format!(
                    "Packed Water dependency exceeds its byte budget: {path}"
                )));
            }
            if !reflections.contains(path) {
                reflections.verify_source(path, bytes)?;
            }
            Ok(bytes.to_vec())
        })?;
        closure_paths.extend(
            closure
                .into_iter()
                .filter(|path| !reflections.contains(path)),
        );
    }
    if closure_paths != registered {
        return Err(invalid(
            "Packed Water manifest does not equal its complete raw asset closure",
        ));
    }
    checkpoint(control)
}

fn water_surface_materials(bsp: &Bsp, control: &Control) -> io::Result<BTreeSet<String>> {
    for (index, maximum) in [
        (bsp::lump::LEAFWATERDATA, bsp::MAX_LEAF_WATER_RECORDS * 12),
        (bsp::lump::TEXINFO, 32_768 * 72),
        (bsp::lump::TEXDATA, 2048 * 32),
        (bsp::lump::TEXDATA_STRING_TABLE, 65_536 * 4),
        (bsp::lump::TEXDATA_STRING_DATA, 1024 * 1024),
    ] {
        checkpoint(control)?;
        let lump = &bsp.lumps[index];
        if lump.data.len() > maximum || lump.fourcc != [0; 4] {
            return Err(invalid(
                "Packed Water BSP texture/water record budget or encoding is unsupported",
            ));
        }
    }
    let records = bsp.records::<bsp::LeafWaterData>()?;
    let infos = bsp.records::<bsp::TexInfo>()?;
    let data = bsp.records::<bsp::TexData>()?;
    let names = bsp.texdata_strings()?;
    let mut roots = BTreeSet::new();
    for record in records {
        checkpoint(control)?;
        let info = usize::try_from(record.surface_texinfo)
            .ok()
            .and_then(|index| infos.get(index))
            .ok_or_else(|| invalid("Packed Water leaf-water surface texinfo is invalid"))?;
        let texture = usize::try_from(info.texdata)
            .ok()
            .and_then(|index| data.get(index))
            .ok_or_else(|| invalid("Packed Water surface texdata is invalid"))?;
        let name = usize::try_from(texture.name_index)
            .ok()
            .and_then(|index| names.get(index))
            .ok_or_else(|| invalid("Packed Water surface material name index is invalid"))?;
        let path = crate::water_dependencies::logical_material_path(&name.to_ascii_lowercase())?;
        roots.insert(path);
    }
    Ok(roots)
}

fn verify_bsp_size(
    bsp: &Bsp,
    pak_bytes: usize,
    limits: PakLimits,
    control: &Control,
) -> io::Result<()> {
    crate::bsp_size::verify(bsp, pak_bytes, None, limits.max_bsp_bytes, control)
        .map_err(|error| error.into_io("Packed Water"))
}
