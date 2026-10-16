use map_core::query::Ray;
use render::Camera;
use serde_json::Value;

use super::*;
use crate::session::Pointer;

pub(super) struct Picking {
    pub(super) row: usize,
    pub(super) size: (u32, u32),
    camera: Camera,
    session_camera: Camera,
    camera_serial: u64,
    scene: u64,
    view: u64,
}

impl Session {
    pub fn linked_path_pick_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}:{:?}",
            self.linked_path_stamp(),
            self.scene_serial,
            self.view_serial,
            self.camera_serial,
            self.camera
        )
    }

    pub(super) fn checked_linked_path_pick(
        &self,
        actor: Actor,
        token: u64,
    ) -> Result<&Picking, String> {
        let preview = self.checked_linked_path(token)?;
        if preview.owner != actor {
            return Err("Review the other actor's linked path before picking a position".into());
        }
        let pick = preview
            .picking
            .as_ref()
            .ok_or("Choose Pick position first")?;
        if pick.scene != self.scene_serial
            || pick.view != self.view_serial
            || pick.camera_serial != self.camera_serial
            || pick.session_camera != self.camera
        {
            return Err("Scene, view or camera changed; choose Pick position again".into());
        }
        Ok(pick)
    }

    pub fn begin_linked_path_pick(
        &mut self,
        actor: Actor,
        token: u64,
        row: usize,
        camera: Camera,
        size: (u32, u32),
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.linked_path_pick_stamp() {
            return Err("Position picking context changed; inspect the linked path again".into());
        }
        let preview = self.checked_linked_path(token)?;
        if preview.owner != actor {
            return Err("Review the other actor's linked path before picking a position".into());
        }
        if row >= preview.request.points.len() {
            return Err("Choose an existing zero-based path row".into());
        }
        if size.0 == 0 || size.1 == 0 || size.0 > 8192 || size.1 > 8192 {
            return Err("Position picking viewport dimensions must be between 1 and 8192".into());
        }
        let (origin, direction) = camera.ray(0.0, 0.0, size.0 as f32 / size.1 as f32);
        if !origin.is_finite() || !direction.is_finite() || direction.length_squared() == 0.0 {
            return Err("Position picking requires a finite camera".into());
        }
        if !self.measurement_state()["draft"]["picking"].is_null()
            || self.get_entity_placement()["draft"]["picking"] == true
            || self.surface_placement()["picking"] == true
            || self.clip_tool
            || self.spatial.draw_block
            || self.component_picking().is_some()
            || self.component_drag().is_some()
            || self.paint_stroke().is_some()
            || self.transform_drag().is_some()
            || self.area_preview().is_some()
            || self.volume_preview().is_some()
            || self.terrain_move_handles_json()["active"] == true
            || self.terrain_stroke_json()["active"] == true
        {
            return Err("Finish the active viewport tool before picking a path position".into());
        }
        let pick = Picking {
            row,
            size,
            camera,
            session_camera: self.camera,
            camera_serial: self.camera_serial,
            scene: self.scene_serial,
            view: self.view_serial,
        };
        self.geometry.linked_path.serial += 1;
        let preview = self.geometry.linked_path.preview.as_mut().unwrap();
        preview.token = self.geometry.linked_path.serial;
        preview.picking = Some(pick);
        Ok(self.linked_path())
    }

    pub fn cancel_linked_path_pick(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let preview = self
            .geometry
            .linked_path
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Linked path preview was closed or replaced")?;
        if preview.owner != actor {
            return Err(
                "Review the other actor's linked path before cancelling position picking".into(),
            );
        }
        if preview.picking.is_none() {
            return Err("Linked path position picking is not active".into());
        }
        self.geometry.linked_path.serial += 1;
        let preview = self.geometry.linked_path.preview.as_mut().unwrap();
        preview.token = self.geometry.linked_path.serial;
        preview.picking = None;
        Ok(self.linked_path())
    }

    pub fn pick_linked_path_ray(
        &mut self,
        actor: Actor,
        token: u64,
        ray: Ray,
    ) -> Result<Value, String> {
        let row = self.checked_linked_path_pick(actor, token)?.row;
        let preview = self.checked_linked_path(token)?;
        let excluded = preview
            .objects
            .iter()
            .copied()
            .chain(preview.removed.iter().map(|&id| ObjectId::Entity(id)))
            .collect::<Vec<_>>();
        let surface = self.placement_surface(&excluded, ray, true)?;
        let preview = self.checked_linked_path(token)?;
        let mut request = preview.request.clone();
        request.points[row] = surface.hit.point;
        let (request, plan) = if let Some(mut existing) = preview.existing.clone() {
            existing.nodes[row].origin = surface.hit.point;
            self.linked_path_change_plan(&existing)?
        } else {
            let plan = self.linked_path_plan(&request)?;
            (request, plan)
        };
        self.install_linked_path(actor, request, plan)?;
        self.geometry
            .linked_path
            .preview
            .as_mut()
            .unwrap()
            .last_pick = Some((row, surface));
        Ok(self.linked_path())
    }

    pub fn pick_linked_path_pixel(
        &mut self,
        actor: Actor,
        token: u64,
        pointer: Pointer,
    ) -> Result<Value, String> {
        let pick = self.checked_linked_path_pick(actor, token)?;
        let pointer = pointer.validate()?;
        if pointer.size != pick.size {
            return Err("Viewport dimensions changed; choose Pick position again".into());
        }
        if pointer.position.x < 0.0
            || pointer.position.y < 0.0
            || pointer.position.x >= f64::from(pointer.size.0)
            || pointer.position.y >= f64::from(pointer.size.1)
        {
            return Err("Position pixel must lie inside the captured viewport".into());
        }
        let ndc = pointer.ndc();
        let (origin, direction) = pick.camera.ray(
            ndc.x as f32,
            ndc.y as f32,
            pointer.size.0 as f32 / pointer.size.1 as f32,
        );
        self.pick_linked_path_ray(
            actor,
            token,
            Ray {
                origin: origin.as_dvec3(),
                direction: direction.as_dvec3(),
            },
        )
    }
}

#[cfg(test)]
mod tests;
