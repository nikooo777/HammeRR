use super::Session;
use render::ViewOptions;

#[derive(Default)]
pub struct ViewUpdate<'a> {
    pub instance_contents: Option<bool>,
    pub lighting_preview: Option<bool>,
    pub lighting_channel: Option<&'a str>,
    pub lighting_sky_ambient: Option<bool>,
    pub lighting_quality: Option<&'a str>,
    pub lighting_exposure_ev: Option<f64>,
    pub normal_maps: Option<bool>,
    pub surface_shading: Option<bool>,
    pub surface: Option<&'a str>,
    pub smoothing_group: Option<u32>,
    pub tool_textures: Option<bool>,
    pub leak_path: Option<bool>,
    pub sky_preview: Option<bool>,
    pub sky3d_preview: Option<bool>,
    pub rope_preview: Option<bool>,
    pub selection_outlines: Option<bool>,
    pub selection_tint: Option<bool>,
    pub entity_images: Option<bool>,
    pub additive_sprite_guides: Option<bool>,
    pub entity_model_guides: Option<bool>,
    pub authored_models: Option<bool>,
    pub sprite_resources: Option<bool>,
    pub entity_helpers: Option<bool>,
    pub selection_bounds: Option<bool>,
    pub sky_channel: Option<&'a str>,
    pub sky_exposure_ev: Option<f64>,
}

impl Session {
    pub fn render_scene_key(&self, view: ViewOptions) -> render::SceneKey {
        let mut key = view.scene_key(self.scene_serial());
        key.instance_contents_serial = self.instance_contents_serial();
        key.reference_images_serial = self.reference_images_serial();
        key
    }

    pub fn normal_map_preview_state(&mut self) -> serde_json::Value {
        let report = self.build_scene_for_view(self.view()).normal_maps;
        serde_json::json!({"enabled":self.view().normal_maps,"summary":report.summary(),"report":report,"limits":render::normal_maps::limits()})
    }

    pub fn resolve_view_update(&self, update: ViewUpdate<'_>) -> Result<ViewOptions, String> {
        let mut view = self.surface_view_options(
            update.surface.map(render::SurfaceView::parse).transpose()?,
            update.smoothing_group,
        )?;
        if let Some(value) = update.instance_contents {
            view.instance_contents = value;
        }
        if let Some(value) = update.normal_maps {
            view.normal_maps = value;
        }
        if let Some(value) = update.surface_shading {
            view.surface_shading = value;
        }
        if let Some(value) = update.tool_textures {
            view.tool_textures = value;
        }
        if let Some(value) = update.leak_path {
            view.leak_path = value;
        }
        if let Some(value) = update.sky_preview {
            view.sky_preview = value;
        }
        if let Some(value) = update.sky3d_preview {
            view.sky3d_preview = value;
        }
        if let Some(value) = update.rope_preview {
            view.rope_preview = value;
        }
        if let Some(value) = update.selection_tint {
            view.selection_tint = value;
        }
        if let Some(value) = update.selection_outlines {
            view.selection_outlines = value;
        }
        if let Some(value) = update.entity_images {
            view.entity_images = value;
            if update.sprite_resources.is_none() {
                view.sprite_resources = value;
            }
        }
        if let Some(value) = update.authored_models {
            view.authored_models = value;
        }
        if let Some(value) = update.sprite_resources {
            view.sprite_resources = value;
        }
        if let Some(value) = update.entity_helpers {
            view.entity_helpers = value;
        }
        if let Some(value) = update.selection_bounds {
            view.selection_bounds = value;
        }
        if let Some(value) = update.additive_sprite_guides {
            view.additive_sprite_guides = value;
        }
        if let Some(value) = update.entity_model_guides {
            view.entity_model_guides = value;
        }
        if let Some(value) = update.lighting_preview {
            view.lighting_preview = value;
        }
        if let Some(value) = update.lighting_channel {
            view.lighting_channel = render::LightingChannel::parse(value)?;
        }
        if let Some(value) = update.lighting_sky_ambient {
            view.lighting_sky_ambient = value;
        }
        if let Some(value) = update.lighting_quality {
            view.lighting_quality = render::lighting::Quality::parse(value)?;
        }
        if let Some(value) = update.lighting_exposure_ev {
            if !value.is_finite() || !(-8.0..=8.0).contains(&value) {
                return Err("Lighting exposure must be finite and between -8 and 8 EV".into());
            }
            view.lighting_exposure_ev = value as f32;
        }
        if view.lighting_preview
            && view.normal_maps
            && view.lighting_quality == render::lighting::Quality::Fast
        {
            return Err("Fast authored lighting does not support normal maps; choose Detail or disable Normal map preview".into());
        }
        self.sky_preview_options(view, update.sky_channel, update.sky_exposure_ev)
    }
}
