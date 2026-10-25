use glam::DVec3;
use map_core::{
    ObjectId,
    query::{Ray, SurfaceHit},
};
use render::Camera;
use serde::Serialize;

use super::{Actor, Pointer, Session, TransformTool, viewport_pick::Context};

pub const MAX_VIEWPORT_DIMENSION: u32 = super::viewport_pick::MAX_DIMENSION;

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    picking: Option<Picking>,
    last_pick: Option<Surface>,
}

struct Picking {
    token: u64,
    owner: Actor,
    generation: u64,
    revision: u64,
    definitions: u64,
    selection: Vec<ObjectId>,
    tool: TransformTool,
    context: Context,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Surface {
    pub target: String,
    pub solid: Option<u32>,
    pub side: Option<u32>,
    pub point: [f64; 3],
    pub normal: [f64; 3],
    pub generation: u64,
    pub revision: u64,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct PickReport {
    pub token: u64,
    pub owner: &'static str,
    pub width: u32,
    pub height: u32,
    pub previous_pivot: Option<[f64; 3]>,
    pub valid: bool,
    pub error: Option<String>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Report {
    pub snapshot: String,
    pub current_pivot: Option<[f64; 3]>,
    pub picking: Option<PickReport>,
    pub last_pick: Option<Surface>,
    pub last_pick_matches_pivot: bool,
    pub last_pick_document_current: bool,
    pub begin_error: Option<String>,
    pub policy: &'static str,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct GridReport {
    pub snapshot: String,
    pub before: Option<[f64; 3]>,
    pub after: Option<[f64; 3]>,
    pub grid: f64,
    pub changed: bool,
    pub error: Option<String>,
}

impl Session {
    fn transform_pivot_grid_admission(&self) -> Result<(), String> {
        self.ensure_not_closing()?;
        self.ensure_entity_placement_resolved()?;
        if self.history_navigation_blocked().is_some()
            || self.viewport_pick_conflict()
            || self.measurement_picking()
            || self.linked_path_picking()
            || self.paint_enabled()
            || !self.component_selection().is_empty()
        {
            return Err("Finish or cancel the active tool, preview, build or launch before snapping the transform pivot".into());
        }
        Ok(())
    }

    fn transform_pivot_grid_points(&self) -> Result<(DVec3, DVec3), String> {
        let before = match self.transform_tool.pivot {
            Some(point) => point,
            None => {
                let ids = self.selected_ids();
                if self.face_mode || ids.is_empty() {
                    return Err("Select objects to snap the selection-center pivot".into());
                }
                let display = self.display_bounds_in(&self.document);
                let mut bounds = Vec::with_capacity(ids.len());
                for id in ids {
                    if !self.document.contains(id) {
                        return Err(format!("{id} does not exist"));
                    }
                    let (mins, maxs) = display
                        .get(&id)
                        .copied()
                        .or_else(|| map_core::query::bounds(&self.document, id))
                        .ok_or_else(|| format!("{id} has no valid bounds for a transform pivot"))?;
                    if !mins.is_finite() || !maxs.is_finite() || !mins.cmple(maxs).all() {
                        return Err(format!("{id} has no valid bounds for a transform pivot"));
                    }
                    bounds.push((mins, maxs));
                }
                let (mins, maxs) = map_core::query::union(bounds)
                    .ok_or("Selected objects have no valid bounds for a transform pivot")?;
                (mins + maxs) * 0.5
            }
        };
        let maximum = map_core::primitive::MAX_COORDINATE;
        if !before.is_finite() || before.abs().max_element() > maximum {
            return Err(format!(
                "Transform pivot snapping requires finite coordinates within ±{maximum} units"
            ));
        }
        let after = crate::spatial::Settings {
            snap: true,
            ..self.spatial
        }
        .snap_point(before);
        if !after.is_finite() || after.abs().max_element() > maximum {
            return Err(format!(
                "Snapped transform pivot must stay within ±{maximum} units"
            ));
        }
        Ok((before, after))
    }

    pub fn transform_pivot_grid_stamp(&self) -> String {
        format!(
            "{}:{}:{}",
            self.transform_pivot_pick_stamp(),
            self.spatial.grid_size,
            self.pivot_preferences.local_serial
        )
    }

    pub fn transform_pivot_grid_state(&self) -> GridReport {
        let points = self.transform_pivot_grid_points();
        let error = self
            .transform_pivot_grid_admission()
            .err()
            .or_else(|| points.as_ref().err().cloned());
        GridReport {
            snapshot: self.transform_pivot_grid_stamp(),
            before: points.as_ref().ok().map(|(before, _)| before.to_array()),
            after: points.ok().map(|(_, after)| after.to_array()),
            grid: self.spatial.grid_size,
            changed: false,
            error,
        }
    }

    pub fn snap_transform_pivot_to_grid(
        &mut self,
        actor: Actor,
        snapshot: &str,
    ) -> Result<GridReport, String> {
        if snapshot != self.transform_pivot_grid_stamp() {
            return Err(
                "Transform pivot, selection or grid changed; inspect pivot snapping again".into(),
            );
        }
        self.transform_pivot_grid_admission()?;
        let (before, after) = self.transform_pivot_grid_points()?;
        let changed = self.transform_tool.pivot != Some(after);
        self.set_transform_tool(
            actor,
            TransformTool {
                pivot: Some(after),
                ..self.transform_tool
            },
        )?;
        if self.transform_pivot.last_pick.take().is_some() {
            self.transform_pivot.serial += 1;
        }
        self.log(actor, "snapped transform pivot to grid");
        Ok(GridReport {
            snapshot: self.transform_pivot_grid_stamp(),
            before: Some(before.to_array()),
            after: Some(after.to_array()),
            grid: self.spatial.grid_size,
            changed,
            error: None,
        })
    }

    fn pivot_pick_context(&self) -> Result<(), String> {
        if self.face_mode || self.selected_ids().is_empty() {
            return Err("Select objects before picking a transform pivot".into());
        }
        if self.history_navigation_blocked().is_some() {
            return Err("Finish or cancel the active preview, build or launch before picking a transform pivot".into());
        }
        if self.paint_enabled() || !self.component_selection().is_empty() {
            return Err("Leave paint or component editing before picking a transform pivot".into());
        }
        Ok(())
    }

    fn checked_transform_pivot_pick(&self, actor: Actor, token: u64) -> Result<&Picking, String> {
        let picking = self
            .transform_pivot
            .picking
            .as_ref()
            .filter(|picking| picking.token == token)
            .ok_or("Transform pivot pick was closed or replaced")?;
        if picking.owner != actor {
            return Err(
                "Only the actor that started this pivot pick may choose or cancel it".into(),
            );
        }
        if picking.generation != self.document_generation
            || picking.revision != self.document.revision()
            || picking.definitions != self.definitions_serial()
            || picking.selection != self.selected_ids()
            || picking.tool != self.transform_tool
            || !picking.context.is_current(self)
        {
            return Err("Map, selection, transform settings, view or camera changed; cancel pivot picking and start again".into());
        }
        self.pivot_pick_context()?;
        Ok(picking)
    }

    pub fn transform_pivot_picking(&self) -> bool {
        self.transform_pivot.picking.is_some()
    }

    pub fn transform_pivot_pick_owned(&self, actor: Actor) -> bool {
        self.transform_pivot
            .picking
            .as_ref()
            .is_some_and(|picking| picking.owner == actor)
    }

    pub fn transform_pivot_pick_interactive(
        &self,
        actor: Actor,
        camera: Camera,
        size: (u32, u32),
    ) -> bool {
        self.transform_pivot
            .picking
            .as_ref()
            .is_some_and(|picking| {
                self.checked_transform_pivot_pick(actor, picking.token)
                    .is_ok()
                    && picking.context.camera() == camera
                    && picking.context.size == size
            })
    }

    pub fn transform_pivot_pick_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}:{}:{:?}:{:?}:{:?}",
            self.transform_pivot.serial,
            self.document_generation,
            self.document.revision(),
            self.definitions_serial(),
            self.scene_serial,
            self.view_serial,
            self.camera_serial,
            self.camera,
            self.transform_tool,
            (self.face_mode, self.selected_ids())
        )
    }

