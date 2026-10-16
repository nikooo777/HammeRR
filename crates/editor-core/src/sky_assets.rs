pub use compiler::sky_assets::{
    SkyAssetSource, SkyAssetsReport, SkyChannelReport, SkyFaceReport, SkyInspectionLimits,
    SkyMaterialSource, SkyTextureReport,
};

use crate::session::Session;

impl Session {
    pub fn inspect_sky_assets(&self) -> SkyAssetsReport {
        compiler::sky_assets::inspect_sky_assets(
            self.document().vmf(),
            self.game(),
            self.game_error(),
        )
    }
}

#[cfg(test)]
mod tests;
