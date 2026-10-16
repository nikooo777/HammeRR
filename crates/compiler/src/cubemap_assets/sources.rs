use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use source_formats::gamefs::{AssetSourcePaths, GameFs};

use super::manifest::invalid;
use super::*;
use crate::{Control, FileStamp};

#[derive(Debug)]
pub struct ReflectionSourceGuard {
    stamps: Vec<FileStamp>,
}

impl ReflectionSourceGuard {
    pub fn verify_metadata(&self) -> io::Result<()> {
        for stamp in &self.stamps {
            stamp.verify_metadata()?;
        }
        Ok(())
    }
}

pub fn capture_reflection_source_guard(
    manifest: &CubemapAssetManifest,
    game: &GameFs,
    control: &Control,
) -> io::Result<ReflectionSourceGuard> {
    let mut stamps = BTreeMap::new();
    for source in &manifest.sources {
        control.checkpoint().map_err(io::Error::other)?;
        let before = game
            .asset_source_paths(&source.path)?
            .ok_or_else(|| invalid(format!("Reflection source disappeared: {}", source.path)))?;
        capture_paths(&before, &mut stamps)?;
        let bytes = game
            .read_limited(&source.path, MAX_GENERATED_BYTES)?
            .ok_or_else(|| invalid(format!("Reflection source disappeared: {}", source.path)))?;
        if hash(&bytes) != source.sha256 {
            return Err(invalid(format!(
                "Reflection source changed before publication: {}",
                source.path
            )));
        }
        let after = game
            .asset_source_paths(&source.path)?
            .ok_or_else(|| invalid(format!("Reflection source disappeared: {}", source.path)))?;
        if before != after {
            return Err(invalid(format!(
                "Reflection source resolution changed: {}",
                source.path
            )));
        }
        capture_paths(&after, &mut stamps)?;
    }
    for path in &manifest.missing_sources {
        control.checkpoint().map_err(io::Error::other)?;
        let before = game.asset_lookup_paths(path)?;
        capture_paths(&before, &mut stamps)?;
        if game.contains(path)? {
            return Err(invalid(format!(
                "Reflection model material candidate appeared before publication: {path}"
            )));
        }
        let after = game.asset_lookup_paths(path)?;
        if before != after {
            return Err(invalid(format!(
                "Reflection source resolution changed: {path}"
            )));
        }
        capture_paths(&after, &mut stamps)?;
    }
    let guard = ReflectionSourceGuard {
        stamps: stamps.into_values().collect(),
    };
    guard.verify_metadata()?;
    Ok(guard)
}

fn capture_paths(
    paths: &AssetSourcePaths,
    stamps: &mut BTreeMap<PathBuf, FileStamp>,
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
                    "Reflection backing file or lookup path changed: {}",
                    path.display()
                )));
            }
        } else {
            if stamps.len() >= 8192 {
                return Err(invalid(
                    "Reflection source guard exceeds 8192 physical lookup paths",
                ));
            }
            stamps.insert(path.clone(), stamp);
        }
    }
    Ok(())
}