    fn transform_pivot_pick_admission(&self) -> Result<(), String> {
        if self.transform_pivot_picking() {
            return Err("Finish or cancel the pending transform pivot pick first".into());
        }
        self.pivot_pick_context()?;
        if self.viewport_pick_conflict() || self.measurement_picking() || self.linked_path_picking()
        {
            return Err("Finish the active viewport tool before picking a transform pivot".into());
        }
        self.selection_pivot(&self.selected_ids())?;
        Ok(())
    }

    pub fn transform_pivot_pick_state(&self) -> Report {
        Report {
            snapshot: self.transform_pivot_pick_stamp(),
            current_pivot: self.transform_tool.pivot.map(|point| point.to_array()),
            picking: self.transform_pivot.picking.as_ref().map(|picking| {
                let checked = self.checked_transform_pivot_pick(picking.owner, picking.token);
                PickReport {
                    token: picking.token,
                    owner: picking.owner.label(),
                    width: picking.context.size.0,
                    height: picking.context.size.1,
                    previous_pivot: picking.tool.pivot.map(|point| point.to_array()),
                    valid: checked.is_ok(),
                    error: checked.err(),
                }
            }),
            last_pick: self.transform_pivot.last_pick.clone(),
            last_pick_matches_pivot: self.transform_pivot.last_pick.as_ref().is_some_and(
                |surface| {
                    self.transform_tool.pivot.map(|point| point.to_array()) == Some(surface.point)
                },
            ),
            last_pick_document_current: self.transform_pivot.last_pick.as_ref().is_some_and(
                |surface| {
                    surface.generation == self.document_generation
                        && surface.revision == self.document.revision()
                },
            ),
            begin_error: self.transform_pivot_pick_admission().err(),
            policy: "Pick the first visible exact brush, terrain sheet or supported model surface, including selected objects. No grid snapping, workplane fallback, offsets or attachment. A hit changes only the shared custom transform pivot and ends picking; misses preserve the old pivot and armed pick. Cancel changes no pivot. Foreign or stale picks never capture desktop input. No map, history, selection, camera, framing or persistence changes.",
        }
    }

