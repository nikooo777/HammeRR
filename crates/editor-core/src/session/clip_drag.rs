use glam::{DMat3, DVec2, DVec3, Vec3};
use map_core::Plane;
use render::Camera;
use render::handles::{self, Guide, Handle, Mode};
use serde_json::{Value, json};

use super::clipping::ClipPreview;
use super::manipulation::{axis_parameter, plane_parameter};
use super::{Actor, Pointer, Session, clip_points};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub mode: Option<Mode>,
    pub snap: bool,
    pub angle_step: f64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mode: Some(Mode::Move),
            snap: true,
            angle_step: 15.0,
        }
    }
}

pub struct Drag {
    pub token: u64,
    pub handle: Handle,
    pub error: Option<String>,
    mode: Mode,
    camera: Camera,
    pointer: Option<Pointer>,
    pivot: DVec3,
    origin: DVec3,
    initial: DVec2,
    last_angle: f64,
    angle: f64,
    before: ClipPreview,
    points: Option<clip_points::Draft>,
    snapshot: String,
}

impl Drag {
    fn axes(&self) -> (DVec3, DVec3) {
        if let Some((a, b)) = self.handle.plane() {
            (DVec3::AXES[a], DVec3::AXES[b])
        } else {
            (self.camera.right().as_dvec3(), self.camera.up().as_dvec3())
        }
    }

    fn parameter(&self, pointer: Pointer) -> Result<DVec2, String> {
        pointer.validate()?;
        if self.pointer.is_none_or(|start| start.size != pointer.size) {
            return Err("Start with a pointer and keep the same viewport size".into());
        }
        if let Some(axis) = self.handle.axis() {
            if self.mode == Mode::Rotate {
                let p = plane_parameter(
                    pointer,
                    self.camera,
                    self.pivot,
                    DVec3::AXES[(axis + 1) % 3],
                    DVec3::AXES[(axis + 2) % 3],
                )?;
                if p.length_squared() < 1e-8 {
                    return Err("Drag away from the rotation center".into());
                }
                return Ok(DVec2::new(p.y.atan2(p.x).to_degrees(), 0.0));
            }
            return axis_parameter(pointer, self.camera, self.pivot, DVec3::AXES[axis])
                .map(|p| DVec2::new(p, 0.0));
        }
        let (u, v) = self.axes();
        plane_parameter(pointer, self.camera, self.pivot, u, v)
    }
}

impl Session {
    pub fn clip_handle_options(&self) -> Options {
        self.clip_handles
    }

    pub fn clip_handles_report(&self) -> Value {
        json!({"mode":match self.clip_handles.mode {
            Some(Mode::Move)=>"move", Some(Mode::Rotate)=>"rotate", _=>"off"
        }, "snap":self.clip_handles.snap, "angle_step":self.clip_handles.angle_step})
    }

    pub fn clip_drag(&self) -> Option<&Drag> {
        self.clip_drag.as_ref()
    }

    pub fn clip_drag_report(&self) -> Option<Value> {
        self.clip_drag.as_ref().map(|d| {
            json!({"token":d.token.to_string(),
            "handle":d.handle.name(), "error":d.error})
        })
    }

    fn check_clip_stamp(&self, snapshot: &str) -> Result<(), String> {
        if snapshot != self.clip_point_stamp() {
            return Err("Cut, view or map changed; inspect the clipping state again".into());
        }
        Ok(())
    }

    pub fn set_clip_handle_options(
        &mut self,
        options: Options,
        snapshot: &str,
    ) -> Result<(), String> {
        self.check_clip_stamp(snapshot)?;
        if options.mode == Some(Mode::Scale)
            || !options.angle_step.is_finite()
            || !(0.1..=180.0).contains(&options.angle_step)
        {
            return Err(
                "Choose move, rotate or off and an angle step from 0.1 to 180 degrees".into(),
            );
        }
        if options != self.clip_handles {
            self.restore_clip_drag();
            self.clip_handles = options;
            self.clip_epoch += 1;
            self.view_serial += 1;
        }
        Ok(())
    }

