use crate::{Camera, Projection, SurfaceView};

mod bounds;
pub use bounds::SourceRadianceBounds;

pub const MIN_EXPOSURE_EV: f32 = -8.0;
pub const MAX_EXPOSURE_EV: f32 = 8.0;
pub const MAX_HDR_RADIANCE: f64 = 65_504.0;
pub const HDR_DISPLAY_CURVE: &str = "reinhard_per_channel";
pub const HDR_DISPLAY_POLICY: &str = "Manual editor HDR scene: unexposed linear RGB, linear fog and translucent composition, one scene exposure 2^EV and per-channel Reinhard x/(1+x), then one sRGB encoding; editor overlays follow display conversion. No native adaptation or framebuffer parity.";

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SceneDisplay {
    #[default]
    Legacy,
    Hdr {
        exposure_ev: f32,
    },
}

impl SceneDisplay {
    pub fn validate(self) -> Result<(), String> {
        if let Self::Hdr { exposure_ev } = self
            && (!exposure_ev.is_finite()
                || !(MIN_EXPOSURE_EV..=MAX_EXPOSURE_EV).contains(&exposure_ev))
        {
            return Err(format!(
                "Scene exposure EV must be finite and within {MIN_EXPOSURE_EV}..{MAX_EXPOSURE_EV}"
            ));
        }
        Ok(())
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Hdr { .. } => "hdr",
        }
    }

    pub fn exposure_ev(self) -> Option<f32> {
        match self {
            Self::Legacy => None,
            Self::Hdr { exposure_ev } => Some(exposure_ev),
        }
    }

    pub fn active(self, camera: &Camera, surface: SurfaceView) -> bool {
        matches!(self, Self::Hdr { .. })
            && camera.projection == Projection::Perspective
            && surface == SurfaceView::Textured
    }

    pub fn resolve(
        self,
        camera: &Camera,
        surface: SurfaceView,
    ) -> Result<ResolvedSceneDisplay, String> {
        self.validate()?;
        let active = self.active(camera, surface);
        Ok(ResolvedSceneDisplay {
            requested: self,
            active,
            exposure_ev: if active { self.exposure_ev() } else { None },
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct ResolvedSceneDisplay {
    pub requested: SceneDisplay,
    pub active: bool,
    pub exposure_ev: Option<f32>,
}

impl ResolvedSceneDisplay {
    pub fn effective_exposure_ev(self, legacy: f32) -> f32 {
        self.exposure_ev.unwrap_or(legacy)
    }

    pub fn exposure_owner(self) -> &'static str {
        if self.active {
            "scene"
        } else {
            "legacy_component"
        }
    }

    pub fn display_curve(self) -> &'static str {
        if self.active {
            HDR_DISPLAY_CURVE
        } else {
            "legacy_component"
        }
    }
}
