mod collection;
mod manifest;

pub use collection::{WaterAssetPlan, WaterAssetSourceSnapshot, collect_water_asset_plan};
pub use manifest::{
    PACKED_WATER_MANIFEST_PATH, WaterAssetManifest, WaterPackedAsset, read_packed_water,
    verify_packed_water,
};

const MAX_ROOTS: usize = 1024;
const MAX_MANIFEST_BYTES: usize = 2 * 1024 * 1024;

fn invalid(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
}

fn checkpoint(control: &crate::Control) -> std::io::Result<()> {
    control.checkpoint().map_err(std::io::Error::other)
}

#[cfg(test)]
mod build_tests;
#[cfg(test)]
mod tests;