    pub fn clip_guide(&self) -> Option<Guide> {
        if !self.working_selection_visible() {
            return None;
        }
        let mode = self.clip_handles.mode?;
        let preview = self.geometry.clip_preview.as_ref()?;
        let center = (preview.bounds[0] + preview.bounds[1]) * 0.5;
        let origin = self.clip_drag.as_ref().map_or_else(
            || {
                center
                    - preview.plane.normal * (preview.plane.normal.dot(center) - preview.plane.dist)
            },
            |d| d.origin,
        );
        Some(Guide {
            origin: origin.as_vec3(),
            axes: [Vec3::X, Vec3::Y, Vec3::Z],
            mode,
            active: self.clip_drag.as_ref().map(|d| d.handle),
        })
    }

    pub fn pick_clip_handle(&self, pointer: Pointer) -> Option<Handle> {
        handles::pick(
            &self.camera,
            pointer.size,
            &self.clip_guide()?,
            pointer.position.as_vec2(),
        )
    }

    pub fn begin_clip_drag(
        &mut self,
        handle: Handle,
        pointer: Option<Pointer>,
        snapshot: &str,
    ) -> Result<u64, String> {
        self.check_clip_stamp(snapshot)?;
        if self.clip_drag.is_some() {
            return Err("Finish or cancel the current cut-plane drag first".into());
        }
        let guide = self
            .clip_guide()
            .ok_or("Preview a cut and enable plane handles first")?;
        if handle == Handle::Uniform || (guide.mode == Mode::Rotate && handle.axis().is_none()) {
            return Err("Move uses x/y/z, xy/xz/yz or view; rotation uses x/y/z rings".into());
        }
        let pointer = pointer.map(Pointer::validate).transpose()?;
        if pointer.is_some_and(|p| {
            !handles::shapes(&self.camera, p.size, &guide)
                .iter()
                .any(|s| s.handle == handle)
        }) {
            return Err("Handle is not available in this view".into());
        }
        let mut drag = Drag {
            token: self.clip_epoch + 1,
            handle,
            error: None,
            mode: guide.mode,
            camera: self.camera,
            pointer,
            pivot: guide.origin.as_dvec3(),
            origin: guide.origin.as_dvec3(),
            initial: DVec2::ZERO,
            last_angle: 0.0,
            angle: 0.0,
            before: self.geometry.clip_preview.as_ref().unwrap().clone(),
            points: self.clip_points.clone(),
            snapshot: String::new(),
        };
        if let Some(p) = pointer {
            drag.initial = drag.parameter(p)?;
        }
        drag.last_angle = drag.initial.x;
        self.clip_epoch += 1;
        drag.snapshot = self.clip_point_stamp();
        let token = drag.token;
        self.clip_drag = Some(drag);
        self.view_serial += 1;
        Ok(token)
    }

    fn checked_clip_drag(&self, token: u64) -> Result<&Drag, String> {
        let drag = self
            .clip_drag
            .as_ref()
            .filter(|d| d.token == token)
            .ok_or("Cut-plane drag was closed or replaced")?;
        self.check_clip_stamp(&drag.snapshot)?;
        if self.camera != drag.camera {
            return Err("Camera changed; start the cut-plane drag again".into());
        }
        Ok(drag)
    }

