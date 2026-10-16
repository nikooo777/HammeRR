use glam::{DVec2, DVec3, Vec3};
use map_core::surface::FaceId;
use render::{
    Camera,
    handles::{self, Guide, Handle, Mode},
};
use serde_json::{Value, json};

use super::manipulation::{axis_parameter, plane_parameter};
use super::{Actor, Pointer, Session, Snapshot};

#[cfg(test)]
mod tests;

#[derive(Default)]
pub struct State {
    serial: u64,
    active: Option<Move>,
}

struct Move {
    token: u64,
    snapshot: Snapshot,
    faces: Vec<FaceId>,
    mask: u64,
    origin: DVec3,
    offset: DVec3,
    changed: usize,
    preview: Option<u64>,
    snap: bool,
    grid: f64,
    tool_textures: bool,
    drag: Option<Drag>,
    error: Option<String>,
}

struct Drag {
    handle: Handle,
    camera: Camera,
    pointer: Option<Pointer>,
    initial: DVec2,
}

impl Drag {
    fn axes(&self) -> (DVec3, DVec3) {
        if let Some((a, b)) = self.handle.plane() {
            (DVec3::AXES[a], DVec3::AXES[b])
        } else {
            (self.camera.right().as_dvec3(), self.camera.up().as_dvec3())
        }
    }

    fn parameter(&self, pointer: Pointer, origin: DVec3) -> Result<DVec2, String> {
        pointer.validate()?;
        if self.pointer.is_none_or(|start| start.size != pointer.size) {
            return Err("Keep the starting terrain handle viewport size".into());
        }
        if let Some(axis) = self.handle.axis() {
            axis_parameter(pointer, self.camera, origin, DVec3::AXES[axis])
                .map(|amount| DVec2::new(amount, 0.0))
        } else {
            let (u, v) = self.axes();
            plane_parameter(pointer, self.camera, origin, u, v)
        }
    }
}

impl Session {
    pub(super) fn working_region_cancel_terrain_drag(&mut self) {
        if let Some(active) = &mut self.terrain_handles.active {
            active.drag = None;
        }
    }
    fn checked_terrain_move(&self, token: u64) -> Result<&Move, String> {
        let active = self
            .terrain_handles
            .active
            .as_ref()
            .filter(|active| active.token == token)
            .ok_or("Terrain handles were closed or replaced")?;
        if !self.current_snapshot(&active.snapshot) {
            return Err("Map changed; start terrain handles again".into());
        }
        if self
            .terrain_triangle_highlight
            .as_ref()
            .is_none_or(|highlight| {
                highlight.token != active.mask
                    || highlight.generation != active.snapshot.generation
                    || highlight.revision != active.snapshot.revision
            })
        {
            return Err("Terrain vertex mask was replaced or became stale".into());
        }
        if self.face_preview_token() != active.preview
            || !std::ptr::eq(
                self.render_document(),
                self.face_preview_document().unwrap_or(&self.document),
            )
        {
            return Err("Terrain handle preview was replaced".into());
        }
        if self.view.tool_textures != active.tool_textures
            || active
                .faces
                .iter()
                .any(|face| self.component_hidden(face.solid, &Default::default()))
        {
            return Err("Terrain visibility changed; start terrain handles again".into());
        }
        if active
            .drag
            .as_ref()
            .is_some_and(|drag| self.camera != drag.camera)
            || self.spatial.grid_size != active.grid
            || self.spatial.snap != active.snap
        {
            return Err("Camera or grid settings changed; start terrain handles again".into());
        }
        Ok(active)
    }

    pub fn check_terrain_move_token(&self, token: u64) -> Result<(), String> {
        self.checked_terrain_move(token).map(|_| ())
    }

    pub fn terrain_move_offset(&self, token: u64) -> Result<DVec3, String> {
        self.checked_terrain_move(token).map(|active| active.offset)
    }

    pub fn enable_terrain_move_handles(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        mask: u64,
    ) -> Result<u64, String> {
        if self.terrain_handles.active.is_some()
            || !std::ptr::eq(self.render_document(), &self.document)
        {
            return Err("Apply or cancel the active preview or terrain handles first".into());
        }
        let mask_selection = self.terrain_vertex_mask(faces, Some(mask))?;
        let hidden = self.hidden_terrain_tool_faces();
        let faces = mask_selection
            .faces
            .iter()
            .map(|face| face.face)
            .collect::<Vec<_>>();
        if faces.iter().any(|face| {
            hidden.contains(face) || self.component_hidden(face.solid, &Default::default())
        }) {
            return Err("Select visible terrain vertices for handles".into());
        }
        let grids = self.terrain_surface_grids(&faces, false)?;
        let mut mins = DVec3::splat(f64::INFINITY);
        let mut maxs = DVec3::splat(f64::NEG_INFINITY);
        for grid in &grids {
            let selected = mask_selection
                .faces
                .iter()
                .find(|face| face.face == grid.face)
                .unwrap();
            for &id in &selected.vertices {
                mins = mins.min(grid.mesh.positions[id]);
                maxs = maxs.max(grid.mesh.positions[id]);
            }
        }
        self.terrain_handles.serial = self.terrain_handles.serial.wrapping_add(1);
        let token = self.terrain_handles.serial;
        self.terrain_handles.active = Some(Move {
            token,
            snapshot: Snapshot {
                generation: self.document_generation(),
                revision: self.document.revision(),
            },
            faces,
            mask,
            origin: (mins + maxs) * 0.5,
            offset: DVec3::ZERO,
            changed: 0,
            preview: None,
            snap: self.spatial.snap,
            grid: self.spatial.grid_size,
            tool_textures: self.view.tool_textures,
            drag: None,
            error: None,
        });
        self.view_serial += 1;
        self.log(actor, "enabled terrain vertex move handles");
        Ok(token)
    }

