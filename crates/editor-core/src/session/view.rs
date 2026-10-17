use super::Session;
use render::ViewOptions;

#[derive(Default)]
pub struct ViewUpdate<'a> {
    pub normal_maps: Option<bool>,
    pub surface: Option<&'a str>,
    pub smoothing_group: Option<u32>,
    pub tool_textures: Option<bool>,
    pub leak_path: Option<bool>,
    pub sky_preview: Option<bool>,
    pub rope_preview: Option<bool>,
    pub selection_outlines: Option<bool>,
    pub entity_images: Option<bool>,
    pub entity_model_guides: Option<bool>,
    pub sky_channel: Option<&'a str>,
    pub sky_exposure_ev: Option<f64>,
}

impl Session {
    pub fn normal_map_preview_state(&mut self) -> serde_json::Value {
        let report = self.build_scene_for_view(self.view()).normal_maps;
        serde_json::json!({"enabled":self.view().normal_maps,"summary":report.summary(),"report":report,"limits":render::normal_maps::limits()})
    }

    pub fn resolve_view_update(&self, update: ViewUpdate<'_>) -> Result<ViewOptions, String> {
        let mut view = self.surface_view_options(
            update.surface.map(render::SurfaceView::parse).transpose()?,
            update.smoothing_group,
        )?;
        if let Some(value) = update.normal_maps {
            view.normal_maps = value;
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
        if let Some(value) = update.rope_preview {
            view.rope_preview = value;
        }
        if let Some(value) = update.selection_outlines {
            view.selection_outlines = value;
        }
        if let Some(value) = update.entity_images {
            view.entity_images = value;
        }
        if let Some(value) = update.entity_model_guides {
            view.entity_model_guides = value;
        }
        self.sky_preview_options(view, update.sky_channel, update.sky_exposure_ev)
    }
}