    fn clip_drag_pose(
        &self,
        drag: &mut Drag,
        pointer: Option<Pointer>,
        amount: Option<DVec2>,
    ) -> Result<(DMat3, DVec3), String> {
        let mut value = match (pointer, amount) {
            (Some(p), None) => {
                let current = drag.parameter(p)?;
                if drag.mode == Mode::Rotate {
                    drag.angle += (current.x - drag.last_angle + 180.0).rem_euclid(360.0) - 180.0;
                    drag.last_angle = current.x;
                    DVec2::new(drag.angle, 0.0)
                } else {
                    current - drag.initial
                }
            }
            (None, Some(amount)) => amount,
            _ => return Err("Provide either a pointer or a total amount pair".into()),
        };
        if !value.is_finite()
            || value.abs().max_element() > 65536.0
            || (drag.handle.axis().is_some() && value.y != 0.0)
        {
            return Err(
                "Use finite amounts within ±65536; axis handles require a zero second amount"
                    .into(),
            );
        }
        if self.clip_handles.snap {
            let step = if drag.mode == Mode::Rotate {
                self.clip_handles.angle_step
            } else if self.spatial.snap {
                self.spatial.grid_size
            } else {
                0.0
            };
            if step > 0.0 {
                value = (value / step).round() * step;
            }
        }
        if drag.mode == Mode::Rotate {
            return Ok((
                DMat3::from_axis_angle(
                    DVec3::AXES[drag.handle.axis().unwrap()],
                    value.x.to_radians(),
                ),
                DVec3::ZERO,
            ));
        }
        let delta = if let Some(axis) = drag.handle.axis() {
            DVec3::AXES[axis] * value.x
        } else {
            let (u, v) = drag.axes();
            u * value.x + v * value.y
        };
        Ok((DMat3::IDENTITY, delta))
    }

    pub fn update_clip_drag(
        &mut self,
        token: u64,
        pointer: Option<Pointer>,
        amount: Option<DVec2>,
    ) -> Result<(), String> {
        self.checked_clip_drag(token)?;
        let mut drag = self.clip_drag.take().unwrap();
        let result =
            self.clip_drag_pose(&mut drag, pointer, amount)
                .and_then(|(rotation, delta)| {
                    let offset = drag.pivot - rotation * drag.pivot + delta;
                    let normal = rotation * drag.before.plane.normal;
                    let plane = Plane {
                        normal,
                        dist: drag.before.plane.dist + normal.dot(offset),
                    };
                    if !plane.dist.is_finite()
                        || plane.dist.abs() > 65536.0
                        || (drag.pivot + delta).abs().max_element() > 65536.0
                    {
                        return Err("Keep the cut plane within ±65536 world units".into());
                    }
                    let mut points = drag.points.clone();
                    if let Some(draft) = &mut points {
                        for p in &mut draft.points {
                            *p = rotation * *p + offset;
                            if !p.is_finite() || p.abs().max_element() > 65536.0 {
                                return Err("Keep the cut points within ±65536 world units".into());
                            }
                        }
                    }
                    self.clip_brushes(
                        Actor::User,
                        &drag.before.ids,
                        plane,
                        drag.before.keep,
                        drag.before.material.clone(),
                        true,
                    )?;
                    self.clip_points = points;
                    drag.origin = drag.pivot + delta;
                    self.scene_serial += 1;
                    Ok(())
                });
        drag.error = result.as_ref().err().cloned();
        self.clip_epoch += 1;
        drag.snapshot = self.clip_point_stamp();
        self.clip_drag = Some(drag);
        self.view_serial += 1;
        result
    }

    pub fn finish_clip_drag(&mut self, token: u64) -> Result<(), String> {
        if let Some(error) = &self.checked_clip_drag(token)?.error {
            return Err(format!("Fix or cancel the cut-plane drag: {error}"));
        }
        self.clip_drag = None;
        self.clip_epoch += 1;
        self.view_serial += 1;
        Ok(())
    }

    pub fn cancel_clip_drag(&mut self, token: u64) -> Result<(), String> {
        self.checked_clip_drag(token)?;
        self.restore_clip_drag();
        Ok(())
    }

    pub(super) fn restore_clip_drag(&mut self) {
        if let Some(drag) = self.clip_drag.take() {
            self.geometry.clip_preview = Some(drag.before);
            self.clip_points = drag.points;
            self.clip_epoch += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }
}
