use glam::Vec3;
use render::Camera;

use super::Session;

pub(super) const ORIGIN_HALF_EXTENT: f32 = 16.0;

pub(super) fn validate_aspect(aspect: f32) -> Result<(), String> {
    if !aspect.is_finite() || !(0.01..=100.0).contains(&aspect) {
        return Err("Viewport aspect must be finite and between 0.01 and 100".into());
    }
    Ok(())
}

impl Session {
    pub(super) fn inspection_viewport_ready(&self, action: &str) -> Result<(), String> {
        if self.viewport_pick_conflict()
            || self.measurement_picking()
            || self.linked_path_picking()
            || self.block_preview.is_some()
            || self.clip_drag.is_some()
        {
            return Err(format!(
                "Finish or cancel the active viewport gesture or block preview before {action}"
            ));
        }
        Ok(())
    }

    pub(super) fn inspection_frame_camera(
        &self,
        mins: Vec3,
        maxs: Vec3,
        aspect: f32,
        subject: &str,
    ) -> Result<Camera, String> {
        let camera = self.camera.frame_bounds(mins, maxs, aspect);
        if !camera.position.is_finite()
            || !camera.view_projection(aspect).is_finite()
            || !camera.yaw_degrees.is_finite()
            || !camera.pitch_degrees.is_finite()
            || !camera.fov_y_degrees.is_finite()
            || !camera.ortho_height.is_finite()
        {
            return Err(format!(
                "{subject} cannot be framed with the current camera"
            ));
        }
        Ok(camera)
    }
}
