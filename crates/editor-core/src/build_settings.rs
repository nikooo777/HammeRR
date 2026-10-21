use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSettings {
    pub lighting: String,
    pub physics: String,
    pub sky_quality: String,
    pub visibility: String,
    pub bounces: u8,
    pub smoothing_angle: u8,
    #[serde(default)]
    pub generate_reflections: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pack_sky_assets: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pack_water_assets: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pack_static_prop_assets: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bake_static_prop_vertices: bool,
}

impl From<compiler::Options> for BuildSettings {
    fn from(options: compiler::Options) -> Self {
        Self::from_options(options)
    }
}

impl BuildSettings {
    pub fn from_options(options: compiler::Options) -> Self {
        Self {
            lighting: options.lighting.name().into(),
            physics: options.physics.name().into(),
            sky_quality: options.sky_quality.name().into(),
            visibility: options.visibility.name().into(),
            bounces: options.bounces,
            smoothing_angle: options.smoothing_angle,
            generate_reflections: options.generate_reflections,
            pack_sky_assets: options.pack_sky_assets,
            pack_water_assets: options.pack_water_assets,
            pack_static_prop_assets: options.pack_static_prop_assets,
            bake_static_prop_vertices: options.bake_static_prop_vertices,
        }
    }

    pub fn options(&self) -> Result<compiler::Options, String> {
        compiler::Options {
            lighting: compiler::LightingMode::parse(&self.lighting)?,
            physics: compiler::PhysicsMode::parse(&self.physics)?,
            sky_quality: compiler::SkyQuality::parse(&self.sky_quality)?,
            visibility: compiler::VisibilityMode::parse(&self.visibility)?,
            bounces: self.bounces,
            smoothing_angle: self.smoothing_angle,
            generate_reflections: self.generate_reflections,
            pack_sky_assets: self.pack_sky_assets,
            pack_water_assets: self.pack_water_assets,
            pack_static_prop_assets: self.pack_static_prop_assets,
            bake_static_prop_vertices: self.bake_static_prop_vertices,
        }
        .validate()
    }
}

impl BuildSettings {
    pub fn validate(&self) -> Result<(), String> {
        self.options().map(|_| ())
    }

    pub(crate) fn update(&self) -> crate::session::compilation::BuildSettingsUpdate<'_> {
        crate::session::compilation::BuildSettingsUpdate {
            lighting: Some(&self.lighting),
            physics: Some(&self.physics),
            sky_quality: Some(&self.sky_quality),
            visibility: Some(&self.visibility),
            bounces: Some(self.bounces),
            smoothing_angle: Some(self.smoothing_angle),
            generate_reflections: Some(self.generate_reflections),
            pack_sky_assets: Some(self.pack_sky_assets),
            pack_water_assets: Some(self.pack_water_assets),
            pack_static_prop_assets: Some(self.pack_static_prop_assets),
            bake_static_prop_vertices: Some(self.bake_static_prop_vertices),
        }
    }
}
