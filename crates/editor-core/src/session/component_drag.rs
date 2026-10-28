use std::collections::BTreeSet;

use glam::{DVec2, DVec3, Vec3};
use map_core::subobject;
use render::Camera;
use render::handles::{self, Guide, Handle, Mode};
use serde::Serialize;
use serde_json::{Value, json};

use super::manipulation::{axis_parameter, plane_parameter};
use super::{Actor, Pointer, Session};

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Options {
    pub enabled: bool,
    pub snap: bool,
    pub texture_lock: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            enabled: true,
            snap: true,
            texture_lock: false,
        }
    }
}

pub struct Drag {
    pub token: u64,
    pub handle: Handle,
    pub pointer: Option<Pointer>,
    pub error: Option<String>,
    camera: Camera,
    origin: DVec3,
    initial: DVec2,
    base: DVec3,
    snapshot: String,
}

impl Drag {
    fn parameter(&self, pointer: Pointer) -> Result<DVec2, String> {
        pointer.validate()?;
        if self.pointer.is_none_or(|start| start.size != pointer.size) {
            return Err("Start with a pointer and keep the same viewport size".into());
        }
        if let Some(axis) = self.handle.axis() {
            return axis_parameter(pointer, self.camera, self.origin, DVec3::AXES[axis])
                .map(|amount| DVec2::new(amount, 0.0));
        }
        let (u, v) = self.axes();
        plane_parameter(pointer, self.camera, self.origin, u, v)
    }

    fn axes(&self) -> (DVec3, DVec3) {
        if let Some((a, b)) = self.handle.plane() {
            (DVec3::AXES[a], DVec3::AXES[b])
        } else {
            (self.camera.right().as_dvec3(), self.camera.up().as_dvec3())
        }
    }
}

impl Session {
    pub fn component_move_options(&self) -> Options {
        self.geometry.components.options
    }

    pub(super) fn refresh_component_origin(&mut self) {
        self.geometry.components.origin = None;
        if self.geometry.components.selection.is_empty() {
            return;
        }
        let hidden = self.hidden_tool_brushes();
        let mut mins = DVec3::splat(f64::INFINITY);
        let mut maxs = DVec3::splat(f64::NEG_INFINITY);
        let solids = self
            .geometry
            .components
            .selection
            .iter()
            .map(|c| c.solid)
            .collect::<BTreeSet<_>>();
        for solid in solids {
            if self.component_hidden(solid, &hidden) {
                return;
            }
            let Ok((_, brush)) = subobject::solid(&self.document, solid) else {
                return;
            };
            let Ok(shape) = subobject::topology(brush) else {
                return;
            };
            for component in self
                .geometry
                .components
                .selection
                .iter()
                .filter(|c| c.solid == solid)
            {
                let Ok(vertices) = shape.resolve(&component.sides) else {
                    return;
                };
                for index in vertices {
                    let point = shape.vertices[index].position;
                    mins = mins.min(point);
                    maxs = maxs.max(point);
                }
            }
        }
        self.geometry.components.origin = Some((mins + maxs) * 0.5);
    }

    pub fn component_guide(&self) -> Option<Guide> {
        if self
            .component_selection()
            .iter()
            .any(|component| !self.working_solid_visible(self.render_document(), component.solid))
        {
            return None;
        }
        if !self.geometry.components.options.enabled
            || self.component_topology_preview()
            || self
                .geometry
                .components
                .preview
                .as_ref()
                .is_some_and(|preview| preview.transform.is_some())
        {
            return None;
        }
        let origin = self.geometry.components.origin?
            + self
                .geometry
                .components
                .preview
                .as_ref()
                .map_or(DVec3::ZERO, |p| p.delta);
        Some(Guide {
            origin: origin.as_vec3(),
            axes: [Vec3::X, Vec3::Y, Vec3::Z],
            mode: Mode::Move,
            active: self
                .geometry
                .components
                .drag
                .as_ref()
                .map(|drag| drag.handle),
        })
    }

    pub fn pick_component_handle(&self, pointer: Pointer) -> Option<Handle> {
        handles::pick(
            &self.camera,
            pointer.size,
            &self.component_guide()?,
            pointer.position.as_vec2(),
        )
    }

    pub fn component_drag(&self) -> Option<&Drag> {
        self.geometry.components.drag.as_ref()
    }

    pub fn component_drag_report(&self) -> Option<Value> {
        self.geometry.components.drag.as_ref().map(|drag| {
            json!({
                "token":drag.token.to_string(), "handle":drag.handle.name(), "error":drag.error,
            })
        })
    }

    pub fn set_component_move_options(
        &mut self,
        options: Options,
        snapshot: &str,
    ) -> Result<(), String> {
        self.check_component_stamp(snapshot)?;
        if options != self.geometry.components.options {
            self.clear_component_drag();
            self.geometry.components.options = options;
            self.geometry.components.serial += 1;
            self.view_serial += 1;
        }
        Ok(())
    }

