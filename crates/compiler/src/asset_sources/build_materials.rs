use std::sync::{Arc, Mutex, MutexGuard};

use source_formats::gamefs::{AssetOrigin, MountedAsset, asset_relative};

use super::*;

pub const MAX_BUILD_MATERIAL_RETAINED_BYTES: usize = 1024 * 1024 * 1024;
pub const MAX_BUILD_MATERIAL_LOGICAL_PATHS: usize = 16_384;
pub const MAX_BUILD_MATERIAL_PHYSICAL_PATHS: usize = 131_072;
pub const MAX_BUILD_MATERIAL_LOOKUP_ROWS: usize = 262_144;

#[derive(Clone)]
pub struct CapturedBuildAsset {
    source: Arc<BuildAsset>,
}

#[derive(Debug)]
struct BuildAsset {
    path: String,
    bytes: Vec<u8>,
    origin: AssetOrigin,
}

impl std::fmt::Debug for CapturedBuildAsset {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("CapturedBuildAsset")
            .field("path", &self.path())
            .field("bytes", &self.bytes().len())
            .field("origin", self.origin())
            .finish()
    }
}

impl CapturedBuildAsset {
    pub fn path(&self) -> &str {
        &self.source.path
    }

    pub fn bytes(&self) -> &[u8] {
        &self.source.bytes
    }

    pub fn origin(&self) -> &AssetOrigin {
        &self.source.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BuildMaterialLookup {
    present: bool,
    paths: AssetSourcePaths,
}

impl BuildMaterialLookup {
    fn capture(game: &GameFs, path: &str) -> io::Result<Self> {
        match game.asset_source_paths(path)? {
            Some(paths) => Ok(Self {
                present: true,
                paths,
            }),
            None => Ok(Self {
                present: false,
                paths: game.asset_lookup_paths(path)?,
            }),
        }
    }

    pub(super) fn verify(&self, game: &GameFs, path: &str) -> io::Result<()> {
        if Self::capture(game, path)? != *self {
            return Err(changed(path));
        }
        Ok(())
    }

    fn rows(&self) -> Option<usize> {
        self.paths
            .files
            .len()
            .checked_add(self.paths.lookup_directories.len())
            .and_then(|rows| rows.checked_add(self.paths.lookup_symlinks.len()))
    }
}

#[derive(Debug)]
struct Observation {
    lookup: BuildMaterialLookup,
    asset: Option<CapturedBuildAsset>,
}

#[derive(Debug, Clone, Copy)]
struct Limits {
    payload_bytes: usize,
    logical_paths: usize,
    physical_paths: usize,
    lookup_rows: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            payload_bytes: MAX_BUILD_MATERIAL_RETAINED_BYTES,
            logical_paths: MAX_BUILD_MATERIAL_LOGICAL_PATHS,
            physical_paths: MAX_BUILD_MATERIAL_PHYSICAL_PATHS,
            lookup_rows: MAX_BUILD_MATERIAL_LOOKUP_ROWS,
        }
    }
}

#[derive(Debug)]
struct ReadFailure {
    kind: io::ErrorKind,
    message: String,
}

impl ReadFailure {
    fn from_error(error: &io::Error) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
        }
    }

    fn error(&self) -> io::Error {
        io::Error::new(self.kind, self.message.clone())
    }
}

#[derive(Debug, Default)]
struct State {
    limits: Limits,
    observations: BTreeMap<String, Observation>,
    stamps: BTreeMap<PathBuf, FileStamp>,
    payload_bytes: usize,
    lookup_rows: usize,
    failure: Option<ReadFailure>,
}

#[derive(Debug, Clone, Default)]
pub struct BuildMaterialSources {
    state: Arc<Mutex<State>>,
}

impl BuildMaterialSources {
    pub fn read_asset(
        &self,
        game: &GameFs,
        path: &str,
        maximum: usize,
    ) -> io::Result<Option<CapturedBuildAsset>> {
        let path = asset_relative(path)?;
        let mut state = self.lock()?;
        let result = state.read_asset(game, &path, maximum);
        if let Err(error) = &result {
            state.failure = Some(ReadFailure::from_error(error));
        }
        result
    }

    pub fn retained(&self, path: &str) -> io::Result<Option<CapturedBuildAsset>> {
        let path = asset_relative(path)?;
        let state = self.lock()?;
        Ok(state
            .observations
            .get(&path)
            .and_then(|observation| observation.asset.clone()))
    }

