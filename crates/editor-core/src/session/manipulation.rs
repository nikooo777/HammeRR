use super::previews;
use glam::{DMat3, DVec2, DVec3, Vec2};
use map_core::edit::Transform;
use map_core::region::{Match, Selection};
use map_core::{Document, ObjectId, query, query::Ray};
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
    pub grid_snap: Option<GridSnapPreview>,
    pub magnetic: Option<super::placement::magnetic::Report>,
    pub actor: Actor,
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

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Deserialize,
    serde::Serialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum GridSnapMode {
    #[default]
    Selection,
    EachObject,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ObjectGridSnap {
    #[serde(serialize_with = "serialize_snap_id")]
    pub id: ObjectId,
    #[serde(flatten)]
    pub snap: GridSnap,
}

fn serialize_snap_id<S: serde::Serializer>(
    id: &ObjectId,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&crate::identity::id_string(*id))
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(untagged)]
pub enum GridSnapPreview {
    Selection(GridSnap),
    EachObject {
        mode: GridSnapMode,
        grid: f64,
        objects: Vec<ObjectGridSnap>,
    },
}

impl GridSnapPreview {
    pub fn mode(&self) -> GridSnapMode {
        match self {
            Self::Selection(_) => GridSnapMode::Selection,
            Self::EachObject { .. } => GridSnapMode::EachObject,
        }
    }
}

fn grid_snap(anchor: DVec3, grid: f64) -> GridSnap {
    let snapped = (anchor / grid).round() * grid;
    let mut delta = snapped - anchor;
    for axis in 0..3 {
        let scale = anchor[axis].abs().max(snapped[axis].abs()).max(grid);
        if delta[axis].abs() <= GRID_SNAP_ROUNDOFF_ULPS * f64::EPSILON * scale {
            delta[axis] = 0.0;
        }
    }
    GridSnap {
        grid,
        bounds_minimum: anchor.to_array(),
        snapped_bounds_minimum: snapped.to_array(),
        delta: delta.to_array(),
    }
}

