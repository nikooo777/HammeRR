mod closure;
mod collection;
mod manifest;
mod material;

pub use collection::{
    StaticPropAssetPlan, StaticPropAssetSourceSnapshot, collect_static_prop_asset_plan,
};
pub use manifest::{
    PACKED_STATIC_PROPS_MANIFEST_PATH, StaticPropAssetManifest, StaticPropPackedAsset,
    StaticPropPackedUse, read_packed_static_props, verify_packed_static_props,
};

const MAX_MANIFEST_BYTES: usize = 2 * 1024 * 1024;
const MAX_LOOKUPS: usize = 16_384;
const MAX_MATERIAL_BYTES: usize = 1024 * 1024;
const MAX_MATERIAL_TOTAL_BYTES: usize = 4 * MAX_MATERIAL_BYTES;
const MAX_TEXTURE_AXIS: u32 = 4096;

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

#[cfg(test)]
mod patch_tests;

#[cfg(test)]
mod patch_edit_tests;

#[cfg(test)]
mod reflection_tests;
