use super::previews;
use glam::{DMat3, DVec2, DVec3, Vec2};
use map_core::edit::Transform;
use map_core::region::{Match, Selection};
use map_core::{Document, ObjectId, query};
use render::Camera;
use render::handles::{self, Guide, Handle, Mode};

use super::{Actor, SelectMode, Session};

const GRID_SNAP_ROUNDOFF_ULPS: f64 = 4.0;

pub(super) fn combined_selection<T: Copy + Eq + std::hash::Hash>(
    before: &std::collections::HashSet<T>,
    matches: &[T],
    mode: SelectMode,
) -> std::collections::HashSet<T> {
    let mut next = if mode == SelectMode::Replace {
        std::collections::HashSet::new()
    } else {
        before.clone()
    };
    for id in matches {
        match mode {
            SelectMode::Replace | SelectMode::Add => {
                next.insert(*id);
            }
            SelectMode::Remove => {
                next.remove(id);
            }
            SelectMode::Toggle => {
                if !next.remove(id) {
                    next.insert(*id);
                }
            }
        }
    }
    next
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransformTool {
    pub mode: Option<Mode>,
    pub local: bool,
    pub pivot: Option<DVec3>,
    pub texture_lock: bool,
    pub texture_scale_lock: Option<bool>,
    pub angle_step: f64,
}

impl Default for TransformTool {
    fn default() -> Self {
        Self {
            mode: None,
            local: false,
            pivot: None,
            texture_lock: true,
            texture_scale_lock: None,
            angle_step: 15.0,
        }
    }
}

pub struct TransformPreview {
    pub document: Document,
    pub ids: Vec<ObjectId>,
    pub transform: Transform,
    pub grid_snap: Option<GridSnap>,
    generation: u64,
    revision: u64,
    surface_owner: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct GridSnap {
    pub grid: f64,
    pub bounds_minimum: [f64; 3],
    pub snapped_bounds_minimum: [f64; 3],
    pub delta: [f64; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformOperation {
    Move,
    Rotate,
    Scale,
    Mirror,
    Shear,
}

impl TransformOperation {
    pub fn index(self) -> i32 {
        self as i32
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Rotate => "rotate",
            Self::Scale => "scale",
            Self::Mirror => "mirror",
            Self::Shear => "shear",
        }
    }
}

pub struct TransformPresentation {
    pub primary: TransformOperation,
    pub operations: Vec<TransformOperation>,
    pub summary: String,
}

impl TransformPreview {
    pub fn presentation(&self) -> TransformPresentation {
        let spec = self.transform;
        let mut operations = Vec::new();
        let mut lines = Vec::new();
        for (operation, value, active, unit) in [
            (
                TransformOperation::Move,
                spec.basis.transpose() * spec.translation,
                spec.translation != DVec3::ZERO,
                "units",
            ),
            (
                TransformOperation::Rotate,
                spec.rotation,
                spec.rotation.map(|angle| angle.rem_euclid(360.0)) != DVec3::ZERO,
                "degrees",
            ),
            (
                TransformOperation::Scale,
                spec.scale,
                spec.scale != DVec3::ONE,
                "factors",
            ),
        ] {
            if active {
                operations.push(operation);
                lines.push(format!(
                    "{}: {}, {}, {} {unit}",
                    match operation {
                        TransformOperation::Move => "Move",
                        TransformOperation::Rotate => "Rotate",
                        _ => "Scale",
                    },
                    value.x,
                    value.y,
                    value.z
                ));
            }
        }
        let axes = ["X", "Y", "Z"];
        let reflected = axes
            .into_iter()
            .zip(spec.reflection)
            .filter_map(|(axis, reflected)| reflected.then_some(axis))
            .collect::<Vec<_>>();
        if !reflected.is_empty() {
            operations.push(TransformOperation::Mirror);
            lines.push(format!("Mirror: {}", reflected.join(", ")));
        }
        if let Some(shear) = spec.shear.filter(|shear| shear.factor != 0.0) {
            operations.push(TransformOperation::Shear);
            lines.push(format!(
                "Shear: {} by {} × {}",
                axes[shear.axis], axes[shear.reference], shear.factor
            ));
        }
        if let Some(snap) = self.grid_snap {
            lines.insert(
                0,
                format!(
                    "Snap selection bounds minimum to grid {}\nAnchor: {}, {}, {} → {}, {}, {}",
                    snap.grid,
                    snap.bounds_minimum[0],
                    snap.bounds_minimum[1],
                    snap.bounds_minimum[2],
                    snap.snapped_bounds_minimum[0],
                    snap.snapped_bounds_minimum[1],
                    snap.snapped_bounds_minimum[2],
                ),
            );
            if operations.is_empty() {
                lines.push("Already on grid; no geometry change".into());
            }
        }
        TransformPresentation {
            primary: operations
                .last()
                .copied()
                .unwrap_or(TransformOperation::Move),
            operations,
            summary: if lines.is_empty() {
                "No geometry change".into()
            } else {
                lines.join("\n")
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pointer {
    pub position: DVec2,
    pub size: (u32, u32),
}

impl Pointer {
    pub fn validate(self) -> Result<Self, String> {
        if !self.position.is_finite() || self.size.0 == 0 || self.size.1 == 0 {
            return Err("Pointer needs finite coordinates and a positive viewport size".into());
        }
        Ok(self)
    }

    fn ray(self, camera: Camera) -> (DVec3, DVec3) {
        let ndc = self.ndc();
        let (origin, direction) = camera.ray(
            ndc.x as f32,
            ndc.y as f32,
            self.size.0 as f32 / self.size.1 as f32,
        );
        (origin.as_dvec3(), direction.as_dvec3())
    }

    pub fn ndc(self) -> DVec2 {
        DVec2::new(
            2.0 * self.position.x / f64::from(self.size.0) - 1.0,
            1.0 - 2.0 * self.position.y / f64::from(self.size.1),
        )
    }
}

pub struct TransformDrag {
    pub handle: Handle,
    pub mode: Mode,
    pub ids: Vec<ObjectId>,
    pub pivot: DVec3,
    pub basis: DMat3,
    pub camera: Camera,
    pub pointer: Option<Pointer>,
    pub amount: DVec2,
    initial: DVec2,
    last_angle: f64,
    length: f64,
    extent: DVec3,
    generation: u64,
    revision: u64,
}

pub struct AreaPreview {
    pub camera: Camera,
    pub start: Pointer,
    pub end: DVec2,
    pub rule: Match,
    pub mode: SelectMode,
    pub faces: bool,
    pub matches: Selection,
    generation: u64,
    revision: u64,
}

pub(super) fn axis_parameter(
    pointer: Pointer,
    camera: Camera,
    pivot: DVec3,
    axis: DVec3,
) -> Result<f64, String> {
    let (origin, direction) = pointer.ray(camera);
    let dot = direction.dot(axis);
    let denominator = 1.0 - dot * dot;
    if denominator < 1e-5 {
        return Err("This axis is too close to the viewing direction".into());
    }
    let delta = origin - pivot;
    Ok((delta.dot(axis) - delta.dot(direction) * dot) / denominator)
}

pub(super) fn plane_parameter(
    pointer: Pointer,
    camera: Camera,
    pivot: DVec3,
    u: DVec3,
    v: DVec3,
) -> Result<DVec2, String> {
    let normal = u.cross(v);
    let (origin, direction) = pointer.ray(camera);
    let denominator = direction.dot(normal);
    if denominator.abs() < 1e-5 {
        return Err("This plane is edge-on to the camera".into());
    }
    let distance = (pivot - origin).dot(normal) / denominator;
    if distance < 0.0 {
        return Err("The handle is behind the camera".into());
    }
    let point = origin + direction * distance - pivot;
    Ok(DVec2::new(point.dot(u), point.dot(v)))
}

impl TransformDrag {
    fn parameter(&self, pointer: Pointer) -> Result<DVec2, String> {
        if self.pointer.is_some_and(|start| start.size != pointer.size) {
            return Err("Viewport resized; start the transform again".into());
        }
        if let Some(axis) = self.handle.axis() {
            if self.mode == Mode::Rotate {
                let coordinates = plane_parameter(
                    pointer,
                    self.camera,
                    self.pivot,
                    self.basis.col((axis + 1) % 3),
                    self.basis.col((axis + 2) % 3),
                )?;
                if coordinates.length_squared() < 1e-8 {
                    return Err("Drag away from the rotation center".into());
                }
                return Ok(DVec2::new(
                    coordinates.y.atan2(coordinates.x).to_degrees(),
                    0.0,
                ));
            }
            return Ok(DVec2::new(
                axis_parameter(pointer, self.camera, self.pivot, self.basis.col(axis))?,
                0.0,
            ));
        }
        if let Some((a, b)) = self.handle.plane() {
            return plane_parameter(
                pointer,
                self.camera,
                self.pivot,
                self.basis.col(a),
                self.basis.col(b),
            );
        }
        if self.handle == Handle::View {
            return plane_parameter(
                pointer,
                self.camera,
                self.pivot,
                self.camera.right().as_dvec3(),
                self.camera.up().as_dvec3(),
            );
        }
        Ok(DVec2::new(-pointer.position.y, 0.0))
    }
}

impl Session {
    pub fn transform_tool(&self) -> TransformTool {
        self.transform_tool
    }
    pub fn transform_preview(&self) -> Option<&TransformPreview> {
        self.geometry.transform_preview.as_ref()
    }
    pub fn transform_drag(&self) -> Option<&TransformDrag> {
        self.transform_drag.as_ref()
    }
    pub fn interaction_epoch(&self) -> u64 {
        self.interaction_epoch
    }

    pub fn set_transform_tool(&mut self, actor: Actor, tool: TransformTool) -> Result<(), String> {
        if tool.pivot.is_some_and(|pivot| !pivot.is_finite())
            || !tool.angle_step.is_finite()
            || !(0.0..=180.0).contains(&tool.angle_step)
        {
            return Err("Pivot must be finite and rotation snap between 0 and 180 degrees".into());
        }
        if tool == self.transform_tool
            && !self.geometry.paint.enabled
            && self.component_picking().is_none()
            && self.component_selection().is_empty()
        {
            return Ok(());
        }
        self.cancel_transform_preview();
        self.cancel_area_selection();
        self.transform_tool = tool;
        self.geometry.paint.enabled = false;
        self.clear_component_edit();
        if tool.mode.is_some() {
            self.clip_tool = false;
            self.clear_geometry_previews_except(&[
                previews::Kind::Components,
                previews::Kind::Transform,
                previews::Kind::Face,
                previews::Kind::Paint,
            ]);

            self.clear_volume_selection();
            self.spatial.draw_block = false;
            self.cancel_block_preview();
        }
        self.view_serial += 1;
        self.log(actor, "transform controls changed");
        Ok(())
    }

    pub fn transform_basis(&self, ids: &[ObjectId], local: bool) -> Result<DMat3, String> {
        if !local {
            return Ok(DMat3::IDENTITY);
        }
        let id = ids.iter().min().ok_or("Select objects to transform")?;
        if matches!(id, ObjectId::Entity(_))
            && let Some(instance) = self
                .model_instances(&self.document)
                .instances
                .into_iter()
                .find(|instance| instance.id == *id)
        {
            return Ok(DMat3::from_quat(instance.rotation));
        }
        query::local_basis(&self.document, *id)
            .ok_or("Selected object has no valid local frame".into())
    }

    pub fn transform_guide(&self) -> Option<Guide> {
        if self.face_mode
            || self.clip_tool
            || self.spatial.draw_block
            || self.volume_preview().is_some()
            || !self.component_selection().is_empty()
            || self.component_picking().is_some()
            || self.geometry.hollowing.preview.is_some()
            || self.geometry.carving.preview.is_some()
            || self.geometry.merging.preview.is_some()
            || self.geometry.clipboard.preview.is_some()
        {
            return None;
        }
        let mode = self.transform_tool.mode?;
        let ids = self.selected_ids();
        let pivot = self
            .transform_drag
            .as_ref()
            .map(|drag| drag.pivot)
            .or_else(|| {
                self.geometry
                    .transform_preview
                    .as_ref()
                    .map(|preview| preview.transform.pivot)
            })
            .or(self.transform_tool.pivot)
            .or_else(|| self.selection_pivot(&ids).ok())?;
        let basis = self
            .transform_drag
            .as_ref()
            .map(|drag| drag.basis)
            .or_else(|| self.transform_basis(&ids, self.transform_tool.local).ok())?;
        let origin = self
            .geometry
            .transform_preview
            .as_ref()
            .map_or(pivot, |preview| pivot + preview.transform.translation);
        Some(Guide {
            origin: origin.as_vec3(),
            axes: [
                basis.x_axis.as_vec3(),
                basis.y_axis.as_vec3(),
                basis.z_axis.as_vec3(),
            ],
            mode,
            active: self.transform_drag.as_ref().map(|drag| drag.handle),
        })
    }

    pub fn preview_snap_to_grid(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
    ) -> Result<GridSnap, String> {
        if ids.is_empty() {
            return Err("Select objects to snap to the grid".into());
        }
        for &id in ids {
            if !self.document.contains(id) {
                return Err(format!("{id} does not exist"));
            }
        }
        let (anchor, _) = self
            .object_bounds_in(&self.document, ids)
            .ok_or("Selected objects have no valid bounds")?;
        let grid = self.spatial.grid_size;
        let snapped = (anchor / grid).round() * grid;
        let mut delta = snapped - anchor;
        for axis in 0..3 {
            let scale = anchor[axis].abs().max(snapped[axis].abs()).max(grid);
            if delta[axis].abs() <= GRID_SNAP_ROUNDOFF_ULPS * f64::EPSILON * scale {
                delta[axis] = 0.0;
            }
        }
        let snap = GridSnap {
            grid,
            bounds_minimum: anchor.to_array(),
            snapped_bounds_minimum: snapped.to_array(),
            delta: delta.to_array(),
        };
        let transform = Transform {
            translation: delta,
            texture_lock: self.transform_tool.texture_lock,
            texture_scale_lock: self.transform_tool.texture_scale_lock,
            ..Transform::default()
        };
        self.preview_transform(actor, ids, transform, Some(anchor))?;
        self.geometry.transform_preview.as_mut().unwrap().grid_snap = Some(snap);
        self.view_serial += 1;
        Ok(snap)
    }

    pub fn preview_transform(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
        transform: Transform,
        pivot: Option<DVec3>,
    ) -> Result<DVec3, String> {
        self.preview_transform_owned(actor, ids, transform, pivot, None)
    }

    pub(super) fn preview_surface_transform(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
        transform: Transform,
        pivot: DVec3,
        owner: u64,
    ) -> Result<DVec3, String> {
        self.preview_transform_owned(actor, ids, transform, Some(pivot), Some(owner))
    }

    pub(super) fn surface_transform_owner(&self) -> Option<u64> {
        self.geometry
            .transform_preview
            .as_ref()
            .and_then(|preview| preview.surface_owner)
    }

    fn preview_transform_owned(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
        mut transform: Transform,
        pivot: Option<DVec3>,
        surface_owner: Option<u64>,
    ) -> Result<DVec3, String> {
        transform.pivot = pivot.unwrap_or(self.selection_pivot(ids)?);
        if self
            .geometry
            .transform_preview
            .as_ref()
            .is_some_and(|previous| {
                previous.ids == ids
                    && previous.transform == transform
                    && previous.surface_owner == surface_owner
                    && previous.grid_snap.is_none()
            })
        {
            if actor == Actor::Agent && self.transform_drag.take().is_some() {
                self.interaction_epoch += 1;
                self.view_serial += 1;
            }
            return Ok(transform.pivot);
        }
        let mut document = self.document.preview_snapshot();
        document
            .edit("preview transform", |tx| {
                map_core::edit::transform(tx, ids, transform)
            })
            .map_err(|error| error.to_string())?;
        if actor == Actor::Agent && self.transform_drag.is_some() {
            self.cancel_transform_preview();
        }
        self.select(ids, SelectMode::Replace)?;
        self.clear_geometry_previews_except(&[previews::Kind::Transform, previews::Kind::Paint]);

        self.clear_volume_selection();
        self.clip_tool = false;

        self.cancel_block_preview();
        self.spatial.draw_block = false;
        self.area_preview = None;
        self.geometry.transform_preview = Some(TransformPreview {
            document,
            ids: ids.to_vec(),
            transform,
            grid_snap: None,
            generation: self.document_generation,
            revision: self.document.revision(),
            surface_owner,
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(transform.pivot)
    }

    pub fn cancel_transform_preview(&mut self) {
        self.interaction_epoch += 1;
        let changed =
            self.transform_drag.take().is_some() | self.geometry.transform_preview.take().is_some();
        if changed {
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn commit_transform_preview(&mut self, actor: Actor) -> Result<DVec3, String> {
        let preview = self
            .geometry
            .transform_preview
            .take()
            .ok_or("No transform preview to apply")?;
        self.transform_drag = None;
        self.interaction_epoch += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        if preview.generation != self.document_generation
            || preview.revision != self.document.revision()
        {
            return Err("Map changed; start the transform again".into());
        }
        self.transform_objects(
            actor,
            &preview.ids,
            preview.transform,
            Some(preview.transform.pivot),
        )
    }

    pub fn begin_transform_drag(
        &mut self,
        handle: Handle,
        pointer: Option<Pointer>,
    ) -> Result<(), String> {
        let guide = self
            .transform_guide()
            .ok_or("Choose a transform tool and select objects")?;
        if pointer.is_some_and(|pointer| {
            !handles::shapes(&self.camera, pointer.size, &guide)
                .iter()
                .any(|shape| shape.handle == handle)
        }) {
            return Err("Handle is not available in this view".into());
        }
        let valid = handle.axis().is_some()
            || match guide.mode {
                Mode::Move => handle.plane().is_some() || handle == Handle::View,
                Mode::Rotate => false,
                Mode::Scale => handle == Handle::Uniform,
            };
        if !valid {
            return Err("Handle is not supported by the selected transform tool".into());
        }
        let pointer = pointer.map(Pointer::validate).transpose()?;
        let ids = self.selected_ids();
        let pivot = self
            .transform_tool
            .pivot
            .unwrap_or(self.selection_pivot(&ids)?);
        let basis = self.transform_basis(&ids, self.transform_tool.local)?;
        let (mins, maxs) = self
            .object_bounds_in_basis(&self.document, &ids, basis)
            .ok_or("Selection has no bounds")?;
        let extent = maxs - mins;
        let mut drag = TransformDrag {
            handle,
            mode: guide.mode,
            ids,
            pivot,
            basis,
            camera: self.camera,
            pointer,
            amount: DVec2::ZERO,
            initial: DVec2::ZERO,
            last_angle: 0.0,
            length: f64::from(handles::length(
                &self.camera,
                pivot.as_vec3(),
                pointer.map_or(800, |pointer| pointer.size.1),
            )),
            extent,
            generation: self.document_generation,
            revision: self.document.revision(),
        };
        if let Some(pointer) = pointer {
            drag.initial = drag.parameter(pointer)?;
            drag.last_angle = drag.initial.x;
        }
        self.cancel_transform_preview();
        self.cancel_area_selection();
        self.clear_volume_selection();
        self.transform_drag = Some(drag);
        self.view_serial += 1;
        Ok(())
    }

    pub fn update_transform_drag(
        &mut self,
        pointer: Option<Pointer>,
        amount: Option<DVec2>,
    ) -> Result<Transform, String> {
        if pointer.is_some() == amount.is_some() {
            return Err("Provide pointer or numeric amount, exactly one".into());
        }
        let mut drag = self
            .transform_drag
            .take()
            .ok_or("No active transform handle")?;
        if drag.generation != self.document_generation || drag.revision != self.document.revision()
        {
            self.cancel_transform_preview();
            return Err("Map changed; start the transform again".into());
        }
        let result = self.drag_transform(&mut drag, pointer, amount);
        self.clear_volume_selection();
        self.transform_drag = Some(drag);
        match result {
            Ok(transform) => {
                let ids = self.transform_drag.as_ref().unwrap().ids.clone();
                self.preview_transform(Actor::User, &ids, transform, Some(transform.pivot))?;
                Ok(transform)
            }
            Err(error) => Err(error),
        }
    }

    fn drag_transform(
        &self,
        drag: &mut TransformDrag,
        pointer: Option<Pointer>,
        amount: Option<DVec2>,
    ) -> Result<Transform, String> {
        if pointer.is_some() && drag.pointer.is_none() {
            return Err("Begin with a pointer before sending pointer updates".into());
        }
        let mut value = if let Some(pointer) = pointer {
            let parameter = drag.parameter(pointer.validate()?)?;
            if drag.mode == Mode::Rotate {
                let delta = (parameter.x - drag.last_angle + 180.0).rem_euclid(360.0) - 180.0;
                drag.last_angle = parameter.x;
                DVec2::new(drag.amount.x + delta, 0.0)
            } else if drag.mode == Mode::Scale {
                DVec2::new(
                    if drag.handle == Handle::Uniform {
                        2.0_f64.powf(
                            (parameter.x - drag.initial.x) / f64::from(handles::HANDLE_PIXELS),
                        )
                    } else {
                        1.0 + (parameter.x - drag.initial.x) / drag.length
                    },
                    0.0,
                )
            } else {
                parameter - drag.initial
            }
        } else {
            amount.unwrap()
        };
        if !value.is_finite() {
            return Err("Handle amount must be finite".into());
        }
        if value.y != 0.0 && (drag.handle.axis().is_some() || drag.handle == Handle::Uniform) {
            return Err(
                "This handle accepts one amount; use a plane handle for two components".into(),
            );
        }
        drag.amount = value;
        let snap = self.spatial.snap;
        let mut transform = Transform {
            pivot: drag.pivot,
            basis: drag.basis,
            texture_lock: self.transform_tool.texture_lock,
            texture_scale_lock: self.transform_tool.texture_scale_lock,
            ..Transform::default()
        };
        match drag.mode {
            Mode::Move => {
                if snap {
                    value = (value / self.spatial.grid_size).round() * self.spatial.grid_size;
                }
                transform.translation = if let Some(axis) = drag.handle.axis() {
                    drag.basis.col(axis) * value.x
                } else if let Some((a, b)) = drag.handle.plane() {
                    drag.basis.col(a) * value.x + drag.basis.col(b) * value.y
                } else {
                    drag.camera.right().as_dvec3() * value.x + drag.camera.up().as_dvec3() * value.y
                };
            }
            Mode::Rotate => {
                if snap && self.transform_tool.angle_step > 0.0 {
                    value.x = (value.x / self.transform_tool.angle_step).round()
                        * self.transform_tool.angle_step;
                }
                transform.rotation[drag.handle.axis().unwrap()] = value.x;
            }
            Mode::Scale => {
                if value.x <= 0.0 {
                    return Err("Scale must stay positive; reflection is not supported".into());
                }
                let extent = drag
                    .handle
                    .axis()
                    .map_or(drag.extent.max_element(), |axis| drag.extent[axis]);
                if snap && extent > 1e-8 {
                    value.x = ((value.x * extent / self.spatial.grid_size).round()
                        * self.spatial.grid_size
                        / extent)
                        .max(self.spatial.grid_size / extent);
                }
                if let Some(axis) = drag.handle.axis() {
                    transform.scale[axis] = value.x;
                } else {
                    transform.scale = DVec3::splat(value.x);
                }
            }
        }
        Ok(transform)
    }

    pub fn area_preview(&self) -> Option<&AreaPreview> {
        self.area_preview.as_ref()
    }
    pub fn area_rule(&self) -> Match {
        self.area_rule
    }
    pub fn area_mode(&self) -> SelectMode {
        self.area_mode
    }

    pub fn set_area_settings(&mut self, rule: Match, mode: SelectMode) {
        if self.area_rule == rule && self.area_mode == mode {
            return;
        }
        self.cancel_area_selection();
        self.area_rule = rule;
        self.area_mode = mode;
        self.view_serial += 1;
    }

    pub fn preview_area_selection(
        &mut self,
        actor: Actor,
        camera: Camera,
        start: Pointer,
        end: DVec2,
        rule: Match,
        mode: SelectMode,
    ) -> Result<Selection, String> {
        start.validate()?;
        if !end.is_finite() || (end - start.position).abs().min_element() < 1.0 {
            return Err(
                "Selection area needs finite corners and at least one pixel of width and height"
                    .into(),
            );
        }
        let ndc = [
            start.ndc(),
            Pointer {
                position: end,
                ..start
            }
            .ndc(),
        ];
        let region =
            camera.selection_region(ndc, f64::from(start.size.0) / f64::from(start.size.1));
        if region.planes.len() != 6 {
            return Err("Selection area has a degenerate frustum".into());
        }
        if actor == Actor::Agent {
            self.cancel_area_selection();
        }
        if self.geometry.transform_preview.is_some() || self.transform_drag.is_some() {
            self.cancel_transform_preview();
        }
        self.clear_geometry_previews_except(&[previews::Kind::Transform, previews::Kind::Paint]);

        self.clear_volume_selection();
        self.clip_tool = false;

        self.cancel_block_preview();
        self.spatial.draw_block = false;
        let matches = self.select_region(&region, rule, self.face_mode);
        self.area_preview = Some(AreaPreview {
            camera,
            start,
            end,
            rule,
            mode,
            faces: self.face_mode,
            matches: matches.clone(),
            generation: self.document_generation,
            revision: self.document.revision(),
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(matches)
    }

    pub fn cancel_area_selection(&mut self) {
        self.clear_paint_stroke();
        self.interaction_epoch += 1;
        if self.area_preview.take().is_some() {
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn commit_area_selection(&mut self) -> Result<Selection, String> {
        let preview = self
            .area_preview
            .take()
            .ok_or("No area selection to apply")?;
        self.interaction_epoch += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        if preview.generation != self.document_generation
            || preview.revision != self.document.revision()
            || preview.faces != self.face_mode
        {
            return Err("Map or selection mode changed; draw the area again".into());
        }
        if preview.faces {
            self.select_faces(&preview.matches.faces, preview.mode)?;
        } else {
            self.select_grouped(&preview.matches.objects, preview.mode)?;
        }
        Ok(preview.matches)
    }

    pub fn interaction_camera_changed(&mut self) {
        self.restore_clip_drag();
        self.clear_component_drag();
        if self.transform_drag.is_some() {
            self.cancel_transform_preview();
        }
        self.cancel_area_selection();
    }

    pub fn pick_transform_handle(&self, pointer: Pointer) -> Option<Handle> {
        let guide = self.transform_guide()?;
        handles::pick(
            &self.camera,
            pointer.size,
            &guide,
            Vec2::new(pointer.position.x as f32, pointer.position.y as f32),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use map_core::query::Ray;
    use render::Projection;

    fn fixture() -> (Session, ObjectId) {
        let mut session = Session::new(Err("no assets needed".into()));
        let id = session
            .create_block(
                Actor::User,
                DVec3::new(-32.0, -16.0, 0.0),
                DVec3::new(32.0, 16.0, 64.0),
                "brick/test",
                false,
            )
            .unwrap();
        session.document.mark_saved();
        session.set_camera(Camera::planar(
            Projection::Top,
            DVec3::new(0.0, 0.0, 32.0).as_vec3(),
            256.0,
        ));
        (session, id)
    }

    fn mode(session: &mut Session, mode: Mode) {
        session
            .set_transform_tool(
                Actor::Agent,
                TransformTool {
                    mode: Some(mode),
                    ..TransformTool::default()
                },
            )
            .unwrap();
    }

    #[test]
    fn grid_snap_rigidly_moves_fractional_brush_and_mixed_selections_for_both_actors() {
        for actor in [Actor::User, Actor::Agent] {
            for mixed in [false, true] {
                for texture_lock in [false, true] {
                    let mut session = Session::new(Err("no assets needed".into()));
                    let block = session
                        .create_block(
                            actor,
                            DVec3::new(-10.125, 3.0625, -0.375),
                            DVec3::new(5.375, 18.5625, 15.125),
                            "brick/test",
                            false,
                        )
                        .unwrap();
                    let child = session
                        .create_block(
                            actor,
                            DVec3::new(32.0625, 20.125, 8.1875),
                            DVec3::new(48.5625, 36.625, 24.6875),
                            "brick/test",
                            false,
                        )
                        .unwrap();
                    let entity = session
                        .tie_to_entity(actor, &[child], "func_detail", Default::default())
                        .unwrap();
                    let mut ids = vec![block, entity];
                    if mixed {
                        ids.push(
                            session
                                .create_entity(
                                    actor,
                                    "info_target",
                                    Some(DVec3::new(-32.125, -24.375, -16.0625)),
                                    Default::default(),
                                )
                                .unwrap(),
                        );
                    }
                    session
                        .set_spatial(
                            actor,
                            crate::spatial::Update {
                                grid_size: Some(0.25),
                                snap: Some(false),
                                ..Default::default()
                            },
                        )
                        .unwrap();
                    session
                        .set_transform_tool(
                            actor,
                            TransformTool {
                                local: true,
                                pivot: Some(DVec3::splat(123.0)),
                                texture_lock,
                                ..Default::default()
                            },
                        )
                        .unwrap();
                    session.select(&ids, SelectMode::Replace).unwrap();
                    let before = session.document.vmf().clone();
                    let history = session.document.undo_labels().count();
                    let bounds = session.object_bounds_in(&session.document, &ids).unwrap();
                    let snap = session.preview_snap_to_grid(actor, &ids).unwrap();
                    let delta = DVec3::from_array(snap.delta);
                    assert_eq!(snap.grid, 0.25);
                    assert_eq!(snap.bounds_minimum, bounds.0.to_array());
                    assert_eq!(
                        snap.snapped_bounds_minimum,
                        if mixed {
                            [-40.25, -32.5, -24.0]
                        } else {
                            [-10.25, 3.0, -0.5]
                        }
                    );
                    assert_eq!(session.document.vmf(), &before);
                    assert_eq!(session.document.undo_labels().count(), history);
                    let draft = session.transform_preview().unwrap();
                    assert_eq!(draft.grid_snap, Some(snap));
                    assert_eq!(draft.transform.translation, delta);
                    assert_eq!(draft.transform.basis, DMat3::IDENTITY);
                    assert!(draft.presentation().summary.contains("bounds minimum"));
                    let after = session.render_document().vmf().clone();
                    for (old, new) in before.solids().zip(after.solids()) {
                        for (old, new) in old.sides.iter().zip(&new.sides) {
                            for (old, new) in old.plane.0.iter().zip(new.plane.0) {
                                assert!((new - *old).abs_diff_eq(delta, 1e-8));
                            }
                            if texture_lock {
                                assert!(
                                    (map_core::texel_uv(old, old.plane.0[0])
                                        - map_core::texel_uv(new, new.plane.0[0]))
                                    .length()
                                        < 1e-8
                                );
                            } else {
                                assert_eq!((old.u_axis, old.v_axis), (new.u_axis, new.v_axis));
                            }
                        }
                    }
                    if mixed {
                        let old = before.entities.last().unwrap().origin().unwrap();
                        let new = after.entities.last().unwrap().origin().unwrap();
                        assert!((new - old).abs_diff_eq(delta, 1e-8));
                    }
                    session.commit_transform_preview(actor).unwrap();
                    assert_eq!(session.document.vmf(), &after);
                    assert_eq!(session.document.undo_labels().count(), history + 1);
                    let revision = session.document.revision();
                    let aligned = session.preview_snap_to_grid(actor, &ids).unwrap();
                    assert_eq!(aligned.delta, [0.0; 3]);
                    session.commit_transform_preview(actor).unwrap();
                    assert_eq!(session.document.revision(), revision);
                    assert_eq!(session.document.undo_labels().count(), history + 1);
                    session.undo(actor).unwrap();
                    assert_eq!(session.document.vmf(), &before);
                }
            }
        }
    }

    #[test]
    fn grid_snap_suppresses_only_roundoff_scale_offsets() {
        for actor in [Actor::User, Actor::Agent] {
            let mut session = Session::new(Err("no assets needed".into()));
            let ObjectId::Entity(id) = session
                .create_entity(
                    actor,
                    "info_target",
                    Some(DVec3::new(8.0 + 1e-10, 8.0, 8.0)),
                    Default::default(),
                )
                .unwrap()
            else {
                unreachable!()
            };
            let ids = [ObjectId::Entity(id)];
            let real_offset = session.preview_snap_to_grid(actor, &ids).unwrap();
            assert!(real_offset.delta[0] < -1e-11);
            session.commit_transform_preview(actor).unwrap();
            session
                .set_entity_properties(
                    actor,
                    id,
                    &[("origin".into(), Some("8.000000000000002 8 8".into()))],
                )
                .unwrap();
            let before = session.document.vmf().clone();
            let revision = session.document.revision();
            let history = session.document.undo_labels().count();
            let roundoff = session.preview_snap_to_grid(actor, &ids).unwrap();
            assert!(roundoff.bounds_minimum[0] > 0.0);
            assert_eq!(roundoff.delta, [0.0; 3]);
            session.commit_transform_preview(actor).unwrap();
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.document.revision(), revision);
            assert_eq!(session.document.undo_labels().count(), history);
        }
    }

    #[test]
    fn model_reload_cancels_grid_snap_drafts_and_retains_other_transforms() {
        for actor in [Actor::User, Actor::Agent] {
            let (mut session, id) = fixture();
            let before = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            session.preview_snap_to_grid(actor, &[id]).unwrap();
            assert!(session.refresh_model_assets(actor));
            assert!(session.transform_preview().is_none());
            assert!(session.commit_transform_preview(actor).is_err());
            session
                .preview_transform(
                    actor,
                    &[id],
                    Transform {
                        translation: DVec3::X * 0.125,
                        ..Default::default()
                    },
                    None,
                )
                .unwrap();
            let draft = session.render_document().vmf().clone();
            assert!(!session.refresh_model_assets(actor));
            assert!(session.transform_preview().is_some());
            assert_eq!(session.render_document().vmf(), &draft);
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.document.undo_labels().count(), history);
        }
    }

    #[test]
    fn scale_handles_share_independent_texture_policy_cancel_and_actor_history() {
        for actor in [Actor::User, Actor::Agent] {
            for stretch in [false, true] {
                let (mut session, id) = fixture();
                let before = session.document.vmf().clone();
                session
                    .set_transform_tool(
                        actor,
                        TransformTool {
                            mode: Some(Mode::Scale),
                            texture_lock: false,
                            texture_scale_lock: Some(stretch),
                            ..Default::default()
                        },
                    )
                    .unwrap();
                session.begin_transform_drag(Handle::X, None).unwrap();
                let spec = session
                    .update_transform_drag(None, Some(DVec2::new(2.0, 0.0)))
                    .unwrap();
                assert!(!spec.texture_lock);
                assert_eq!(spec.texture_scale_lock, Some(stretch));
                assert_eq!(session.document.vmf(), &before);
                let draft = session.render_document().vmf().clone();
                session.cancel_transform_preview();
                assert_eq!(session.render_document().vmf(), &before);
                session.begin_transform_drag(Handle::X, None).unwrap();
                session
                    .update_transform_drag(None, Some(DVec2::new(2.0, 0.0)))
                    .unwrap();
                session.commit_transform_preview(actor).unwrap();
                assert_eq!(session.document.vmf(), &draft);
                let ObjectId::Solid(solid) = id else {
                    unreachable!()
                };
                let old = before
                    .world
                    .solids
                    .iter()
                    .find(|brush| brush.id == solid)
                    .unwrap();
                let new = session.document.solid(solid).unwrap();
                for (old, new) in old.sides.iter().zip(&new.sides) {
                    if stretch {
                        assert!(
                            (map_core::texel_uv(old, old.plane.0[0])
                                - map_core::texel_uv(new, new.plane.0[0]))
                            .length()
                                < 1e-8
                        );
                    } else {
                        assert_eq!((old.u_axis, old.v_axis), (new.u_axis, new.v_axis));
                    }
                }
                session.undo(actor).unwrap();
                assert_eq!(session.document.vmf(), &before);
            }
        }
    }

    #[test]
    fn handle_updates_are_total_snapped_previews_with_single_step_undo_and_cancel() {
        let (mut session, id) = fixture();
        let original = session.document.vmf().clone();
        let revision = session.document.revision();
        mode(&mut session, Mode::Move);
        session.begin_transform_drag(Handle::X, None).unwrap();
        for amount in [16.0, 35.0] {
            session
                .update_transform_drag(None, Some(DVec2::new(amount, 0.0)))
                .unwrap();
        }
        assert_eq!(
            session.transform_preview().unwrap().transform.translation,
            DVec3::new(32.0, 0.0, 0.0)
        );
        assert_eq!(session.document.vmf(), &original);
        assert!(!session.document.is_dirty());
        assert_eq!(session.document.revision(), revision);
        assert_eq!(session.document.undo_labels().count(), 1);
        let preview = session.render_document().vmf().clone();
        session.commit_transform_preview(Actor::User).unwrap();
        assert_eq!(session.document.vmf(), &preview);
        assert_eq!(session.selected_ids(), vec![id]);
        assert_eq!(session.document.undo_labels().count(), 2);
        session.undo(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &original);
        session.redo(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &preview);
        session.begin_transform_drag(Handle::XY, None).unwrap();
        session
            .update_transform_drag(None, Some(DVec2::new(48.0, 16.0)))
            .unwrap();
        session.cancel_transform_preview();
        assert_eq!(session.document.vmf(), &preview);
        assert!(session.commit_transform_preview(Actor::Agent).is_err());
    }

    #[test]
    fn local_axes_rotation_scaling_and_invalid_settings_preserve_the_batch() {
        let (mut session, id) = fixture();
        session
            .transform_objects(
                Actor::User,
                &[id],
                Transform {
                    rotation: DVec3::new(0.0, 0.0, 90.0),
                    ..Transform::default()
                },
                None,
            )
            .unwrap();
        let basis = session.transform_basis(&[id], true).unwrap();
        assert!(basis.x_axis.abs_diff_eq(DVec3::Y, 1e-8));
        session
            .set_transform_tool(
                Actor::Agent,
                TransformTool {
                    mode: Some(Mode::Move),
                    local: true,
                    ..TransformTool::default()
                },
            )
            .unwrap();
        session.begin_transform_drag(Handle::X, None).unwrap();
        let transform = session
            .update_transform_drag(None, Some(DVec2::new(16.0, 0.0)))
            .unwrap();
        assert!(transform.translation.abs_diff_eq(DVec3::Y * 16.0, 1e-8));
        assert!(
            session
                .update_transform_drag(
                    Some(Pointer {
                        position: DVec2::ZERO,
                        size: (1000, 1000)
                    }),
                    None
                )
                .is_err()
        );
        assert_eq!(session.transform_preview().unwrap().transform, transform);
        session.commit_transform_preview(Actor::Agent).unwrap();
        mode(&mut session, Mode::Rotate);
        session.begin_transform_drag(Handle::Z, None).unwrap();
        assert_eq!(
            session
                .update_transform_drag(None, Some(DVec2::new(23.0, 0.0)))
                .unwrap()
                .rotation
                .z,
            30.0
        );
        session.cancel_transform_preview();
        mode(&mut session, Mode::Scale);
        session.begin_transform_drag(Handle::Uniform, None).unwrap();
        assert!(
            session
                .update_transform_drag(None, Some(DVec2::new(2.0, 0.0)))
                .unwrap()
                .scale
                .abs_diff_eq(DVec3::splat(2.0), 1e-9)
        );
        let before = session.render_document().vmf().clone();
        assert!(
            session
                .update_transform_drag(None, Some(DVec2::new(-1.0, 0.0)))
                .is_err()
        );
        assert_eq!(session.render_document().vmf(), &before);
        let settings = session.transform_tool();
        assert!(
            session
                .set_transform_tool(
                    Actor::User,
                    TransformTool {
                        angle_step: f64::NAN,
                        ..settings
                    }
                )
                .is_err()
        );
        assert_eq!(session.transform_tool(), settings);
        assert!(session.transform_preview().is_some());
    }

    #[test]
    fn picking_uses_transform_drafts_and_changes_invalidate_active_handles() {
        let (mut session, id) = fixture();
        let ray = Ray {
            origin: DVec3::new(-128.0, 0.0, 32.0),
            direction: DVec3::X,
        };
        assert_eq!(session.pick(&ray).unwrap().point.x, -32.0);
        mode(&mut session, Mode::Move);
        session.begin_transform_drag(Handle::X, None).unwrap();
        session
            .update_transform_drag(None, Some(DVec2::new(48.0, 0.0)))
            .unwrap();
        assert_eq!(session.pick(&ray).unwrap().point.x, 16.0);
        session.set_camera(session.camera());
        assert!(session.transform_drag().is_none());
        assert_eq!(session.pick(&ray).unwrap().point.x, -32.0);
        session
            .preview_transform(
                Actor::Agent,
                &[id],
                Transform {
                    translation: DVec3::X * 16.0,
                    ..Transform::default()
                },
                Some(DVec3::new(12.0, 24.0, 36.0)),
            )
            .unwrap();
        assert_eq!(
            session.transform_guide().unwrap().origin,
            glam::Vec3::new(28.0, 24.0, 36.0)
        );
        session.set_camera(session.camera());
        assert!(session.transform_preview().is_some());
        session
            .set_spatial(
                Actor::Agent,
                crate::spatial::Update {
                    grid_size: Some(0.25),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(session.transform_preview().is_none());
        session.begin_transform_drag(Handle::X, None).unwrap();
        session.select(&[], SelectMode::Replace).unwrap();
        assert!(
            session
                .update_transform_drag(None, Some(DVec2::new(1.0, 0.0)))
                .is_err()
        );
        assert!(session.transform_guide().is_none());
    }

    #[test]
    fn area_previews_rules_modes_faces_and_reverse_corners_share_selection_without_undo() {
        let (mut session, id) = fixture();
        let outside = session
            .create_block(
                Actor::User,
                DVec3::new(160.0, 0.0, 0.0),
                DVec3::new(192.0, 32.0, 64.0),
                "brick/test",
                false,
            )
            .unwrap();
        let light = session
            .create_entity(
                Actor::User,
                "light",
                Some(DVec3::new(0.0, 0.0, 128.0)),
                Default::default(),
            )
            .unwrap();
        session.select(&[outside], SelectMode::Replace).unwrap();
        let revision = session.document.revision();
        let camera = session.camera();
        let start = Pointer {
            position: DVec2::new(350.0, 400.0),
            size: (1000, 1000),
        };
        let end = DVec2::new(650.0, 600.0);
        let matches = session
            .preview_area_selection(
                Actor::Agent,
                camera,
                start,
                end,
                Match::Contained,
                SelectMode::Replace,
            )
            .unwrap();
        assert_eq!(matches.objects, vec![id, light]);
        assert_eq!(session.selected_ids(), vec![outside]);
        session.cancel_area_selection();
        assert_eq!(session.selected_ids(), vec![outside]);
        for (mode, expected) in [
            (SelectMode::Replace, vec![id, light]),
            (SelectMode::Add, vec![id, outside, light]),
            (SelectMode::Remove, vec![outside]),
            (SelectMode::Toggle, vec![id, outside, light]),
        ] {
            session.select(&[outside], SelectMode::Replace).unwrap();
            session
                .preview_area_selection(
                    Actor::User,
                    camera,
                    Pointer {
                        position: end,
                        ..start
                    },
                    start.position,
                    Match::Contained,
                    mode,
                )
                .unwrap();
            session.commit_area_selection().unwrap();
            assert_eq!(session.selected_ids(), expected);
        }
        let partial = Pointer {
            position: DVec2::new(500.0, 400.0),
            ..start
        };
        assert!(
            session
                .preview_area_selection(
                    Actor::Agent,
                    camera,
                    partial,
                    end,
                    Match::Contained,
                    SelectMode::Replace
                )
                .unwrap()
                .objects
                .is_empty()
        );
        assert_eq!(
            session
                .preview_area_selection(
                    Actor::Agent,
                    camera,
                    partial,
                    end,
                    Match::Intersecting,
                    SelectMode::Replace
                )
                .unwrap()
                .objects,
            vec![id, light]
        );
        session.cancel_area_selection();
        session.set_face_mode(true);
        let matches = session
            .preview_area_selection(
                Actor::User,
                camera,
                start,
                end,
                Match::Contained,
                SelectMode::Replace,
            )
            .unwrap();
        assert!(matches.objects.is_empty());
        assert_eq!(matches.faces.len(), 6);
        session.commit_area_selection().unwrap();
        assert_eq!(session.selected_faces(), matches.faces);
        assert_eq!(session.document.revision(), revision);
        assert!(
            session
                .document
                .undo_labels()
                .all(|label| !label.contains("select"))
        );
    }

    #[test]
    fn area_queries_filter_tools_and_hidden_objects_and_reject_invalid_rectangles_atomically() {
        let (mut session, id) = fixture();
        let assets = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(assets.path().join("materials/tools")).unwrap();
        std::fs::write(
            assets.path().join("materials/tools/toolsnodraw.vmt"),
            "UnlitGeneric { %compilenodraw 1 }",
        )
        .unwrap();
        let gameinfo = assets.path().join("gameinfo.txt");
        std::fs::write(
            &gameinfo,
            "GameInfo { FileSystem { SearchPaths { Game |gameinfo_path|. } } }",
        )
        .unwrap();
        session.game =
            std::sync::Arc::new(source_formats::gamefs::GameFs::from_gameinfo(&gameinfo).unwrap());
        let tool = session
            .create_block(
                Actor::User,
                DVec3::splat(-8.0),
                DVec3::splat(8.0),
                "tools/toolsnodraw",
                false,
            )
            .unwrap();
        let ObjectId::Solid(number) = id else {
            unreachable!()
        };
        session
            .edit(Actor::User, "hide", |tx| {
                tx.solid_mut(number)?.hidden = true;
                Ok(())
            })
            .unwrap();
        let mut view = session.view();
        view.tool_textures = false;
        session.set_view(view);
        let camera = session.camera();
        let start = Pointer {
            position: DVec2::new(100.0, 100.0),
            size: (1000, 1000),
        };
        let end = DVec2::new(900.0, 900.0);
        let matches = session
            .preview_area_selection(
                Actor::Agent,
                camera,
                start,
                end,
                Match::Intersecting,
                SelectMode::Replace,
            )
            .unwrap();
        assert!(matches.objects.is_empty());
        assert!(
            session
                .preview_area_selection(
                    Actor::Agent,
                    camera,
                    start,
                    start.position,
                    Match::Contained,
                    SelectMode::Replace
                )
                .is_err()
        );
        assert!(session.area_preview().is_some());
        assert_eq!(session.selected_ids(), vec![tool]);
        let mut view = session.view();
        view.tool_textures = true;
        session.set_view(view);
        assert!(session.area_preview().is_none());
        assert_eq!(
            session
                .preview_area_selection(
                    Actor::Agent,
                    camera,
                    start,
                    end,
                    Match::Contained,
                    SelectMode::Replace
                )
                .unwrap()
                .objects,
            vec![tool]
        );
        session.new_map(Actor::User, true).unwrap();
        assert!(session.commit_area_selection().is_err());
    }
}
