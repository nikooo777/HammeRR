use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use source_formats::gamefs::{AssetSourcePaths, GameFs};

use crate::{Control, FileStamp};

mod build_materials;
mod material_inspection;

pub use build_materials::{
    BuildMaterialSources, CapturedBuildAsset, MAX_BUILD_MATERIAL_LOGICAL_PATHS,
    MAX_BUILD_MATERIAL_LOOKUP_ROWS, MAX_BUILD_MATERIAL_PHYSICAL_PATHS,
    MAX_BUILD_MATERIAL_RETAINED_BYTES,
};
pub(crate) use material_inspection::MaterialInspectionIssueKind;
pub use material_inspection::{CapturedMaterial, MaterialInspectionCapture};

#[derive(Debug, Clone)]
pub struct AssetSourceGuard {
    stamps: Vec<FileStamp>,
    material_lookups: BTreeMap<String, build_materials::BuildMaterialLookup>,
}

impl AssetSourceGuard {
    pub fn verify_metadata(&self) -> io::Result<()> {
        for stamp in &self.stamps {
            stamp.verify_metadata()?;
        }
        Ok(())
    }

    pub fn verify_sources(&self, game: &GameFs, control: &Control) -> io::Result<()> {
        control.checkpoint().map_err(io::Error::other)?;
        self.verify_metadata()?;
        for (path, lookup) in &self.material_lookups {
            control.checkpoint().map_err(io::Error::other)?;
            lookup.verify(game, path)?;
        }
        control.checkpoint().map_err(io::Error::other)?;
        self.verify_metadata()
    }

    pub fn verify_resolved_sources(&self, game: &GameFs, control: &Control) -> io::Result<()> {
        control.checkpoint().map_err(io::Error::other)?;
        if self.material_lookups.is_empty() {
            return Err(invalid(
                "Resolved source verification requires original logical lookups",
            ));
        }
        if self.material_lookups.len() > MAX_BUILD_MATERIAL_LOGICAL_PATHS
            || self.stamps.len() > MAX_BUILD_MATERIAL_PHYSICAL_PATHS
        {
            return Err(invalid(
                "Resolved source verification exceeds the source ownership limits",
            ));
        }
        let mut directories = Vec::new();
        let mut files = Vec::new();
        for stamp in &self.stamps {
            control.checkpoint().map_err(io::Error::other)?;
            if let Some(current) = stamp.capture_directory_for_resolution()? {
                directories.push(current);
            } else {
                stamp.verify_metadata()?;
                files.push(stamp);
            }
        }
        for (path, lookup) in &self.material_lookups {
            control.checkpoint().map_err(io::Error::other)?;
            lookup.verify(game, path)?;
        }
        for stamp in files {
            control.checkpoint().map_err(io::Error::other)?;
            stamp.verify_metadata()?;
        }
        for stamp in directories {
            control.checkpoint().map_err(io::Error::other)?;
            stamp.verify_metadata()?;
        }
        control.checkpoint().map_err(io::Error::other)
    }
}

#[derive(Debug)]
pub struct OutputAssetSourceGuard {
    reflections: Option<crate::cubemap_assets::ReflectionSourceGuard>,
    sky: Option<crate::sky_assets::SkyAssetSourceGuard>,
    water: Option<AssetSourceGuard>,
    prop_lighting: Option<AssetSourceGuard>,
    material_sources: Option<AssetSourceGuard>,
    static_prop_sources: Option<AssetSourceGuard>,
}

impl OutputAssetSourceGuard {
    pub fn prop_lighting_guard(&self) -> Option<AssetSourceGuard> {
        self.prop_lighting.clone()
    }

    pub fn static_prop_guard(&self) -> Option<AssetSourceGuard> {
        self.static_prop_sources.clone()
    }

    pub fn verify_metadata(&self) -> io::Result<()> {
        if let Some(reflections) = &self.reflections {
            reflections.verify_metadata()?;
        }
        if let Some(sky) = &self.sky {
            sky.verify_metadata()?;
        }
        if let Some(water) = &self.water {
            water.verify_metadata()?;
        }
        if let Some(prop_lighting) = &self.prop_lighting {
            prop_lighting.verify_metadata()?;
        }
        if let Some(material_sources) = &self.material_sources {
            material_sources.verify_metadata()?;
        }
        if let Some(static_prop_sources) = &self.static_prop_sources {
            static_prop_sources.verify_metadata()?;
        }
        Ok(())
    }
}

