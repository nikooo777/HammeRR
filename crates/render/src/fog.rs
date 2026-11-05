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
pub struct ViewFog {
    controller: Profile,
    range: [f32; 2],
    water: Option<WaterFog>,
}

impl ViewFog {
    pub fn controller(profile: Profile) -> Result<Self, String> {
        profile.validate()?;
        Ok(Self {
            controller: profile,
            range: profile.range_parameters()?,
            water: None,
        })
    }

    pub fn water(controller: Option<Profile>, water: WaterFog) -> Result<Self, String> {
        let controller = controller.ok_or(
            "Underwater fog needs an established controller radial context; native pass-history initialization is unavailable",
        )?;
        let mut fog = Self::controller(controller)?;
        fog.water = Some(water);
        Ok(fog)
    }

    pub fn enabled(self) -> bool {
        self.water
            .map_or(self.controller.enabled(), |water| water.enabled)
    }

    pub fn max_density(self) -> f32 {
        if self.water.is_some() {
            1.0
        } else {
            self.controller.max_density().clamp(0.0, 1.0)
        }
    }

    pub fn far_z(self) -> Option<f32> {
        let far = self.controller.far_z();
        (far > 0.0).then_some(far)
    }

    pub fn controller_profile(self) -> Profile {
        self.controller
    }
    pub fn water_settings(self) -> Option<WaterFog> {
        self.water
    }

    pub fn color_for_camera(self, camera: &Camera) -> [f32; 3] {
        let normalized = self.water.map_or_else(
            || {
                let primary = self.controller.color();
                let secondary = self.controller.color2();
                let weight = if self.controller.blend() {
                    let direction = glam::Vec3::from_array(self.controller.direction());
                    let squared = (direction.x * direction.x + direction.y * direction.y)
                        + (direction.z * direction.z + DIRECTION_NORMALIZATION_BIAS);
                    let direction = direction * squared.sqrt().recip();
                    0.5 * camera.forward().dot(direction) + 0.5
                } else {
                    1.0
                };
                std::array::from_fn(|index| {
                    (primary[index] * weight + secondary[index] * (1.0 - weight))
                        * BYTE_NORMALIZATION
                })
            },
            |water| water.normalized_rgb,
        );
        normalized.map(linear_fog_channel)
    }
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

pub(crate) fn uniforms(fog: ViewFog, camera: &Camera) -> ([f32; 4], [f32; 4]) {
    let color = fog.color_for_camera(camera);
    let range = fog.water.map_or(fog.range, |water| water.range);
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
            range[0],
            range[1],
            if fog.controller.radial() { 1.0 } else { 0.0 },
            0.0,
        ],
    )
}