    pub fn begin_transform_pivot_pick(
        &mut self,
        actor: Actor,
        camera: Camera,
        size: (u32, u32),
        snapshot: &str,
    ) -> Result<Report, String> {
        if snapshot != self.transform_pivot_pick_stamp() {
            return Err("Transform pivot context changed; inspect pivot picking again".into());
        }
        self.transform_pivot_pick_admission()?;
        let context = Context::capture(self, camera, size, "Transform pivot pick")?;
        self.transform_pivot.serial += 1;
        self.transform_pivot.picking = Some(Picking {
            token: self.transform_pivot.serial,
            owner: actor,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial(),
            selection: self.selected_ids(),
            tool: self.transform_tool,
            context,
        });
        self.log(actor, "started transform pivot picking");
        Ok(self.transform_pivot_pick_state())
    }

    pub fn pick_transform_pivot_ray(
        &mut self,
        actor: Actor,
        token: u64,
        ray: Ray,
    ) -> Result<Report, String> {
        self.checked_transform_pivot_pick(actor, token)?;
        let SurfaceHit { hit, normal, .. } = self.placement_surface(&[], ray, true)?;
        self.set_transform_tool(
            actor,
            TransformTool {
                pivot: Some(hit.point),
                ..self.transform_tool
            },
        )?;
        self.transform_pivot.picking = None;
        self.transform_pivot.serial += 1;
        self.transform_pivot.last_pick = Some(Surface {
            target: crate::identity::id_string(hit.object),
            solid: hit.solid,
            side: hit.side,
            point: hit.point.to_array(),
            normal: normal.to_array(),
            generation: self.document_generation,
            revision: self.document.revision(),
        });
        self.log(actor, "picked custom transform pivot");
        Ok(self.transform_pivot_pick_state())
    }

    pub fn pick_transform_pivot_pixel(
        &mut self,
        actor: Actor,
        token: u64,
        pointer: Pointer,
    ) -> Result<Report, String> {
        let picking = self.checked_transform_pivot_pick(actor, token)?;
        let ray = picking.context.ray(pointer)?;
        self.pick_transform_pivot_ray(actor, token, ray)
    }

    pub fn cancel_transform_pivot_pick(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Report, String> {
        let picking = self
            .transform_pivot
            .picking
            .as_ref()
            .filter(|picking| picking.token == token)
            .ok_or("Transform pivot pick was closed or replaced")?;
        if picking.owner != actor {
            return Err("Only the actor that started this pivot pick may cancel it".into());
        }
        self.transform_pivot.picking = None;
        self.transform_pivot.serial += 1;
        self.log(actor, "cancelled transform pivot picking");
        Ok(self.transform_pivot_pick_state())
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.picking
            .as_ref()
            .map(|picking| serde_json::json!([self.serial, picking.token, picking.owner.label()]))
    }
}

#[cfg(test)]
mod tests;
