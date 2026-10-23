use glam::DVec2;
use map_core::query::Ray;
use render::Camera;

use super::*;
use crate::session::{Pointer, viewport_pick::Context};

const HANDLE_RADIUS_PIXELS: f32 = 7.0;
const PICK_RADIUS_PIXELS: f32 = 11.0;
const PLANE_EPSILON: f64 = 1e-5;

pub(super) struct Handles {
    context: Context,
    sources: Vec<FaceId>,
    original: Definition,
    drag: Option<Drag>,
}

struct Drag {
    corner: usize,
    initial: DVec2,
    base: DVec2,
}

fn local_position(definition: &Definition, ray: Ray) -> Result<DVec2, String> {
    let origin = DVec3::from_array(definition.basis_origin);
    let u = DVec3::from_array(definition.basis_u);
    let v = DVec3::from_array(definition.basis_v);
    let normal = u.cross(v);
    let denominator = ray.direction.dot(normal);
    if denominator.abs() < PLANE_EPSILON {
        return Err("The overlay plane is edge-on to the camera".into());
    }
    let distance = (origin - ray.origin).dot(normal) / denominator;
    if distance < 0.0 {
        return Err("The overlay corner is behind the camera".into());
    }
    let point = ray.origin + ray.direction * distance - origin;
    let uu = u.length_squared();
    let vv = v.length_squared();
    let uv = u.dot(v);
    let determinant = uu * vv - uv * uv;
    let position = DVec2::new(
        (point.dot(u) * vv - point.dot(v) * uv) / determinant,
        (point.dot(v) * uu - point.dot(u) * uv) / determinant,
    );
    if !position.is_finite() {
        return Err("Overlay corner movement must remain finite".into());
    }
    Ok(position)
}

impl Session {
    pub fn overlay_corner_handles_active(&self) -> bool {
        self.overlay
            .draft
            .as_ref()
            .is_some_and(|draft| draft.handles.is_some())
    }

    pub fn overlay_corner_handles_owned(&self, actor: Actor) -> bool {
        self.overlay
            .draft
            .as_ref()
            .is_some_and(|draft| draft.owner == actor && draft.handles.is_some())
    }

    pub fn overlay_corner_handles_interactive(
        &self,
        actor: Actor,
        camera: Camera,
        size: (u32, u32),
    ) -> bool {
        self.overlay.draft.as_ref().is_some_and(|draft| {
            self.checked_overlay_corner_handle_viewport(actor, draft.token, camera, size)
                .is_ok()
        })
    }

    pub fn overlay_corner_handle_notice(
        &self,
        actor: Actor,
        camera: Camera,
        size: (u32, u32),
    ) -> Option<String> {
        let draft = self.overlay.draft.as_ref()?;
        draft.handles.as_ref()?;
        if draft.owner != actor {
            return Some(if self.checked_overlay(draft.token).is_ok() {
                format!(
                    "Overlay handles belong to {}. Open Overlay… and Review current draft to take ownership.",
                    draft.owner.label()
                )
            } else {
                format!(
                    "Stale overlay handles belong to {}. Their owner can cancel them, or preview a new corner draft in Overlay….",
                    draft.owner.label()
                )
            });
        }
        Some(match self.checked_overlay_corner_handle_viewport(actor, draft.token, camera, size) {
            Ok(_) => "Drag a cyan overlay corner. Release to review with Apply corners. Escape restores the pre-handle draft.".into(),
            Err(error) => format!("{error}. Cancel handles restores the pre-handle draft."),
        })
    }

    pub fn overlay_corner_handle_dragging(&self) -> bool {
        self.overlay
            .draft
            .as_ref()
            .and_then(|draft| draft.handles.as_ref())
            .is_some_and(|handles| handles.drag.is_some())
    }

