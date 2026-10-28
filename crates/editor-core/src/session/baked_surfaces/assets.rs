use std::sync::Arc;

use compiler::{
    Control,
    asset_sources::{AssetSourceGuard, BuildMaterialSources},
};
use render::compiled_faces::{
    MaterialReference,
    materials::{self, Budget, Material},
};
use source_formats::{
    bsp::{Bsp, PakLimits},
    gamefs::GameFs,
};

use super::MAX_METADATA_BYTES;

pub(super) const MAX_ASSET_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_DECODED_BYTES: usize = 128 * 1024 * 1024;
pub(super) const MAX_ASSET_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_MATERIAL_BYTES: usize = 1024 * 1024;
const MAX_MATERIALS: usize = 1024;
const MAX_MOUNTS: usize = 128;
const MAX_PATH_BYTES: usize = 1024;

pub(super) struct Prepared {
    pub(super) materials: Vec<Option<Arc<Material>>>,
    pub(super) guard: AssetSourceGuard,
    pub(super) bytes: usize,
}

impl Prepared {
    pub(super) fn verify(&self, control: &Control) -> Result<(), String> {
        self.guard
            .verify_original_metadata(control, MAX_METADATA_BYTES)
            .map_err(|error| format!("Baked surface preview asset observations changed: {error}"))
    }
}

pub(super) fn prepare(
    game: &GameFs,
    bsp: &Bsp,
    references: &[MaterialReference],
    maximum_decoded_bytes: usize,
    control: &Control,
) -> Result<Prepared, String> {
    if game.mounts.len() > MAX_MOUNTS
        || references.iter().filter(|material| material.used).count() > MAX_MATERIALS
    {
        return Err(
            "Baked surfaces exceed the working budget of 128 mounts or 1024 used materials".into(),
        );
    }
    let limits = PakLimits {
        max_bsp_bytes: super::baked_build::MAX_BSP_BYTES as usize,
        ..PakLimits::default()
    };
    let pak = bsp
        .pak_data_controlled(limits, &mut || {
            control.checkpoint().map_err(std::io::Error::other)
        })
        .map_err(|error| error.to_string())?;
    let sources = BuildMaterialSources::bounded(MAX_ASSET_BYTES, 4096, 16_384, 32_768);
    let mut budget = Budget {
        max_asset_bytes: MAX_ASSET_FILE_BYTES,
        max_texture_axis: 4096,
        max_decoded_bytes: maximum_decoded_bytes.min(MAX_DECODED_BYTES),
        decoded_bytes: 0,
    };
    let mut prepared = Vec::new();
    prepared
        .try_reserve_exact(references.len())
        .map_err(|_| "Baked surface material allocation failed")?;
    for reference in references {
        control.checkpoint()?;
        if !reference.used {
            prepared.push(None);
            continue;
        }
        let material = materials::load(
            &reference.name,
            reference.directional,
            reference.lightmapped,
            &pak,
            &mut |path| {
                control.checkpoint().map_err(std::io::Error::other)?;
                if path.len() > MAX_PATH_BYTES {
                    return Err(std::io::Error::other(
                        "Baked surface input path exceeds 1024 bytes",
                    ));
                }
                let maximum = if path.ends_with(".vmt") {
                    MAX_MATERIAL_BYTES
                } else {
                    MAX_ASSET_FILE_BYTES
                };
                let asset = sources.read_asset(game, path, maximum)?;
                Ok(asset.map(|asset| asset.bytes().to_vec()))
            },
            &mut || control.checkpoint(),
            &mut budget,
        )?;
        prepared.push(Some(Arc::new(material)));
    }
    let guard = sources
        .original_source_guard(control, MAX_METADATA_BYTES)
        .map_err(|error| error.to_string())?;
    guard
        .verify_original_metadata(control, MAX_METADATA_BYTES)
        .map_err(|error| error.to_string())?;
    control.checkpoint()?;
    Ok(Prepared {
        materials: prepared,
        guard,
        bytes: budget.decoded_bytes,
    })
}
