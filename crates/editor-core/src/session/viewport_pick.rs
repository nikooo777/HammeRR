use map_core::query::Ray;
use render::Camera;

use super::{Pointer, Session};

pub(super) const MAX_DIMENSION: u32 = 8192;

pub(super) struct Context {
    pub(super) size: (u32, u32),
    camera: Camera,
    session_camera: Camera,
    camera_serial: u64,
    scene: u64,
    view: u64,
}

impl Context {
    pub(super) fn camera(&self) -> Camera {
        self.camera
    }

    pub(super) fn refresh_scene(&mut self, session: &Session) {
        self.scene = session.scene_serial;
        self.view = session.view_serial;
    }

    pub(super) fn capture(
        session: &Session,
        camera: Camera,
        size: (u32, u32),
        label: &str,
    ) -> Result<Self, String> {
        if size.0 == 0 || size.1 == 0 || size.0 > MAX_DIMENSION || size.1 > MAX_DIMENSION {
            return Err(format!(
                "{label} viewport dimensions must be between 1 and {MAX_DIMENSION}"
            ));
        }
        let (origin, direction) = camera.ray(0.0, 0.0, size.0 as f32 / size.1 as f32);
        if !origin.is_finite() || !direction.is_finite() || direction.length_squared() == 0.0 {
            return Err(format!("{label} requires a finite camera"));
        }
        Ok(Self {
            size,
            camera,
            session_camera: session.camera,
            camera_serial: session.camera_serial,
            scene: session.scene_serial,
            view: session.view_serial,
        })
    }

    pub(super) fn is_current(&self, session: &Session) -> bool {
        self.scene == session.scene_serial
            && self.view == session.view_serial
            && self.camera_serial == session.camera_serial
            && self.session_camera == session.camera
    }

    pub(super) fn ray(&self, pointer: Pointer) -> Result<Ray, String> {
        let pointer = pointer.validate()?;
        if pointer.size != self.size {
            return Err("Viewport dimensions changed; start position picking again".into());
        }
        if pointer.position.x < 0.0
            || pointer.position.y < 0.0
            || pointer.position.x >= f64::from(pointer.size.0)
            || pointer.position.y >= f64::from(pointer.size.1)
        {
            return Err("Position pixel must lie inside the captured viewport".into());
        }
        let (origin, direction) = pointer.ray(self.camera);
        Ok(Ray { origin, direction })
    }
}

impl Session {
    pub(super) fn viewport_pick_conflict(&self) -> bool {
        self.transform_pivot_picking()
            || self.overlay_corner_handles_active()
            || self.entity_surface_picking()
            || self.surface_position_picking()
            || self.clip_tool
            || self.spatial.draw_block
            || self.component_picking().is_some()
            || self.component_drag().is_some()
            || self.paint_stroke().is_some()
            || self.transform_drag().is_some()
            || self.area_preview().is_some()
            || self.volume_preview().is_some()
            || self.terrain_move_active()
            || self.terrain_stroke_active()
    }
}
