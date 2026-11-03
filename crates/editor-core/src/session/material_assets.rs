use serde::Serialize;

use super::{Actor, Session};

#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub material: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub generation: u64,
    pub reloaded: usize,
    pub models_refreshed: usize,
    pub models_changed: usize,
    pub failed: usize,
    pub failures: Vec<Failure>,
    pub summary: String,
}

impl Session {
    pub(super) fn reset_mounted_content_assets(&mut self) {
        self.invalidate_content_builds();
        self.advance_content_epoch();
        self.models.get_mut().clear();
        self.materials = render::MaterialLibrary::default();
        self.material_index = None;
        self.material_shaders = super::material_catalog::ShaderIndex::default();
        self.material_reload = None;
        self.material_search.report = None;
        self.material_recent.clear();
        self.model_browser.catalog = None;
        self.surface_sample = None;
        self.reload_sound_assets();
        self.reload_particle_assets();
        self.reset_instance_contents();
        self.reset_compiled_details();
        self.reset_baked_surfaces();
        self.reset_sky3d_preview();
        self.native_capture.invalidate();
        self.compiled = None;
        self.compiled_visibility = super::compiled_visibility::State::default();
        self.compiled_prop_leaf_bounds = super::baked_prop_vertices::leaf_bounds::State::default();
        self.compilation.retained_pointfile = None;
        self.leak = None;
        self.scene_serial += 1;
        self.view_serial += 1;
    }

    pub fn material_reload_report(&self) -> Option<&Report> {
        self.material_reload.as_ref()
    }

    pub fn reload_material_assets(&mut self, actor: Actor) -> Report {
        self.cancel_geometry_drags();
        self.cancel_selection_previews();
        self.advance_content_epoch();
        let models = self.models.borrow_mut().reload_materials(&self.game);
        self.invalidate_instance_model_preview();
        let mut result = self
            .materials
            .reload_with_names(&self.game, models.materials);
        for (path, error) in models.failures {
            result.record_failure(path, error);
        }
        self.material_index = None;
        match self.search_materials("", usize::MAX) {
            Ok(names) => self.material_index = Some(names),
            Err(error) => result.record_failure("Material catalog".into(), error),
        }
        let previous_scene = self.scene_serial;
        self.scene_serial += 1;
        self.model_browser
            .materials_reloaded(previous_scene, self.scene_serial);
        self.model_attachments
            .materials_reloaded(previous_scene, self.scene_serial);
        self.particles
            .materials_reloaded(previous_scene, self.scene_serial);
        self.sounds
            .materials_reloaded(previous_scene, self.scene_serial);
        self.view_serial += 1;
        self.preflight_sky3d_preview();
        let summary = format!(
            "Reloaded {} cached materials · {} model material sets refreshed · {} unavailable previews",
            result.reloaded, models.refreshed, result.failed
        );
        let report = Report {
            generation: self.materials.generation(),
            reloaded: result.reloaded,
            models_refreshed: models.refreshed,
            models_changed: models.changed,
            failed: result.failed,
            failures: result
                .failures
                .into_iter()
                .map(|(material, error)| Failure { material, error })
                .collect(),
            summary,
        };
        self.log(actor, report.summary.clone());
        self.material_reload = Some(report.clone());
        report
    }
}
