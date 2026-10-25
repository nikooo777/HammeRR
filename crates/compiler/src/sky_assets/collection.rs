use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;

use source_formats::bsp::{PakArchive, PakEntry, PakLimits};
use source_formats::gamefs::{AssetSourcePaths, GameFs};
use source_formats::vmf::Vmf;

use super::closure::{pak_path, validate_family, verify_closure};
use super::{MAX_ASSETS, MAX_MATERIAL_BYTES, MAX_TOTAL_BYTES, Reader, SkyAssetSource};
use crate::asset_sources::{AssetSourceGuard, SourceGuardPolicy, capture_asset_source_guard, hash};
use crate::{Control, FileStamp};

const MAX_PHYSICAL_PATHS: usize = 8192;

#[derive(Debug)]
pub struct SkyAssetSourceSnapshot {
    path: String,
    sha256: String,
    origin: SkyAssetSource,
    lookup: AssetSourcePaths,
}

impl SkyAssetSourceSnapshot {
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn origin(&self) -> &SkyAssetSource {
        &self.origin
    }
    pub fn lookup_paths(&self) -> &AssetSourcePaths {
        &self.lookup
    }
}

#[derive(Debug)]
pub struct SkyAssetSourceGuard {
    stamps: Vec<FileStamp>,
    guard: AssetSourceGuard,
}

impl SkyAssetSourceGuard {
    pub fn verify_metadata(&self) -> io::Result<()> {
        for stamp in &self.stamps {
            stamp.verify_metadata()?;
        }
        self.guard.verify_metadata()
    }
}

#[derive(Debug)]
pub struct SkyAssetPlan {
    family: String,
    hdr: bool,
    entries: Vec<PakEntry>,
    sources: Vec<SkyAssetSourceSnapshot>,
    stamps: Vec<FileStamp>,
    guard: AssetSourceGuard,
    combined_pak_bytes: usize,
    pub(super) limits: PakLimits,
}

impl SkyAssetPlan {
    pub fn family(&self) -> &str {
        &self.family
    }
    pub fn has_hdr(&self) -> bool {
        self.hdr
    }
    pub fn entries(&self) -> &[PakEntry] {
        &self.entries
    }
    pub fn sources(&self) -> &[SkyAssetSourceSnapshot] {
        &self.sources
    }
    pub fn asset_bytes(&self) -> usize {
        self.entries.iter().map(|entry| entry.data.len()).sum()
    }
    pub fn combined_pak_bytes(&self) -> usize {
        self.combined_pak_bytes
    }

    pub fn verify_source_metadata(&self) -> io::Result<()> {
        for stamp in &self.stamps {
            stamp.verify_metadata()?;
        }
        self.guard.verify_metadata()
    }

    pub fn capture_source_guard(
        &self,
        game: &GameFs,
        control: &Control,
    ) -> io::Result<SkyAssetSourceGuard> {
        self.verify_sources(game, control)?;
        Ok(SkyAssetSourceGuard {
            stamps: self.stamps.clone(),
            guard: self.guard.clone(),
        })
    }

    pub fn verify_sources(&self, game: &GameFs, control: &Control) -> io::Result<()> {
        control.checkpoint().map_err(io::Error::other)?;
        verify_stamps(&self.stamps, control)?;
        self.guard.verify_metadata()?;
        verify_lookups(&self.sources, game, control)?;
        capture_asset_source_guard(
            self.sources
                .iter()
                .map(|source| (source.path.as_str(), source.sha256.as_str())),
            [],
            game,
            control,
            guard_policy(),
        )?
        .verify_metadata()?;
        verify_lookups(&self.sources, game, control)?;
        verify_stamps(&self.stamps, control)?;
        self.guard.verify_metadata()
    }
}

struct Collector<'a> {
    reader: Reader<'a>,
    control: &'a Control,
    limits: PakLimits,
    existing_bytes: usize,
    sources: BTreeMap<String, SkyAssetSourceSnapshot>,
    stamps: BTreeMap<PathBuf, FileStamp>,
}

impl Collector<'_> {
    fn read(&mut self, path: &str, material: bool) -> io::Result<Vec<u8>> {
        self.control.checkpoint().map_err(io::Error::other)?;
        let path = pak_path(path, if material { "vmt" } else { "vtf" }, self.limits)?;
        if let Some(source) = self.sources.get(&path) {
            let current = self.reader.game.asset_source_paths(&path)?;
            if current.as_ref() != Some(&source.lookup) {
                return Err(invalid(format!("Sky source resolution changed: {path}")));
            }
            return self
                .reader
                .assets
                .get(&path)
                .and_then(|asset| asset.as_ref().ok())
                .and_then(Option::as_ref)
                .map(|asset| asset.data.clone())
                .ok_or_else(|| invalid("Cached sky source is unavailable"));
        }
        if self.sources.len() >= MAX_ASSETS {
            return Err(invalid("Sky closure exceeds 128 source lookups"));
        }
        let before = self
            .reader
            .game
            .asset_source_paths(&path)?
            .ok_or_else(|| invalid(format!("Missing sky source: {path}")))?;
        capture_paths(&before, &mut self.stamps, self.control)?;
        let remaining = self
            .limits
            .max_total_bytes
            .saturating_sub(self.existing_bytes)
            .saturating_sub(self.reader.bytes);
        let mut maximum = self
            .limits
            .max_entry_bytes
            .min(MAX_TOTAL_BYTES)
            .min(remaining);
        if material {
            maximum = maximum.min(MAX_MATERIAL_BYTES);
        }
        let asset = self
            .reader
            .read(&path, maximum)
            .map_err(invalid)?
            .ok_or_else(|| invalid(format!("Missing sky source: {path}")))?;
        let bytes = asset.data.clone();
        let origin = asset.source.clone();
        let after = self
            .reader
            .game
            .asset_source_paths(&path)?
            .ok_or_else(|| invalid(format!("Sky source disappeared: {path}")))?;
        if before != after {
            return Err(invalid(format!("Sky source resolution changed: {path}")));
        }
        capture_paths(&after, &mut self.stamps, self.control)?;
        self.sources.insert(
            path.clone(),
            SkyAssetSourceSnapshot {
                path,
                sha256: hash(&bytes),
                origin,
                lookup: before,
            },
        );
        Ok(bytes)
    }
}

