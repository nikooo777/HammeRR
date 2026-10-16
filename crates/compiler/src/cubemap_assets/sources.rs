use std::io;

use source_formats::gamefs::GameFs;

use super::{CubemapAssetManifest, MAX_GENERATED_BYTES};
use crate::Control;
use crate::asset_sources::{AssetSourceGuard, SourceGuardPolicy, capture_asset_source_guard};

const MAX_PHYSICAL_PATHS: usize = 8192;

#[derive(Debug)]
pub struct ReflectionSourceGuard {
    guard: AssetSourceGuard,
}

impl ReflectionSourceGuard {
    pub fn verify_metadata(&self) -> io::Result<()> {
        self.guard.verify_metadata()
    }
}

pub fn capture_reflection_source_guard(
    manifest: &CubemapAssetManifest,
    game: &GameFs,
    control: &Control,
) -> io::Result<ReflectionSourceGuard> {
    let guard = capture_asset_source_guard(
        manifest
            .sources
            .iter()
            .map(|source| (source.path.as_str(), source.sha256.as_str())),
        manifest.missing_sources.iter().map(String::as_str),
        game,
        control,
        SourceGuardPolicy {
            label: "Reflection",
            missing_label: "Reflection model material candidate",
            asset_bytes: MAX_GENERATED_BYTES,
            physical_paths: MAX_PHYSICAL_PATHS,
        },
    )?;
    Ok(ReflectionSourceGuard { guard })
}
