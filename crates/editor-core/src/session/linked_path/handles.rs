use super::*;
use crate::session::{
    Pointer,
    manipulation::{axis_parameter, plane_parameter},
    viewport_pick::Context,
};
use glam::{DVec2, Vec3};
use render::{
    Camera,
    handles::{Guide, Handle, Mode},
};

pub(super) struct Handles {
    row: usize,
    context: Context,
    drag: Option<Drag>,
}

impl Handles {
    pub(super) fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

struct Drag {
    handle: Handle,
    origin: DVec3,
    initial: DVec2,
    pointer: bool,
    document: Document,
    request: Request,
    existing: Option<ChangeRequest>,
    core: Option<operations::Draft>,
    last_pick: Option<(usize, map_core::query::SurfaceHit)>,
}

fn parameter(
    context: &Context,
    handle: Handle,
    origin: DVec3,
    pointer: Pointer,
) -> Result<DVec2, String> {
    context.ray(pointer)?;
    if let Some(axis) = handle.axis() {
        axis_parameter(pointer, context.camera(), origin, DVec3::AXES[axis])
            .map(|value| DVec2::new(value, 0.0))
    } else if let Some((u, v)) = handle.plane() {
        plane_parameter(
            pointer,
            context.camera(),
            origin,
            DVec3::AXES[u],
            DVec3::AXES[v],
        )
    } else if handle == Handle::View {
        plane_parameter(
            pointer,
            context.camera(),
            origin,
            context.camera().right().as_dvec3(),
            context.camera().up().as_dvec3(),
        )
    } else {
        Err("Path nodes support move axes, planes and the view handle".into())
    }
}

impl Session {
    pub(in crate::session) fn append_linked_path_guides(&self, mesh: &mut render::SceneMesh) {
        let Some(preview) = self
            .geometry
            .linked_path
            .preview
            .as_ref()
            .filter(|preview| self.checked_linked_path(preview.token).is_ok())
        else {
            return;
        };
        let points = &preview.request.points;
        let closing = preview
            .request
            .closed
            .then(|| [points[points.len() - 1], points[0]]);
        for [start, end] in points
            .windows(2)
            .map(|pair| [pair[0], pair[1]])
            .chain(closing)
        {
            let color = [100, 210, 255, 255];
            mesh.add_guide_line(start, end, color);
            let delta = end - start;
            let direction = (delta / delta.abs().max_element()).normalize();
            let side = direction.any_orthonormal_vector();
            let up = direction.cross(side);
            let length = (start.distance(end) * 0.15).min(16.0);
            let tip = start.lerp(end, 0.6);
            for wing in [side, -side, up, -up] {
                mesh.add_guide_line(tip, tip - direction * length + wing * length * 0.5, color);
            }
        }
    }

    pub fn linked_path_handles_active(&self) -> bool {
        self.geometry
            .linked_path
            .preview
            .as_ref()
            .is_some_and(|preview| preview.handles.is_some())
    }

    pub fn linked_path_handle_dragging(&self) -> bool {
        self.geometry
            .linked_path
            .preview
            .as_ref()
            .and_then(|preview| preview.handles.as_ref())
            .is_some_and(|handles| handles.drag.is_some())
    }

    fn checked_linked_path_handles(&self, actor: Actor, token: u64) -> Result<&Handles, String> {
        let preview = self.checked_linked_path(token)?;
        if preview.owner != actor {
            return Err("Review the other actor's linked path before using its handles".into());
        }
        let handles = preview
            .handles
            .as_ref()
            .ok_or("Choose Viewport handles first")?;
        if !handles.context.is_current(self) {
            return Err(
                "Scene, view or camera changed; cancel path handles and start again".into(),
            );
        }
        Ok(handles)
    }

    pub(super) fn linked_path_handles_report(&self) -> Option<Value> {
        let preview = self.geometry.linked_path.preview.as_ref()?;
        let handles = preview.handles.as_ref()?;
        let result = self.checked_linked_path_handles(preview.owner, preview.token);
        Some(
            json!({"row":handles.row,"dragging":handles.drag.is_some(),"valid":result.is_ok(),"error":result.err(),"width":handles.context.size.0,"height":handles.context.size.1,"snapping":self.spatial.snap,"grid":self.spatial.grid_size,"active":handles.drag.as_ref().map(|drag|drag.handle.name())}),
        )
    }