fn apply_transform_plan(
    tx: &mut map_core::Transaction,
    ids: &[ObjectId],
    transform: Transform,
    snap: Option<&GridSnapPreview>,
) -> Result<(), map_core::EditError> {
    if let Some(GridSnapPreview::EachObject { objects, .. }) = snap {
        let transforms = objects
            .iter()
            .map(|object| {
                (
                    object.id,
                    Transform {
                        translation: DVec3::from_array(object.snap.delta),
                        ..transform
                    },
                )
            })
            .collect::<Vec<_>>();
        map_core::edit::transform_each(tx, &transforms)
    } else {
        map_core::edit::transform(tx, ids, transform)
    }
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
        if let Some(GridSnapPreview::EachObject { grid, objects, .. }) = &self.grid_snap {
            let moved = objects
                .iter()
                .filter(|object| object.snap.delta != [0.0; 3])
                .count();
            return TransformPresentation {
                primary: TransformOperation::Move,
                operations: if moved == 0 {
                    Vec::new()
                } else {
                    vec![TransformOperation::Move]
                },
                summary: format!(
                    "Snap each object's bounds minimum to grid {grid}\n{} objects · {moved} to move{}",
                    objects.len(),
                    if moved == 0 {
                        "\nAlready on grid; no geometry change"
                    } else {
                        ""
                    },
                ),
            };
        }
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
        if let Some(GridSnapPreview::Selection(snap)) = &self.grid_snap {
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

    pub(super) fn ray(self, camera: Camera) -> (DVec3, DVec3) {
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
    filters: Option<u64>,
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
        if let Some(pivot) = tool.pivot {
            crate::pivot_preferences::validate_point(pivot)?;
        }
        if !tool.angle_step.is_finite() || !(0.0..=180.0).contains(&tool.angle_step) {
            return Err("Rotation snap must be finite and between 0 and 180 degrees".into());
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
        if self.transform_tool.pivot != tool.pivot {
            self.pivot_preferences.local_serial += 1;
        }
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
        if !self.working_selection_visible() {
            return None;
        }
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
            || self.entity_placement_preview_id().is_some()
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
            if !self.selection_contains(id) {
                return Err(format!("{id} does not exist"));
            }
        }
        let (anchor, _) = self
            .object_bounds_in(&self.document, ids)
            .ok_or("Selected objects have no valid bounds")?;
        let grid = self.spatial.grid_size;
        let snap = grid_snap(anchor, grid);
        let transform = Transform {
            translation: DVec3::from_array(snap.delta),
            texture_lock: self.transform_tool.texture_lock,
            texture_scale_lock: self.transform_tool.texture_scale_lock,
            ..Transform::default()
        };
        self.preview_transform(actor, ids, transform, Some(anchor))?;
        self.geometry.transform_preview.as_mut().unwrap().grid_snap =
            Some(GridSnapPreview::Selection(snap));
        self.view_serial += 1;
        Ok(snap)
    }

    pub fn preview_snap_to_grid_mode(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
        mode: GridSnapMode,
    ) -> Result<GridSnapPreview, String> {
        if mode == GridSnapMode::Selection {
            return self
                .preview_snap_to_grid(actor, ids)
                .map(GridSnapPreview::Selection);
        }
        let ids = ids
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        self.whole_translation_roots(&ids)?;
        let mut context = self.placement_support_context(&self.document);
        let grid = self.spatial.grid_size;
        let mut objects = Vec::with_capacity(ids.len());
        for &id in &ids {
            let (anchor, maximum) =
                super::models::combined_bounds(&self.document, &[id], &context.bounds)
                    .ok_or_else(|| format!("{id} has no valid bounds"))?;
            if !anchor.is_finite() || !maximum.is_finite() {
                return Err(format!("{id} has nonfinite bounds"));
            }
            let snap = grid_snap(anchor, grid);
            self.object_translation_bounds_prepared(
                &[id],
                DVec3::from_array(snap.delta),
                Some(&mut context),
            )?;
            objects.push(ObjectGridSnap { id, snap });
        }
        let snap = GridSnapPreview::EachObject {
            mode,
            grid,
            objects,
        };
        let transform = Transform {
            texture_lock: self.transform_tool.texture_lock,
            texture_scale_lock: self.transform_tool.texture_scale_lock,
            ..Transform::default()
        };
        self.preview_transform_plan(actor, &ids, transform, None, None, Some(snap.clone()))?;
        Ok(snap)
    }

    pub fn preview_transform(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
        transform: Transform,
        pivot: Option<DVec3>,
    ) -> Result<DVec3, String> {
        let pivot = self.preview_transform_owned(actor, ids, transform, pivot, None)?;
        if self
            .geometry
            .transform_preview
            .as_mut()
            .is_some_and(|preview| preview.magnetic.take().is_some())
        {
            self.scene_serial += 1;
            self.view_serial += 1;
        }
        Ok(pivot)
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
        transform: Transform,
        pivot: Option<DVec3>,
        surface_owner: Option<u64>,
    ) -> Result<DVec3, String> {
        self.preview_transform_plan(actor, ids, transform, pivot, surface_owner, None)
    }

    fn preview_transform_plan(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
        mut transform: Transform,
        pivot: Option<DVec3>,
        surface_owner: Option<u64>,
        grid_snap: Option<GridSnapPreview>,
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
                    && previous.grid_snap == grid_snap
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
                apply_transform_plan(tx, ids, transform, grid_snap.as_ref())
            })
            .map_err(|error| error.to_string())?;
        if actor == Actor::Agent && self.transform_drag.is_some() {
            self.cancel_transform_preview();
        }
        self.select(ids, SelectMode::Replace)?;
        self.clear_geometry_previews_except(&[previews::Kind::Transform]);
        if self.transform_drag.is_none() {
            self.cancel_selection_previews();
        } else {
            self.clear_volume_selection();
            self.area_preview = None;
        }
        self.clip_tool = false;

        self.cancel_block_preview();
        self.spatial.draw_block = false;
        self.geometry.transform_preview = Some(TransformPreview {
            document,
            ids: ids.to_vec(),
            transform,
            grid_snap,
            magnetic: None,
            actor,
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
        if self
            .geometry
            .transform_preview
            .as_ref()
            .and_then(|preview| preview.magnetic.as_ref())
            .is_some_and(|report| self.magnetic_preview_stale(report))
        {
            return Err(
                "Magnetic source, target or visibility changed; preview movement again".into(),
            );
        }
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
        if matches!(preview.grid_snap, Some(GridSnapPreview::EachObject { .. })) {
            self.edit(
                actor,
                &format!("snap {} objects to grid", preview.ids.len()),
                |tx| {
                    apply_transform_plan(
                        tx,
                        &preview.ids,
                        preview.transform,
                        preview.grid_snap.as_ref(),
                    )
                },
            )?;
            return Ok(preview.transform.pivot);
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
                if let Some(pointer) = pointer.filter(|_| {
                    self.magnetic_snapping_enabled()
                        && self
                            .transform_drag
                            .as_ref()
                            .is_some_and(|drag| drag.mode == Mode::Move)
                }) {
                    let drag = self.transform_drag.as_ref().unwrap();
                    let ndc = pointer.ndc();
                    let (origin, direction) = drag.camera.ray(
                        ndc.x as f32,
                        ndc.y as f32,
                        pointer.size.0 as f32 / pointer.size.1 as f32,
                    );
                    self.preview_magnetic_transform(
                        Actor::User,
                        &ids,
                        transform,
                        Ray {
                            origin: origin.as_dvec3(),
                            direction: direction.as_dvec3(),
                        },
                    )
                } else {
                    self.preview_transform(Actor::User, &ids, transform, Some(transform.pivot))?;
                    Ok(transform)
                }
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
            filters: (!self.face_mode).then(|| self.selection_filter_revision()),
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

    pub(super) fn cancel_selection_previews(&mut self) {
        self.cancel_area_selection();
        self.clear_volume_selection();
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
            || preview
                .filters
                .is_some_and(|revision| revision != self.selection_filter_revision())
            || preview.faces != self.face_mode
        {
            return Err("Map or selection mode changed; draw the area again".into());
        }
        if preview.faces {
            self.select_faces(&preview.matches.faces, preview.mode)?;
        } else {
            self.select_picked_objects(&preview.matches.objects, preview.mode)?;
        }
        Ok(preview.matches)
    }

    pub(super) fn cancel_geometry_drags(&mut self) {
        self.restore_clip_drag();
        self.clear_component_drag();
        if self.transform_drag.is_some() {
            self.cancel_transform_preview();
        }
    }

    pub fn interaction_camera_changed(&mut self) {
        self.cancel_geometry_drags();
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

impl TransformDrag {
    pub(super) fn close_draft(&self) -> serde_json::Value {
        serde_json::json!([self.amount.to_array(), self.last_angle])
    }
}