    pub fn is_observed(&self, path: &str) -> io::Result<bool> {
        let path = asset_relative(path)?;
        Ok(self.lock()?.observations.contains_key(&path))
    }

    pub fn source_guard(&self, game: &GameFs, control: &Control) -> io::Result<AssetSourceGuard> {
        control.checkpoint().map_err(io::Error::other)?;
        let state = self.lock()?;
        let guard = AssetSourceGuard {
            stamps: state.stamps.values().cloned().collect(),
            material_lookups: state
                .observations
                .iter()
                .map(|(path, observation)| (path.clone(), observation.lookup.clone()))
                .collect(),
        };
        guard.verify_sources(game, control)?;
        Ok(guard)
    }

    fn lock(&self) -> io::Result<MutexGuard<'_, State>> {
        let state = self
            .state
            .lock()
            .map_err(|_| invalid("Build material source owner is unavailable"))?;
        if let Some(failure) = &state.failure {
            return Err(failure.error());
        }
        Ok(state)
    }
}

impl State {
    fn read_asset(
        &mut self,
        game: &GameFs,
        path: &str,
        maximum: usize,
    ) -> io::Result<Option<CapturedBuildAsset>> {
        let before = BuildMaterialLookup::capture(game, path)?;
        let original = self.observations.get(path);
        let first_read = original.is_none();
        if let Some(original) = original {
            if original.lookup != before {
                return Err(changed(path));
            }
        } else {
            if self.observations.len() >= self.limits.logical_paths {
                return Err(invalid(format!(
                    "Build material source ownership exceeds {} logical paths",
                    self.limits.logical_paths,
                )));
            }
            self.lookup_rows = before
                .rows()
                .and_then(|rows| self.lookup_rows.checked_add(rows))
                .filter(|rows| *rows <= self.limits.lookup_rows)
                .ok_or_else(|| {
                    invalid(format!(
                        "Build material source ownership exceeds {} retained lookup rows",
                        self.limits.lookup_rows,
                    ))
                })?;
            self.observations.insert(
                path.to_owned(),
                Observation {
                    lookup: before.clone(),
                    asset: None,
                },
            );
        }
        let policy = SourceGuardPolicy {
            label: "Build material",
            missing_label: "Build material candidate",
            asset_bytes: maximum,
            physical_paths: self.limits.physical_paths,
        };
        capture_paths(&before.paths, &mut self.stamps, policy)?;
        let remaining = self.limits.payload_bytes - self.payload_bytes;
        let limit = if first_read {
            maximum.min(remaining)
        } else {
            maximum
        };
        let asset = game.read_asset_limited(path, limit).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "Build material {path} (retained payload limit {} bytes; remaining {remaining} bytes; effective read limit {limit} bytes): {error}",
                    self.limits.payload_bytes,
                ),
            )
        })?;
        let after = BuildMaterialLookup::capture(game, path)?;
        if before != after || before.present != asset.is_some() {
            return Err(changed(path));
        }
        capture_paths(&after.paths, &mut self.stamps, policy)?;
        if !first_read {
            let original = &self.observations[path].asset;
            if !same_asset(original.as_ref(), asset.as_ref()) {
                return Err(changed(path));
            }
            return Ok(original.clone());
        }
        let asset = asset.map(|asset| CapturedBuildAsset {
            source: Arc::new(BuildAsset {
                path: path.to_owned(),
                bytes: asset.data,
                origin: asset.origin,
            }),
        });
        self.payload_bytes = self
            .payload_bytes
            .checked_add(asset.as_ref().map_or(0, |asset| asset.bytes().len()))
            .filter(|bytes| *bytes <= self.limits.payload_bytes)
            .ok_or_else(|| invalid("Build material retained payload accounting overflow"))?;
        self.observations.get_mut(path).unwrap().asset = asset.clone();
        Ok(asset)
    }
}

fn same_asset(original: Option<&CapturedBuildAsset>, current: Option<&MountedAsset>) -> bool {
    match (original, current) {
        (Some(original), Some(current)) => {
            original.bytes() == current.data.as_slice() && original.origin() == &current.origin
        }
        (None, None) => true,
        _ => false,
    }
}

fn changed(path: &str) -> io::Error {
    invalid(format!(
        "Build material original source or winning/missing lookup changed: {path}"
    ))
}

#[cfg(test)]
mod tests;