pub fn capture_output_asset_source_guard(
    output: &crate::Output,
    game: &GameFs,
    control: &Control,
) -> io::Result<OutputAssetSourceGuard> {
    control.checkpoint().map_err(io::Error::other)?;
    let reflections = output
        .cubemap_assets
        .as_ref()
        .map(|manifest| {
            crate::cubemap_assets::capture_reflection_source_guard(manifest, game, control)
        })
        .transpose()?;
    let sky = output
        .sky_assets
        .as_ref()
        .map(|plan| plan.capture_source_guard(game, control))
        .transpose()?;
    let prop_lighting = output
        .prop_lighting_assets
        .as_ref()
        .map(|plan| plan.capture_source_guard(game, control))
        .transpose()?;
    let water = output
        .water_assets
        .as_ref()
        .map(|plan| plan.capture_source_guard(game, control))
        .transpose()?;
    let material_sources = output.material_sources.clone();
    if let Some(material_sources) = &material_sources {
        material_sources.verify_sources(game, control)?;
    }
    let static_prop_sources = output.static_prop_source_guard()?.cloned();
    if let Some(static_prop_sources) = &static_prop_sources {
        static_prop_sources.verify_sources(game, control)?;
    }
    let guard = OutputAssetSourceGuard {
        reflections,
        sky,
        water,
        prop_lighting,
        material_sources,
        static_prop_sources,
    };
    control.checkpoint().map_err(io::Error::other)?;
    guard.verify_metadata()?;
    Ok(guard)
}

#[derive(Debug, Clone, Copy)]
pub struct SourceGuardPolicy {
    pub label: &'static str,
    pub missing_label: &'static str,
    pub asset_bytes: usize,
    pub physical_paths: usize,
}

pub fn capture_asset_source_guard<'a>(
    sources: impl IntoIterator<Item = (&'a str, &'a str)>,
    missing_sources: impl IntoIterator<Item = &'a str>,
    game: &GameFs,
    control: &Control,
    policy: SourceGuardPolicy,
) -> io::Result<AssetSourceGuard> {
    let mut stamps = BTreeMap::new();
    for (path, sha256) in sources {
        control.checkpoint().map_err(io::Error::other)?;
        let before = game
            .asset_source_paths(path)?
            .ok_or_else(|| invalid(format!("{} source disappeared: {path}", policy.label)))?;
        capture_paths(&before, &mut stamps, policy)?;
        let bytes = game
            .read_limited(path, policy.asset_bytes)?
            .ok_or_else(|| invalid(format!("{} source disappeared: {path}", policy.label)))?;
        if hash(&bytes) != sha256 {
            return Err(invalid(format!(
                "{} source changed before publication: {path}",
                policy.label
            )));
        }
        let after = game
            .asset_source_paths(path)?
            .ok_or_else(|| invalid(format!("{} source disappeared: {path}", policy.label)))?;
        if before != after {
            return Err(invalid(format!(
                "{} source resolution changed: {path}",
                policy.label
            )));
        }
        capture_paths(&after, &mut stamps, policy)?;
    }
    for path in missing_sources {
        control.checkpoint().map_err(io::Error::other)?;
        let before = game.asset_lookup_paths(path)?;
        capture_paths(&before, &mut stamps, policy)?;
        if game.contains(path)? {
            return Err(invalid(format!(
                "{} appeared before publication: {path}",
                policy.missing_label
            )));
        }
        let after = game.asset_lookup_paths(path)?;
        if before != after {
            return Err(invalid(format!(
                "{} source resolution changed: {path}",
                policy.label
            )));
        }
        capture_paths(&after, &mut stamps, policy)?;
    }
    let guard = AssetSourceGuard {
        stamps: stamps.into_values().collect(),
        material_lookups: BTreeMap::new(),
    };
    guard.verify_metadata()?;
    Ok(guard)
}

fn capture_paths(
    paths: &AssetSourcePaths,
    stamps: &mut BTreeMap<PathBuf, FileStamp>,
    policy: SourceGuardPolicy,
) -> io::Result<()> {
    for path in paths
        .files
        .iter()
        .chain(&paths.lookup_directories)
        .chain(&paths.lookup_symlinks)
    {
        let stamp = FileStamp::capture_container(path)?;
        if let Some(old) = stamps.get(path) {
            if old != &stamp {
                return Err(invalid(format!(
                    "{} backing file or lookup path changed: {}",
                    policy.label,
                    path.display()
                )));
            }
        } else {
            if stamps.len() >= policy.physical_paths {
                return Err(invalid(format!(
                    "{} source guard exceeds {} physical lookup paths",
                    policy.label, policy.physical_paths,
                )));
            }
            stamps.insert(path.clone(), stamp);
        }
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub(crate) fn hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests;
