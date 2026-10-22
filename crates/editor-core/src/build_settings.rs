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
    #[serde(
        default = "default_lighting_work_limit",
        skip_serializing_if = "is_default_lighting_work_limit"
    )]
    pub lighting_work_limit: u64,
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
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub brush_texture_shadows: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub refine_ambient_probes: bool,
}

fn default_lighting_work_limit() -> u64 {
    compiler::DEFAULT_LIGHTING_WORK_LIMIT
}

fn is_default_lighting_work_limit(value: &u64) -> bool {
    *value == compiler::DEFAULT_LIGHTING_WORK_LIMIT
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
            lighting_work_limit: options.lighting_work_limit,
            generate_reflections: options.generate_reflections,
            pack_sky_assets: options.pack_sky_assets,
            pack_water_assets: options.pack_water_assets,
            pack_static_prop_assets: options.pack_static_prop_assets,
            bake_static_prop_vertices: options.bake_static_prop_vertices,
            brush_texture_shadows: options.brush_texture_shadows,
            refine_ambient_probes: options.refine_ambient_probes,
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
            lighting_work_limit: self.lighting_work_limit,
            generate_reflections: self.generate_reflections,
            pack_sky_assets: self.pack_sky_assets,
            pack_water_assets: self.pack_water_assets,
            pack_static_prop_assets: self.pack_static_prop_assets,
            bake_static_prop_vertices: self.bake_static_prop_vertices,
            brush_texture_shadows: self.brush_texture_shadows,
            refine_ambient_probes: self.refine_ambient_probes,
        }
        .validate()
    }
}

impl BuildSettings {
    pub(crate) fn report(&self) -> serde_json::Value {
        let mut settings = serde_json::json!(self);
        settings["lighting_work_limit"] = self.lighting_work_limit.into();
        settings["brush_texture_shadows"] = self.brush_texture_shadows.into();
        settings["refine_ambient_probes"] = self.refine_ambient_probes.into();
        settings
    }

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
            lighting_work_limit: Some(self.lighting_work_limit),
            generate_reflections: Some(self.generate_reflections),
            pack_sky_assets: Some(self.pack_sky_assets),
            pack_water_assets: Some(self.pack_water_assets),
            pack_static_prop_assets: Some(self.pack_static_prop_assets),
            bake_static_prop_vertices: Some(self.bake_static_prop_vertices),
            brush_texture_shadows: Some(self.brush_texture_shadows),
            refine_ambient_probes: Some(self.refine_ambient_probes),
        }
    }
}