/// Collects the supported six-face texture closure with VTF layout, source
/// identity and PAK budget checks. Scalar rendering behavior and HDR radiance
/// remain outside this validation.
pub fn collect_sky_asset_plan(
    vmf: &Vmf,
    game: &GameFs,
    existing: &PakArchive,
    limits: PakLimits,
    control: &Control,
) -> io::Result<SkyAssetPlan> {
    control.checkpoint().map_err(io::Error::other)?;
    let limits = crate::pak_limits::bounded(limits);
    let mut check = || control.checkpoint().map_err(io::Error::other);
    existing.to_bytes_controlled(limits, &mut check)?;
    let mut names = vmf.world.properties.get_all("skyname");
    let family = names
        .next()
        .ok_or_else(|| invalid("Set world skyname before collecting sky assets"))?;
    if names.next().is_some() {
        return Err(invalid("World has duplicate skyname keys"));
    }
    validate_family(family)?;
    let mut collector = Collector {
        reader: Reader::new(game),
        control,
        limits,
        existing_bytes: existing.entries.iter().map(|entry| entry.data.len()).sum(),
        sources: BTreeMap::new(),
        stamps: BTreeMap::new(),
    };
    let closure = verify_closure(family, limits, control, &mut |path, material| {
        collector.read(path, material)
    })?;
    let hdr = closure.hdr;
    let mut entries = Vec::new();
    let existing_names = existing
        .entries
        .iter()
        .map(|entry| entry.name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    for (path, asset) in &collector.reader.assets {
        control.checkpoint().map_err(io::Error::other)?;
        let asset = asset
            .as_ref()
            .map_err(|error| invalid(error.clone()))?
            .as_ref()
            .ok_or_else(|| invalid("Sky closure contains a missing source"))?;
        if existing_names.contains(&path.to_ascii_lowercase()) {
            return Err(invalid(format!(
                "Sky asset collides with existing packed content: {path}"
            )));
        }
        entries.push(PakEntry {
            name: path.clone(),
            data: asset.data.clone(),
        });
    }
    if entries.len() > MAX_ASSETS {
        return Err(invalid("Sky closure exceeds its asset budget"));
    }
    let mut combined = existing.clone();
    combined.entries.extend(entries.iter().cloned());
    let combined_pak_bytes = combined.to_bytes_controlled(limits, &mut check)?.len();
    let sources = collector.sources.into_values().collect::<Vec<_>>();
    let stamps = collector.stamps.into_values().collect::<Vec<_>>();
    verify_stamps(&stamps, control)?;
    verify_lookups(&sources, game, control)?;
    let guard = capture_asset_source_guard(
        sources
            .iter()
            .map(|source| (source.path.as_str(), source.sha256.as_str())),
        [],
        game,
        control,
        guard_policy(),
    )?;
    verify_lookups(&sources, game, control)?;
    verify_stamps(&stamps, control)?;
    Ok(SkyAssetPlan {
        family: family.into(),
        hdr,
        entries,
        sources,
        stamps,
        guard,
        combined_pak_bytes,
        limits,
    })
}

fn capture_paths(
    paths: &AssetSourcePaths,
    stamps: &mut BTreeMap<PathBuf, FileStamp>,
    control: &Control,
) -> io::Result<()> {
    for path in paths
        .files
        .iter()
        .chain(&paths.lookup_directories)
        .chain(&paths.lookup_symlinks)
    {
        control.checkpoint().map_err(io::Error::other)?;
        let stamp = FileStamp::capture_container(path)?;
        if let Some(old) = stamps.get(path) {
            if old != &stamp {
                return Err(invalid(format!(
                    "Sky backing file or lookup path changed: {}",
                    path.display()
                )));
            }
        } else {
            if stamps.len() >= MAX_PHYSICAL_PATHS {
                return Err(invalid(
                    "Sky source guard exceeds 8192 physical lookup paths",
                ));
            }
            stamps.insert(path.clone(), stamp);
        }
    }
    Ok(())
}

fn verify_stamps(stamps: &[FileStamp], control: &Control) -> io::Result<()> {
    for stamp in stamps {
        control.checkpoint().map_err(io::Error::other)?;
        stamp.verify_metadata()?;
    }
    Ok(())
}

fn verify_lookups(
    sources: &[SkyAssetSourceSnapshot],
    game: &GameFs,
    control: &Control,
) -> io::Result<()> {
    for source in sources {
        control.checkpoint().map_err(io::Error::other)?;
        if game.asset_source_paths(&source.path)?.as_ref() != Some(&source.lookup) {
            return Err(invalid(format!(
                "Sky source resolution changed: {}",
                source.path
            )));
        }
    }
    Ok(())
}

fn guard_policy() -> SourceGuardPolicy {
    SourceGuardPolicy {
        label: "Sky",
        missing_label: "Sky dependency",
        asset_bytes: MAX_TOTAL_BYTES,
        physical_paths: MAX_PHYSICAL_PATHS,
    }
}

pub(super) fn invalid(message: impl Into<String>) -> io::Error {
    let message = message.into();
    let (mut message, truncated) = super::bounded(&message, super::MAX_ISSUE_CHARACTERS);
    if truncated {
        message.push_str(" [truncated]");
    }
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests;