    pub fn terrain_move_guide(&self) -> Option<Guide> {
        let active = self.terrain_handles.active.as_ref()?;
        self.checked_terrain_move(active.token).ok()?;
        Some(Guide {
            origin: (active.origin + active.offset).as_vec3(),
            axes: [Vec3::X, Vec3::Y, Vec3::Z],
            mode: Mode::Move,
            active: active.drag.as_ref().map(|drag| drag.handle),
        })
    }

    pub fn pick_terrain_move_handle(&self, pointer: Pointer) -> Option<Handle> {
        handles::pick(
            &self.camera,
            pointer.size,
            &self.terrain_move_guide()?,
            pointer.position.as_vec2(),
        )
    }

    pub fn begin_terrain_move_drag(
        &mut self,
        token: u64,
        handle: Handle,
        pointer: Option<Pointer>,
    ) -> Result<(), String> {
        let active = self.checked_terrain_move(token)?;
        if active.drag.is_some() || handle == Handle::Uniform {
            return Err("Start one terrain drag using x/y/z, xy/xz/yz or view".into());
        }
        let pointer = pointer.map(Pointer::validate).transpose()?;
        let guide = self.terrain_move_guide().unwrap();
        if pointer.is_some_and(|pointer| {
            !handles::shapes(&self.camera, pointer.size, &guide)
                .iter()
                .any(|shape| shape.handle == handle)
        }) {
            return Err("Terrain handle is unavailable in this view".into());
        }
        let mut drag = Drag {
            handle,
            camera: self.camera,
            pointer,
            initial: DVec2::ZERO,
        };
        if let Some(pointer) = pointer {
            drag.initial = drag.parameter(pointer, active.origin)?;
        }
        self.terrain_handles.active.as_mut().unwrap().drag = Some(drag);
        self.view_serial += 1;
        Ok(())
    }

    pub fn update_terrain_move_drag(
        &mut self,
        actor: Actor,
        token: u64,
        pointer: Option<Pointer>,
        amount: Option<DVec2>,
    ) -> Result<usize, String> {
        let active = self.checked_terrain_move(token)?;
        let drag = active
            .drag
            .as_ref()
            .ok_or("Begin a terrain handle drag first")?;
        let result = (|| {
            if pointer.is_some() == amount.is_some() {
                return Err("Provide a pointer or total handle offset, exactly one".into());
            }
            let mut value = if let Some(pointer) = pointer {
                drag.parameter(pointer, active.origin)? - drag.initial
            } else {
                amount.unwrap()
            };
            if !value.is_finite() || (drag.handle.axis().is_some() && value.y != 0.0) {
                return Err("Use finite offsets; axis handles accept only the first amount".into());
            }
            if active.snap {
                value = (value / active.grid).round() * active.grid;
            }
            let offset = if let Some(axis) = drag.handle.axis() {
                DVec3::AXES[axis] * value.x
            } else {
                let (u, v) = drag.axes();
                u * value.x + v * value.y
            };
            Ok(offset)
        })();
        let faces = active.faces.clone();
        let mask = active.mask;
        let current = active.preview.map(|_| (active.offset, active.changed));
        let result = result.and_then(|offset| {
            if let Some((previous, changed)) = current
                && offset == previous
            {
                return Ok(changed);
            }
            let changed = self.preview_terrain_vertex_move(actor, &faces, offset, mask)?;
            let preview = self.face_preview_token();
            let active = self.terrain_handles.active.as_mut().unwrap();
            active.offset = offset;
            active.changed = changed;
            active.preview = preview;
            Ok(changed)
        });
        self.terrain_handles.active.as_mut().unwrap().error = result.as_ref().err().cloned();
        self.view_serial += 1;
        result
    }

    pub fn finish_terrain_move_drag(&mut self, token: u64) -> Result<u64, String> {
        let active = self.checked_terrain_move(token)?;
        if let Some(error) = &active.error {
            return Err(error.clone());
        }
        let preview = active
            .preview
            .ok_or("Move a terrain handle before finishing")?;
        self.terrain_handles.active = None;
        self.view_serial += 1;
        Ok(preview)
    }

    pub fn cancel_terrain_move_handles(&mut self, token: u64) -> Result<(), String> {
        let active = self
            .terrain_handles
            .active
            .as_ref()
            .filter(|active| active.token == token)
            .ok_or("Terrain handles were closed or replaced")?;
        if active.preview.is_some() && self.face_preview_token() == active.preview {
            self.cancel_face_preview();
        }
        self.terrain_handles.active = None;
        self.view_serial += 1;
        Ok(())
    }

    pub fn terrain_move_handles_json(&self) -> Value {
        self.terrain_handles.active.as_ref().map_or_else(|| json!({"active":false,"token":null}), |active| {
            json!({"active":true,"token":active.token.to_string(),"stale":self.checked_terrain_move(active.token).is_err(),"mask_token":active.mask.to_string(),"origin":active.origin.to_array(),"offset":active.offset.to_array(),"preview_token":active.preview.map(|token|token.to_string()),"dragging":active.drag.is_some(),"handle":active.drag.as_ref().map(|drag|drag.handle.name()),"snap":active.snap,"grid_size":active.grid,"error":active.error})
        })
    }
}