    pub fn linked_path_guide(&self) -> Option<Guide> {
        let preview = self.geometry.linked_path.preview.as_ref()?;
        let handles = self
            .checked_linked_path_handles(preview.owner, preview.token)
            .ok()?;
        Some(Guide {
            origin: preview.request.points[handles.row].as_vec3(),
            axes: [Vec3::X, Vec3::Y, Vec3::Z],
            mode: Mode::Move,
            active: handles.drag.as_ref().map(|drag| drag.handle),
        })
    }

    fn advance_linked_path_handles(&mut self) -> Value {
        self.geometry.linked_path.serial += 1;
        self.view_serial += 1;
        let mut handles = self
            .geometry
            .linked_path
            .preview
            .as_mut()
            .unwrap()
            .handles
            .take();
        if let Some(handles) = &mut handles {
            handles.context.refresh_scene(self);
        }
        let preview = self.geometry.linked_path.preview.as_mut().unwrap();
        preview.token = self.geometry.linked_path.serial;
        preview.handles = handles;
        self.linked_path()
    }

    pub fn begin_linked_path_handles(
        &mut self,
        actor: Actor,
        token: u64,
        row: usize,
        camera: Camera,
        size: (u32, u32),
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.linked_path_pick_stamp() {
            return Err("Path handle context changed; inspect the linked path again".into());
        }
        let preview = self.checked_linked_path(token)?;
        if preview.owner != actor {
            return Err("Review the other actor's linked path before using its handles".into());
        }
        if preview.picking.is_some() || preview.handles.is_some() {
            return Err("Finish or cancel the current path positioning gesture first".into());
        }
        if row >= preview.request.points.len() {
            return Err("Choose an existing zero-based path row".into());
        }
        if self.measurement_picking() || self.viewport_pick_conflict() {
            return Err("Finish the active viewport tool before starting path handles".into());
        }
        let context = Context::capture(self, camera, size, "Path handles")?;
        self.geometry.linked_path.preview.as_mut().unwrap().handles = Some(Handles {
            row,
            context,
            drag: None,
        });
        Ok(self.advance_linked_path_handles())
    }

    pub fn pick_linked_path_handle(&self, pointer: Pointer) -> Option<Handle> {
        let preview = self.geometry.linked_path.preview.as_ref()?;
        let handles = self
            .checked_linked_path_handles(preview.owner, preview.token)
            .ok()?;
        handles.context.ray(pointer).ok()?;
        render::handles::pick(
            &handles.context.camera(),
            handles.context.size,
            &self.linked_path_guide()?,
            pointer.position.as_vec2(),
        )
    }

    pub fn begin_linked_path_drag(
        &mut self,
        actor: Actor,
        token: u64,
        handle: Handle,
        pointer: Option<Pointer>,
    ) -> Result<Value, String> {
        let handles = self.checked_linked_path_handles(actor, token)?;
        if handles.drag.is_some() {
            return Err("Finish or cancel the current path handle drag first".into());
        }
        let origin = self.checked_linked_path(token)?.request.points[handles.row];
        if handle.axis().is_none() && handle.plane().is_none() && handle != Handle::View {
            return Err("Path nodes support move axes, planes and the view handle".into());
        }
        if !render::handles::shapes(
            &handles.context.camera(),
            handles.context.size,
            &self
                .linked_path_guide()
                .ok_or("Path handles are unavailable")?,
        )
        .iter()
        .any(|shape| shape.handle == handle)
        {
            return Err("Handle is not available in the captured view".into());
        }
        let initial = pointer
            .map(|pointer| parameter(&handles.context, handle, origin, pointer))
            .transpose()?
            .unwrap_or(DVec2::ZERO);
        let preview = self.checked_linked_path(token)?;
        let drag = Drag {
            handle,
            origin,
            initial,
            pointer: pointer.is_some(),
            document: preview.document.preview_snapshot(),
            request: preview.request.clone(),
            existing: preview.existing.clone(),
            core: preview.core.clone(),
            last_pick: preview.last_pick,
        };
        self.geometry
            .linked_path
            .preview
            .as_mut()
            .unwrap()
            .handles
            .as_mut()
            .unwrap()
            .drag = Some(drag);
        Ok(self.advance_linked_path_handles())
    }