    pub fn begin_component_drag(
        &mut self,
        handle: Handle,
        pointer: Option<Pointer>,
        snapshot: &str,
    ) -> Result<u64, String> {
        self.check_component_stamp(snapshot)?;
        let guide = self
            .component_guide()
            .ok_or("Select visible brush components and enable move handles")?;
        if handle == Handle::Uniform {
            return Err("Component handles support movement along x/y/z, xy/xz/yz or view".into());
        }
        let pointer = pointer.map(Pointer::validate).transpose()?;
        if pointer.is_some_and(|pointer| {
            !handles::shapes(&self.camera, pointer.size, &guide)
                .iter()
                .any(|s| s.handle == handle)
        }) {
            return Err("Handle is not available in this view".into());
        }
        let base = self
            .geometry
            .components
            .preview
            .as_ref()
            .map_or(DVec3::ZERO, |p| p.delta);
        let mut drag = Drag {
            token: self.geometry.components.serial + 1,
            handle,
            pointer,
            error: None,
            camera: self.camera,
            origin: self.geometry.components.origin.unwrap() + base,
            initial: DVec2::ZERO,
            base,
            snapshot: String::new(),
        };
        if let Some(pointer) = pointer {
            drag.initial = drag.parameter(pointer)?;
        }
        self.geometry.components.serial += 1;
        drag.snapshot = self.component_stamp();
        let token = drag.token;
        self.geometry.components.drag = Some(drag);
        self.view_serial += 1;
        Ok(token)
    }

    fn checked_component_drag(&self, token: u64) -> Result<&Drag, String> {
        let drag = self
            .geometry
            .components
            .drag
            .as_ref()
            .filter(|d| d.token == token)
            .ok_or("Component drag was closed or replaced")?;
        self.check_component_stamp(&drag.snapshot)?;
        if self.camera != drag.camera {
            return Err("Camera changed; start the component drag again".into());
        }
        Ok(drag)
    }

    pub fn update_component_drag(
        &mut self,
        token: u64,
        pointer: Option<Pointer>,
        amount: Option<DVec2>,
    ) -> Result<(), String> {
        self.checked_component_drag(token)?;
        let mut drag = self.geometry.components.drag.take().unwrap();
        let result = self
            .component_drag_delta(&drag, pointer, amount)
            .and_then(|delta| {
                if delta == DVec3::ZERO {
                    self.geometry.components.preview = None;
                    self.geometry.components.serial += 1;
                    self.scene_serial += 1;
                    self.view_serial += 1;
                    return Ok(());
                }
                let options = self.geometry.components.options;
                self.preview_component_move(delta, false, options.texture_lock, &drag.snapshot)?;
                self.geometry.components.options = options;
                Ok(())
            });
        drag.error = result.as_ref().err().cloned();
        self.geometry.components.serial += 1;
        drag.snapshot = self.component_stamp();
        self.geometry.components.drag = Some(drag);
        self.view_serial += 1;
        result
    }

    fn component_drag_delta(
        &self,
        drag: &Drag,
        pointer: Option<Pointer>,
        amount: Option<DVec2>,
    ) -> Result<DVec3, String> {
        if pointer.is_some() == amount.is_some() {
            return Err("Provide pointer or total handle offset, exactly one".into());
        }
        let mut value = if let Some(pointer) = pointer {
            drag.parameter(pointer)? - drag.initial
        } else {
            amount.unwrap()
        };
        if !value.is_finite() || (drag.handle.axis().is_some() && value.y != 0.0) {
            return Err("Use finite offsets; axis handles accept only the first offset".into());
        }
        if self.geometry.components.options.snap && self.spatial.snap {
            value = (value / self.spatial.grid_size).round() * self.spatial.grid_size;
        }
        let offset = if let Some(axis) = drag.handle.axis() {
            DVec3::AXES[axis] * value.x
        } else {
            let (u, v) = drag.axes();
            u * value.x + v * value.y
        };
        Ok(drag.base + offset)
    }

    pub fn commit_component_drag(&mut self, actor: Actor, token: u64) -> Result<usize, String> {
        let drag = self.checked_component_drag(token)?;
        if let Some(error) = &drag.error {
            return Err(error.clone());
        }
        if let Some(preview) = &self.geometry.components.preview {
            self.commit_component_move(actor, preview.token)
        } else {
            self.clear_component_drag();
            Ok(0)
        }
    }

    pub fn cancel_component_drag(&mut self, token: u64) -> Result<(), String> {
        self.checked_component_drag(token)?;
        self.clear_component_drag();
        Ok(())
    }

    pub(super) fn clear_component_drag(&mut self) {
        if self.geometry.components.drag.take().is_some() {
            self.geometry.components.preview = None;
            self.geometry.components.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }
}
