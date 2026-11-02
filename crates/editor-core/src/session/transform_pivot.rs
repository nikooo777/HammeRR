use glam::DVec3;
use map_core::{
    ObjectId,
    query::{Ray, SurfaceHit},
};
use render::Camera;
use serde::{Deserialize, Serialize};

use super::{Actor, Pointer, Session, TransformTool, surface_vertex, viewport_pick::Context};

pub const MAX_VIEWPORT_DIMENSION: u32 = super::viewport_pick::MAX_DIMENSION;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PivotSource {
    #[default]
    SelectionCenter,
    CurrentTransformPivot,
    CustomWorldPoint,
}

impl PivotSource {
    pub fn from_request(source: Option<Self>, point: Option<DVec3>) -> Result<Self, String> {
        let source = source.unwrap_or(if point.is_some() {
            Self::CustomWorldPoint
        } else {
            Self::SelectionCenter
        });
        match (source, point) {
            (Self::CustomWorldPoint, Some(point)) => {
                crate::pivot_preferences::validate_point(point)?
            }
            (Self::CustomWorldPoint, None) => {
                return Err("Custom world pivot requires a point".into());
            }
            (_, Some(_)) => {
                return Err("Only custom_world_point accepts explicit pivot coordinates".into());
            }
            (_, None) => {}
        }
        Ok(source)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum AttachmentSource {
    Entity {
        entity: u32,
        local_offset: Option<DVec3>,
    },
    Vertex(surface_vertex::Reference),
}

#[derive(Clone, Debug, PartialEq)]
struct Attachment {
    source: AttachmentSource,
    generation: u64,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct AttachmentInfo {
    pub object: String,
    pub kind: &'static str,
    pub local_offset: Option<[f64; 3]>,
    pub generation: u64,
    pub source: Option<surface_vertex::Reference>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct AttachmentReport {
    pub snapshot: String,
    pub attachment: Option<AttachmentInfo>,
    pub effective_pivot: Option<[f64; 3]>,
    pub mode: &'static str,
    pub error: Option<String>,
    pub attach_error: Option<String>,
    pub detach_error: Option<String>,
    pub policy: &'static str,
}

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    picking: Option<Picking>,
    last_pick: Option<Surface>,
    attachment: Option<Attachment>,
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
    previous_pivot: Option<DVec3>,
    kind: PickKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PickKind {
    WorldPoint,
    AttachVertex,
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
    pub kind: PickKind,
    pub attach_vertex: bool,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Report {
    pub snapshot: String,
    pub current_pivot: Option<[f64; 3]>,
    pub current_error: Option<String>,
    pub attachment: Option<AttachmentInfo>,
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
    pub fn transform_pivot_value(&self) -> Result<Option<DVec3>, String> {
        let Some(attachment) = &self.transform_pivot.attachment else {
            if let Some(point) = self.transform_tool.pivot {
                crate::pivot_preferences::validate_point(point)?;
            }
            return Ok(self.transform_tool.pivot);
        };
        if attachment.generation != self.document_generation {
            return Err("Attached transform pivot belongs to a different map".into());
        }
        let point = match &attachment.source {
            AttachmentSource::Entity {
                entity,
                local_offset,
            } => {
                let object = ObjectId::Entity(*entity);
                match local_offset {
                    Some(offset) => {
                        let (origin, basis) =
                            map_core::measurements::entity_local_pose(self.document.vmf(), object)?;
                        origin + basis * *offset
                    }
                    None => map_core::measurements::entity_origin(self.document.vmf(), object)?,
                }
            }
            AttachmentSource::Vertex(reference) => self.resolve_surface_vertex(reference)?,
        };
        crate::pivot_preferences::validate_point(point)?;
        Ok(Some(point))
    }

    pub fn resolved_transform_pivot(&self, ids: &[ObjectId]) -> Result<DVec3, String> {
        let point = match self.transform_pivot_value()? {
            Some(point) => point,
            None => self.selection_pivot(ids)?,
        };
        crate::pivot_preferences::validate_point(point)?;
        Ok(point)
    }

    pub fn resolve_transform_pivot_request(
        &self,
        ids: &[ObjectId],
        source: Option<PivotSource>,
        point: Option<DVec3>,
    ) -> Result<DVec3, String> {
        match PivotSource::from_request(source, point)? {
            PivotSource::SelectionCenter => self.selection_pivot(ids),
            PivotSource::CurrentTransformPivot => self.resolved_transform_pivot(ids),
            PivotSource::CustomWorldPoint => {
                point.ok_or("Custom world pivot requires a point".into())
            }
        }
    }

    pub fn transform_pivot_attached(&self) -> bool {
        self.transform_pivot.attachment.is_some()
    }

    pub(super) fn clear_transform_pivot_attachment(&mut self) -> bool {
        if self.transform_pivot.attachment.take().is_none() {
            return false;
        }
        self.transform_pivot.serial += 1;
        true
    }

    pub(super) fn reset_transform_pivot_attachment(&mut self) {
        if self.transform_pivot_attached() {
            let point = self.transform_pivot_value().ok().flatten();
            self.clear_transform_pivot_attachment();
            self.transform_tool.pivot = point;
            self.pivot_preferences.local_serial += 1;
        }
    }

    fn pivot_attachment_admission(&self) -> Result<(), String> {
        self.ensure_not_closing()?;
        self.ensure_entity_placement_resolved()?;
        if self.history_navigation_blocked().is_some()
            || self.viewport_pick_conflict()
            || self.measurement_picking()
            || self.linked_path_picking()
            || self.paint_enabled()
            || !self.component_selection().is_empty()
        {
            return Err("Finish or cancel the active tool, preview, build or launch before changing the pivot attachment".into());
        }
        Ok(())
    }

    pub fn transform_pivot_attachment_stamp(&self) -> String {
        format!(
            "pivot-attachment:{}:{}:{}:{}:{:?}",
            self.buffer_review_scope,
            self.surface_vertex_context_stamp(),
            self.pivot_preferences.local_serial,
            self.transform_pivot.serial,
            (self.face_mode, self.selected_ids())
        )
    }

    pub fn transform_pivot_attachment_state(&self) -> AttachmentReport {
        let point = self.resolved_transform_pivot(&self.selected_ids());
        let admission = self.pivot_attachment_admission().err();
        let attachment = self.transform_pivot_attachment_info();
        let mode = match self
            .transform_pivot
            .attachment
            .as_ref()
            .map(|attachment| &attachment.source)
        {
            Some(AttachmentSource::Entity {
                local_offset: Some(_),
                ..
            }) => "attached_local",
            Some(AttachmentSource::Entity { .. }) => "attached_origin",
            Some(AttachmentSource::Vertex(_)) => "attached_vertex",
            None => match self.transform_tool.pivot {
                None => "selection_center",
                Some(point) if point == DVec3::ZERO => "world_origin",
                Some(_) => "custom_world_point",
            },
        };
        AttachmentReport {
            snapshot: self.transform_pivot_attachment_stamp(),
            detach_error: admission.clone().or_else(|| {
                if attachment.is_none() {
                    Some("No transform pivot attachment is active".into())
                } else {
                    point.as_ref().err().cloned()
                }
            }),
            attachment,
            effective_pivot: point.as_ref().ok().map(|point| point.to_array()),
            mode,
            error: point.err(),
            attach_error: admission,
            policy: "Session-local point-entity origin/rigid-local or stable brush, terrain and supported model vertex attachment. Point-local offsets ignore model scale, parenting, FGD orientation adjustments and model meshes; vertex references use the shared source-family identity checks. Vertex picking snaps explicitly to one actual vertex of the first visible surface, not an arbitrary surface point. Missing or invalid targets are unavailable without a world-point fallback; Undo restores resolution when valid identity returns. Detach captures the resolved world point; explicit world replacement repairs unavailable attachments. Previews and drags capture the committed resolved point once. New/Open clear all bindings. Attached pivots cannot be saved as startup defaults; detach explicitly first. No source, history or camera edits.",
        }
    }

    fn transform_pivot_attachment_info(&self) -> Option<AttachmentInfo> {
        let attachment = self.transform_pivot.attachment.as_ref()?;
        let (object, kind, local_offset, source) = match &attachment.source {
            AttachmentSource::Entity {
                entity,
                local_offset,
            } => (
                ObjectId::Entity(*entity),
                if local_offset.is_some() {
                    "local"
                } else {
                    "origin"
                },
                local_offset.map(|offset| offset.to_array()),
                None,
            ),
            AttachmentSource::Vertex(reference) => {
                (reference.object(), "vertex", None, Some(reference.clone()))
            }
        };
        Some(AttachmentInfo {
            object: crate::identity::id_string(object),
            kind,
            local_offset,
            generation: attachment.generation,
            source,
        })
    }

    fn check_pivot_attachment_action(&self, snapshot: &str) -> Result<(), String> {
        if snapshot != self.transform_pivot_attachment_stamp() {
            return Err(
                "Transform pivot, map or selection changed; inspect pivot attachment again".into(),
            );
        }
        self.pivot_attachment_admission()
    }

    fn attach_transform_pivot(
        &mut self,
        actor: Actor,
        object: Option<ObjectId>,
        local: bool,
        snapshot: &str,
    ) -> Result<AttachmentReport, String> {
        self.check_pivot_attachment_action(snapshot)?;
        let object = match object {
            Some(object) => object,
            None => match self.selected_ids().as_slice() {
                [object] => *object,
                _ => return Err("Choose one authored point entity for the pivot attachment".into()),
            },
        };
        let (point, local_offset) = if local {
            let point = self.resolved_transform_pivot(&self.selected_ids())?;
            let (origin, basis) =
                map_core::measurements::entity_local_pose(self.document.vmf(), object)?;
            let offset = basis.transpose() * (point - origin);
            crate::pivot_preferences::validate_point(offset)?;
            (point, Some(offset))
        } else {
            (
                map_core::measurements::entity_origin(self.document.vmf(), object)?,
                None,
            )
        };
        let ObjectId::Entity(entity) = object else {
            return Err("Pivot attachment requires an authored point entity".into());
        };
        self.install_transform_pivot_attachment(
            actor,
            point,
            AttachmentSource::Entity {
                entity,
                local_offset,
            },
        )?;
        self.log(
            actor,
            if local {
                "attached transform pivot to point-entity local frame"
            } else {
                "attached transform pivot to point-entity origin"
            },
        );
        Ok(self.transform_pivot_attachment_state())
    }

    fn install_transform_pivot_attachment(
        &mut self,
        actor: Actor,
        point: DVec3,
        source: AttachmentSource,
    ) -> Result<(), String> {
        self.set_transform_tool_with_pivot(
            actor,
            TransformTool {
                pivot: Some(point),
                ..self.transform_tool
            },
            true,
        )?;
        self.transform_pivot.attachment = Some(Attachment {
            source,
            generation: self.document_generation,
        });
        self.transform_pivot.last_pick = None;
        self.transform_pivot.serial += 1;
        self.pivot_preferences.local_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    pub fn attach_transform_pivot_origin(
        &mut self,
        actor: Actor,
        object: Option<ObjectId>,
        snapshot: &str,
    ) -> Result<AttachmentReport, String> {
        self.attach_transform_pivot(actor, object, false, snapshot)
    }

    pub fn attach_transform_pivot_local(
        &mut self,
        actor: Actor,
        object: Option<ObjectId>,
        snapshot: &str,
    ) -> Result<AttachmentReport, String> {
        self.attach_transform_pivot(actor, object, true, snapshot)
    }

    pub fn detach_transform_pivot(
        &mut self,
        actor: Actor,
        snapshot: &str,
    ) -> Result<AttachmentReport, String> {
        self.check_pivot_attachment_action(snapshot)?;
        if !self.transform_pivot_attached() {
            return Err("No transform pivot attachment is active".into());
        }
        let point = self
            .transform_pivot_value()?
            .ok_or("Attached pivot has no resolved world point")?;
        self.set_transform_tool_with_pivot(
            actor,
            TransformTool {
                pivot: Some(point),
                ..self.transform_tool
            },
            true,
        )?;
        self.log(
            actor,
            "detached transform pivot at its resolved world point",
        );
        Ok(self.transform_pivot_attachment_state())
    }

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
        let ids = self.selected_ids();
        if !self.transform_pivot_attached()
            && self.transform_tool.pivot.is_none()
            && (self.face_mode || ids.is_empty())
        {
            return Err("Select objects to snap the selection-center pivot".into());
        }
        let before = self.resolved_transform_pivot(&ids)?;
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
        let changed = self.transform_pivot_attached() || self.transform_tool.pivot != Some(after);
        self.set_transform_tool_with_pivot(
            actor,
            TransformTool {
                pivot: Some(after),
                ..self.transform_tool
            },
            true,
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
            "{}:{}:{}:{}:{}:{:?}:{:?}:{:?}",
            self.transform_pivot.serial,
            self.surface_vertex_context_stamp(),
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
        let begin_error = self.transform_pivot_pick_admission().err();
        let point = self.transform_pivot_value();
        Report {
            snapshot: self.transform_pivot_pick_stamp(),
            current_pivot: point
                .as_ref()
                .ok()
                .copied()
                .flatten()
                .map(|point| point.to_array()),
            current_error: point.as_ref().err().cloned(),
            attachment: self.transform_pivot_attachment_info(),
            picking: self.transform_pivot.picking.as_ref().map(|picking| {
                let checked = self.checked_transform_pivot_pick(picking.owner, picking.token);
                PickReport {
                    token: picking.token,
                    owner: picking.owner.label(),
                    width: picking.context.size.0,
                    height: picking.context.size.1,
                    previous_pivot: picking.previous_pivot.map(|point| point.to_array()),
                    valid: checked.is_ok(),
                    error: checked.err(),
                    kind: picking.kind,
                    attach_vertex: picking.kind == PickKind::AttachVertex,
                }
            }),
            last_pick: self.transform_pivot.last_pick.clone(),
            last_pick_matches_pivot: self.transform_pivot.last_pick.as_ref().is_some_and(
                |surface| {
                    point
                        .as_ref()
                        .ok()
                        .copied()
                        .flatten()
                        .map(|point| point.to_array())
                        == Some(surface.point)
                },
            ),
            last_pick_document_current: self.transform_pivot.last_pick.as_ref().is_some_and(
                |surface| {
                    surface.generation == self.document_generation
                        && surface.revision == self.document.revision()
                },
            ),
            begin_error,
            policy: "Pick the first visible exact brush, terrain sheet or supported model surface, including selected objects. World-point mode captures a fixed point; attach-vertex mode explicitly snaps to an actual vertex and retains its shared stable source reference. Neither mode uses grid snapping or a workplane fallback. A successful hit replaces any attachment and ends picking; misses or invalid source identities preserve the old pivot and armed pick. Cancel changes no pivot. Foreign or stale picks never capture desktop input. No map, history, selection, camera, framing or persistence changes.",
        }
    }

    pub fn begin_transform_pivot_pick(
        &mut self,
        actor: Actor,
        camera: Camera,
        size: (u32, u32),
        snapshot: &str,
    ) -> Result<Report, String> {
        self.begin_transform_pivot_pick_with_kind(
            actor,
            camera,
            size,
            snapshot,
            PickKind::WorldPoint,
        )
    }

    pub fn begin_transform_pivot_vertex_pick(
        &mut self,
        actor: Actor,
        camera: Camera,
        size: (u32, u32),
        snapshot: &str,
    ) -> Result<Report, String> {
        self.begin_transform_pivot_pick_with_kind(
            actor,
            camera,
            size,
            snapshot,
            PickKind::AttachVertex,
        )
    }

    fn begin_transform_pivot_pick_with_kind(
        &mut self,
        actor: Actor,
        camera: Camera,
        size: (u32, u32),
        snapshot: &str,
        kind: PickKind,
    ) -> Result<Report, String> {
        if snapshot != self.transform_pivot_pick_stamp() {
            return Err("Transform pivot context changed; inspect pivot picking again".into());
        }
        self.transform_pivot_pick_admission()?;
        let context = Context::capture(self, camera, size, "Transform pivot pick")?;
        let previous_pivot = self.transform_pivot_value().ok().flatten();
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
            previous_pivot,
            kind,
        });
        self.log(
            actor,
            match kind {
                PickKind::WorldPoint => "started transform pivot picking",
                PickKind::AttachVertex => "started transform pivot vertex attachment picking",
            },
        );
        Ok(self.transform_pivot_pick_state())
    }

    pub fn pick_transform_pivot_ray(
        &mut self,
        actor: Actor,
        token: u64,
        ray: Ray,
    ) -> Result<Report, String> {
        let kind = self.checked_transform_pivot_pick(actor, token)?.kind;
        match kind {
            PickKind::WorldPoint => {
                let SurfaceHit { hit, normal, .. } = self.placement_surface(&[], ray, true)?;
                self.set_transform_tool_with_pivot(
                    actor,
                    TransformTool {
                        pivot: Some(hit.point),
                        ..self.transform_tool
                    },
                    true,
                )?;
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
            }
            PickKind::AttachVertex => {
                let (reference, point) = self.pick_surface_vertex(ray)?;
                self.install_transform_pivot_attachment(
                    actor,
                    point,
                    AttachmentSource::Vertex(reference),
                )?;
                self.log(actor, "attached transform pivot to picked surface vertex");
            }
        }
        self.transform_pivot.picking = None;
        self.transform_pivot.serial += 1;
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