    pub fn overlay_corner_handle_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}:{:?}",
            self.overlay_stamp(),
            self.scene_serial,
            self.view_serial,
            self.camera_serial,
            self.camera
        )
    }

    pub fn overlay_corner_handle_state(&self) -> Value {
        let mut state = self.get_overlay_state();
        state["handle_snapshot"] = json!(self.overlay_corner_handle_stamp());
        state
    }

    fn checked_overlay_corner_handles(&self, actor: Actor, token: u64) -> Result<&Handles, String> {
        let draft = self.checked_overlay(token)?;
        if draft.owner != actor {
            return Err("Review the current overlay draft before moving its corner handles".into());
        }
        let handles = draft
            .handles
            .as_ref()
            .ok_or("Choose Viewport handles first")?;
        if !handles.context.is_current(self) {
            return Err(
                "Scene, view or camera changed; cancel viewport handles and start again".into(),
            );
        }
        if handles
            .sources
            .iter()
            .any(|source| !self.working_solid_visible(self.render_document(), source.solid))
        {
            return Err("The bound overlay face is outside the working region".into());
        }
        Ok(handles)
    }

    fn checked_overlay_corner_handle_viewport(
        &self,
        actor: Actor,
        token: u64,
        camera: Camera,
        size: (u32, u32),
    ) -> Result<&Handles, String> {
        let handles = self.checked_overlay_corner_handles(actor, token)?;
        if handles.context.camera() != camera || handles.context.size != size {
            return Err(
                "Viewport camera or dimensions changed; cancel handles and start again".into(),
            );
        }
        Ok(handles)
    }

    pub(super) fn overlay_corner_handles_report(&self) -> Option<Value> {
        let draft = self.overlay.draft.as_ref()?;
        let handles = draft.handles.as_ref()?;
        let checked = self.checked_overlay_corner_handles(draft.owner, draft.token);
        let camera = handles.context.camera();
        let corners = draft.definition.corners().map(|point| {
            render::handles::project(
                &camera,
                DVec3::from_array(point).as_vec3(),
                handles.context.size,
            )
            .map(|point| point.to_array())
        });
        Some(json!({
            "width":handles.context.size.0,"height":handles.context.size.1,
            "valid":checked.is_ok(),"error":checked.err(),
            "dragging":handles.drag.as_ref().map(|drag|drag.corner),
            "pixels":corners,"pick_radius":PICK_RADIUS_PIXELS,
            "snapping":false,
        }))
    }

    fn advance_overlay_corner_handles(&mut self) -> Value {
        let mut handles = self.overlay.draft.as_mut().unwrap().handles.take();
        self.overlay.serial += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        if let Some(handles) = &mut handles {
            handles.context.refresh_scene(self);
        }
        let draft = self.overlay.draft.as_mut().unwrap();
        draft.token = self.overlay.serial;
        draft.handles = handles;
        self.overlay_corner_handle_state()
    }

    pub fn begin_overlay_corner_handles(
        &mut self,
        actor: Actor,
        token: u64,
        camera: Camera,
        size: (u32, u32),
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.overlay_corner_handle_stamp() {
            return Err("Overlay handle context changed; inspect viewport handles again".into());
        }
        let draft = self.checked_overlay(token)?;
        if draft.owner != actor {
            return Err("Review the current overlay draft before starting corner handles".into());
        }
        if !matches!(draft.operation, Operation::Corners(_)) {
            return Err(
                "Preview corners on an existing overlay before starting viewport handles".into(),
            );
        }
        if self.viewport_pick_conflict() || self.measurement_picking() || self.linked_path_picking()
        {
            return Err(
                "Finish the active viewport tool before starting overlay corner handles".into(),
            );
        }
        let sources = std::iter::once(draft.source)
            .chain(draft.additional_sources.iter().copied())
            .collect::<Vec<_>>();
        if sources
            .iter()
            .any(|source| !self.working_solid_visible(self.render_document(), source.solid))
        {
            return Err("The bound overlay face is outside the working region".into());
        }
        let context = Context::capture(self, camera, size, "Overlay corner handles")?;
        let handles = Handles {
            context,
            sources,
            original: draft.definition.clone(),
            drag: None,
        };
        self.overlay.draft.as_mut().unwrap().handles = Some(handles);
        self.log(actor, "Start overlay corner handles");
        Ok(self.advance_overlay_corner_handles())
    }

    pub fn pick_overlay_corner_handle(
        &mut self,
        actor: Actor,
        token: u64,
        pointer: Pointer,
    ) -> Result<Value, String> {
        let handles = self.checked_overlay_corner_handles(actor, token)?;
        let ray = handles.context.ray(pointer)?;
        if handles.drag.is_some() {
            return Err("Release or cancel the current corner drag first".into());
        }
        let draft = self.overlay.draft.as_ref().unwrap();
        let camera = handles.context.camera();
        let corner = draft
            .definition
            .corners()
            .into_iter()
            .enumerate()
            .filter_map(|(index, point)| {
                let pixel = render::handles::project(
                    &camera,
                    DVec3::from_array(point).as_vec3(),
                    handles.context.size,
                )?;
                let distance = pixel.distance(pointer.position.as_vec2());
                (distance <= PICK_RADIUS_PIXELS).then_some((index, distance))
            })
            .min_by(|left, right| left.1.total_cmp(&right.1))
            .map(|(index, _)| index)
            .ok_or("Click one of the four cyan overlay corner handles")?;
        let initial = local_position(&draft.definition, ray)?;
        let point = draft.definition.uv_points[corner];
        let drag = Drag {
            corner,
            initial,
            base: DVec2::new(point[0], point[1]),
        };
        self.overlay
            .draft
            .as_mut()
            .unwrap()
            .handles
            .as_mut()
            .unwrap()
            .drag = Some(drag);
        Ok(self.advance_overlay_corner_handles())
    }

    pub fn move_overlay_corner_handle(
        &mut self,
        actor: Actor,
        token: u64,
        pointer: Pointer,
    ) -> Result<Value, String> {
        let handles = self.checked_overlay_corner_handles(actor, token)?;
        let ray = handles.context.ray(pointer)?;
        let drag = handles.drag.as_ref().ok_or("Pick a corner handle first")?;
        let draft = self.overlay.draft.as_ref().unwrap();
        let Operation::Corners(id) = draft.operation else {
            return Err("Viewport handles require an existing overlay corner draft".into());
        };
        let position = drag.base + (local_position(&draft.definition, ray)? - drag.initial);
        let mut corners = draft.definition.uv_points.map(|point| [point[0], point[1]]);
        corners[drag.corner] = position.to_array();
        let (_, definition) = overlay::with_corner_faces(self.document.vmf(), id, corners)?;
        self.overlay.draft.as_mut().unwrap().definition = definition;
        Ok(self.advance_overlay_corner_handles())
    }

    pub fn finish_overlay_corner_handles(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        let handles = self.checked_overlay_corner_handles(actor, token)?;
        if handles.drag.is_none() {
            return Err("Drag a corner handle or Cancel handles first".into());
        }
        self.overlay.draft.as_mut().unwrap().handles = None;
        self.log(actor, "Finish overlay corner handles");
        Ok(self.advance_overlay_corner_handles())
    }

    pub fn cancel_overlay_corner_handles(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        let draft = self
            .overlay
            .draft
            .as_mut()
            .filter(|draft| draft.token == token)
            .ok_or("Overlay draft was closed or replaced")?;
        if draft.owner != actor {
            return Err("Review the current overlay draft before cancelling corner handles".into());
        }
        let handles = draft
            .handles
            .take()
            .ok_or("Viewport corner handles are not active")?;
        draft.definition = handles.original;
        self.log(actor, "Cancel overlay corner handles");
        Ok(self.advance_overlay_corner_handles())
    }

    pub(super) fn append_overlay_corner_handles(&self, mesh: &mut render::SceneMesh) {
        let Some(draft) = self.overlay.draft.as_ref() else {
            return;
        };
        let Ok(handles) = self.checked_overlay_corner_handles(draft.owner, draft.token) else {
            return;
        };
        let camera = handles.context.camera();
        for (index, point) in draft.definition.corners().into_iter().enumerate() {
            let point = DVec3::from_array(point).as_vec3();
            if render::handles::project(&camera, point, handles.context.size).is_none() {
                continue;
            }
            let radius = render::handles::length(&camera, point, handles.context.size.1)
                * HANDLE_RADIUS_PIXELS
                / render::handles::HANDLE_PIXELS;
            let u = camera.right() * radius;
            let v = camera.up() * radius;
            let points = [point - u - v, point + u - v, point + u + v, point - u + v];
            let color = if handles
                .drag
                .as_ref()
                .is_some_and(|drag| drag.corner == index)
            {
                [255, 195, 85, 255]
            } else {
                [80, 220, 255, 255]
            };
            for edge in 0..4 {
                for point in [points[edge], points[(edge + 1) % 4]] {
                    mesh.lines.push(render::scene::LineVertex {
                        position: point.to_array(),
                        color,
                    });
                }
            }
        }
    }
}
