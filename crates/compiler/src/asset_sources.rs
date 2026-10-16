use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use source_formats::gamefs::{AssetSourcePaths, GameFs};

use crate::{Control, FileStamp};

#[derive(Debug)]
pub struct AssetSourceGuard {
    stamps: Vec<FileStamp>,
}

impl AssetSourceGuard {
    pub fn verify_metadata(&self) -> io::Result<()> {
        for stamp in &self.stamps {
            stamp.verify_metadata()?;
        }
        Ok(())
    }
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