    fn install_linked_path_handle_position(
        &mut self,
        actor: Actor,
        token: u64,
        origin: DVec3,
    ) -> Result<Value, String> {
        let row = self.checked_linked_path_handles(actor, token)?.row;
        let (request, plan) = self.linked_path_point_plan(token, row, origin)?;
        let mut handles = self
            .geometry
            .linked_path
            .preview
            .as_mut()
            .unwrap()
            .handles
            .take();
        let result = self.install_linked_path(actor, request, plan);
        if let Some(handles) = &mut handles {
            handles.context.refresh_scene(self);
        }
        self.geometry.linked_path.preview.as_mut().unwrap().handles = handles;
        result?;
        Ok(self.linked_path())
    }

    pub fn update_linked_path_drag(
        &mut self,
        actor: Actor,
        token: u64,
        pointer: Option<Pointer>,
        amount: Option<DVec2>,
    ) -> Result<Value, String> {
        if pointer.is_some() == amount.is_some() {
            return Err("Provide pointer or numeric amount, exactly one".into());
        }
        let handles = self.checked_linked_path_handles(actor, token)?;
        let drag = handles
            .drag
            .as_ref()
            .ok_or("Begin a path handle drag first")?;
        let mut amount = if let Some(pointer) = pointer {
            if !drag.pointer {
                return Err("Begin with a pointer before sending pointer updates".into());
            }
            parameter(&handles.context, drag.handle, drag.origin, pointer)? - drag.initial
        } else {
            amount.unwrap()
        };
        if !amount.is_finite() {
            return Err("Path handle amount must be finite".into());
        }
        if drag.handle.axis().is_some() && amount.y != 0.0 {
            return Err("An axis handle accepts one movement amount".into());
        }
        if self.spatial.snap {
            amount = (amount / self.spatial.grid_size).round() * self.spatial.grid_size;
        }
        let delta = if let Some(axis) = drag.handle.axis() {
            DVec3::AXES[axis] * amount.x
        } else if let Some((u, v)) = drag.handle.plane() {
            DVec3::AXES[u] * amount.x + DVec3::AXES[v] * amount.y
        } else {
            handles.context.camera().right().as_dvec3() * amount.x
                + handles.context.camera().up().as_dvec3() * amount.y
        };
        self.install_linked_path_handle_position(actor, token, drag.origin + delta)
    }

    pub fn finish_linked_path_drag(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        if self
            .checked_linked_path_handles(actor, token)?
            .drag
            .is_none()
        {
            return Err("Begin a path handle drag first".into());
        }
        self.geometry.linked_path.preview.as_mut().unwrap().handles = None;
        Ok(self.advance_linked_path_handles())
    }

    pub fn cancel_linked_path_drag(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let preview = self
            .geometry
            .linked_path
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Linked path preview was closed or replaced")?;
        if preview.owner != actor {
            return Err(
                "Review the other actor's linked path before cancelling its handles".into(),
            );
        }
        if preview
            .handles
            .as_ref()
            .and_then(|handles| handles.drag.as_ref())
            .is_none()
        {
            return Err("No path handle drag is active".into());
        }
        let preview = self.geometry.linked_path.preview.as_mut().unwrap();
        let drag = preview.handles.as_mut().unwrap().drag.take().unwrap();
        preview.document = drag.document;
        preview.request = drag.request;
        preview.existing = drag.existing;
        preview.core = drag.core;
        preview.last_pick = drag.last_pick;
        self.scene_serial += 1;
        Ok(self.advance_linked_path_handles())
    }

    pub fn cancel_linked_path_handles(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        let preview = self
            .geometry
            .linked_path
            .preview
            .as_mut()
            .filter(|preview| preview.token == token)
            .ok_or("Linked path preview was closed or replaced")?;
        if preview.owner != actor {
            return Err(
                "Review the other actor's linked path before cancelling its handles".into(),
            );
        }
        if preview
            .handles
            .as_ref()
            .is_some_and(|handles| handles.drag.is_some())
        {
            return Err("Cancel the active path drag before closing its handles".into());
        }
        if preview.handles.take().is_none() {
            return Err("Path handles are not active".into());
        }
        Ok(self.advance_linked_path_handles())
    }
}
