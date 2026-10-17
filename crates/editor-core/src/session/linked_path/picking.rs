use map_core::query::Ray;
use render::Camera;
use serde_json::Value;

use super::*;
use crate::session::Pointer;

pub(super) struct Picking {
    pub(super) row: usize,
    pub(super) context: super::super::viewport_pick::Context,
}

impl Session {
    pub(super) fn linked_path_picking(&self) -> bool {
        self.geometry
            .linked_path
            .preview
            .as_ref()
            .is_some_and(|preview| preview.picking.is_some())
    }

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
        if !pick.context.is_current(self) {
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
        let context =
            super::super::viewport_pick::Context::capture(self, camera, size, "Position picking")?;
        if self.measurement_picking() || self.viewport_pick_conflict() {
            return Err("Finish the active viewport tool before picking a path position".into());
        }
        let pick = Picking { row, context };
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
        let ray = pick.context.ray(pointer)?;
        self.pick_linked_path_ray(actor, token, ray)
    }
}

#[cfg(test)]
mod tests;
