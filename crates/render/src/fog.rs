pub use map_core::fog::{Fog, Profile};

use crate::{Camera, Projection, SurfaceView};

const BYTE_NORMALIZATION: f32 = 1.0 / 255.0;
const DIRECTION_NORMALIZATION_BIAS: f32 = 1.0e-10;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterFog {
    normalized_rgb: [f32; 3],
    range: [f32; 2],
    start: f32,
    end: f32,
    enabled: bool,
}

impl WaterFog {
    pub fn new(
        normalized_rgb: [f32; 3],
        start: f32,
        end: f32,
        enabled: bool,
    ) -> Result<Self, String> {
        if normalized_rgb.into_iter().any(|value| {
            !value.is_finite() || (value * 255.0).abs() > map_core::fog::MAX_FIELD_MAGNITUDE
        }) {
            return Err("Water fog RGB cannot produce admitted finite byte-color inputs".into());
        }
        Ok(Self {
            normalized_rgb,
            range: map_core::fog::range_parameters(start, end)?,
            start,
            end,
            enabled,
        })
    }

    pub fn normalized_rgb(self) -> [f32; 3] {
        self.normalized_rgb
    }
    pub fn start(self) -> f32 {
        self.start
    }
    pub fn end(self) -> f32 {
        self.end
    }
    pub fn enabled(self) -> bool {
        self.enabled
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum FogSource {
    Controller(Profile),
    Water {
        controller: Option<Profile>,
        water: WaterFog,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewFog {
    source: FogSource,
    range: [f32; 2],
    radial: bool,
}

impl ViewFog {
    pub fn controller(profile: Profile) -> Result<Self, String> {
        profile.validate()?;
        Ok(Self {
            source: FogSource::Controller(profile),
            range: profile.range_parameters()?,
            radial: profile.radial(),
        })
    }

    pub fn water(controller: Option<Profile>, water: WaterFog) -> Result<Self, String> {
        let controller = controller.ok_or(
            "Underwater fog needs an established controller radial context; native pass-history initialization is unavailable",
        )?;
        Self::water_with_context(Some(controller), water, controller.radial())
    }

    pub fn water_with_context(
        controller: Option<Profile>,
        water: WaterFog,
        radial: bool,
    ) -> Result<Self, String> {
        if let Some(controller) = controller {
            controller.validate()?;
        }
        Ok(Self {
            source: FogSource::Water { controller, water },
            range: water.range,
            radial,
        })
    }

    pub fn enabled(self) -> bool {
        match self.source {
            FogSource::Controller(controller) => controller.enabled(),
            FogSource::Water { water, .. } => water.enabled,
        }
    }

    pub fn max_density(self) -> f32 {
        match self.source {
            FogSource::Controller(controller) => controller.max_density().clamp(0.0, 1.0),
            FogSource::Water { .. } => 1.0,
        }
    }

    pub fn far_z(self) -> Option<f32> {
        self.controller_profile()
            .map(|controller| controller.far_z())
            .filter(|far| *far > 0.0)
    }

    pub fn controller_profile(self) -> Option<Profile> {
        match self.source {
            FogSource::Controller(controller) => Some(controller),
            FogSource::Water { controller, .. } => controller,
        }
    }

    pub fn water_settings(self) -> Option<WaterFog> {
        match self.source {
            FogSource::Controller(_) => None,
            FogSource::Water { water, .. } => Some(water),
        }
    }

    pub fn radial(self) -> bool {
        self.radial
    }

    pub fn color_for_camera(self, camera: &Camera) -> [f32; 3] {
        let normalized = match self.source {
            FogSource::Controller(controller) => controller_color(controller, camera),
            FogSource::Water { water, .. } => water.normalized_rgb,
        };
        normalized.map(linear_fog_channel)
    }
}

fn controller_color(controller: Profile, camera: &Camera) -> [f32; 3] {
    let primary = controller.color();
    let secondary = controller.color2();
    let weight = if controller.blend() {
        let direction = glam::Vec3::from_array(controller.received_direction());
        let squared = (direction.x * direction.x + direction.y * direction.y)
            + (direction.z * direction.z + DIRECTION_NORMALIZATION_BIAS);
        let direction = direction * squared.sqrt().recip();
        0.5 * camera.forward().dot(direction) + 0.5
    } else {
        1.0
    };
    std::array::from_fn(|index| {
        (primary[index] * weight + secondary[index] * (1.0 - weight)) * BYTE_NORMALIZATION
    })
}

fn linear_fog_channel(normalized: f32) -> f32 {
    let byte = (normalized * 255.0).trunc().clamp(0.0, 255.0);
    let value = byte * BYTE_NORMALIZATION;
    if value <= 0.04045 {
        value * (1.0 / 12.92)
    } else {
        ((value + 0.055) * (1.0 / 1.055)).powf(2.4)
    }
}

pub fn for_color(fog: Option<ViewFog>, camera: &Camera, surface: SurfaceView) -> Option<ViewFog> {
    fog.filter(|_| surface == SurfaceView::Textured && camera.projection == Projection::Perspective)
}

pub fn far_z(fog: Option<ViewFog>, camera: &Camera) -> Option<f32> {
    fog.filter(|_| camera.projection == Projection::Perspective)
        .and_then(ViewFog::far_z)
}

pub fn sky_view(
    profile: Profile,
    scale: f32,
    context: map_core::fog::InitialRadialContext,
) -> Result<ViewFog, String> {
    if profile.far_z != 0.0 || profile.lerp_time != 0.0 {
        return Err("Static sky fog does not accept controller far-Z or transition state".into());
    }
    ViewFog::controller(context.initial_profile(profile).scaled_range(scale)?)
}

pub(crate) fn uniforms(fog: ViewFog, camera: &Camera) -> ([f32; 4], [f32; 4]) {
    let color = fog.color_for_camera(camera);
    (
        [
            color[0],
            color[1],
            color[2],
            if fog.enabled() {
                fog.max_density()
            } else {
                0.0
            },
        ],
        [
            fog.range[0],
            fog.range[1],
            if fog.radial() { 1.0 } else { 0.0 },
            0.0,
        ],
    )
}
