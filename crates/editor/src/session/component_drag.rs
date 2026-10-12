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
        self.components.options
    }

    pub(super) fn refresh_component_origin(&mut self) {
        self.components.origin = None;
        if self.components.selection.is_empty() {
            return;
        }
        let hidden = self.hidden_tool_brushes();
        let mut mins = DVec3::splat(f64::INFINITY);
        let mut maxs = DVec3::splat(f64::NEG_INFINITY);
        let solids = self
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
        self.components.origin = Some((mins + maxs) * 0.5);
    }

    pub fn component_guide(&self) -> Option<Guide> {
        if !self.components.options.enabled || self.component_topology_preview() {
            return None;
        }
        let origin = self.components.origin?
            + self
                .components
                .preview
                .as_ref()
                .map_or(DVec3::ZERO, |p| p.delta);
        Some(Guide {
            origin: origin.as_vec3(),
            axes: [Vec3::X, Vec3::Y, Vec3::Z],
            mode: Mode::Move,
            active: self.components.drag.as_ref().map(|drag| drag.handle),
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
        self.components.drag.as_ref()
    }

    pub fn component_drag_report(&self) -> Option<Value> {
        self.components.drag.as_ref().map(|drag| {
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
        if options != self.components.options {
            self.clear_component_drag();
            self.components.options = options;
            self.components.serial += 1;
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
            .components
            .preview
            .as_ref()
            .map_or(DVec3::ZERO, |p| p.delta);
        let mut drag = Drag {
            token: self.components.serial + 1,
            handle,
            pointer,
            error: None,
            camera: self.camera,
            origin: self.components.origin.unwrap() + base,
            initial: DVec2::ZERO,
            base,
            snapshot: String::new(),
        };
        if let Some(pointer) = pointer {
            drag.initial = drag.parameter(pointer)?;
        }
        self.components.serial += 1;
        drag.snapshot = self.component_stamp();
        let token = drag.token;
        self.components.drag = Some(drag);
        self.view_serial += 1;
        Ok(token)
    }

    fn checked_component_drag(&self, token: u64) -> Result<&Drag, String> {
        let drag = self
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
        let mut drag = self.components.drag.take().unwrap();
        let result = self
            .component_drag_delta(&drag, pointer, amount)
            .and_then(|delta| {
                if delta == DVec3::ZERO {
                    self.components.preview = None;
                    self.components.serial += 1;
                    self.scene_serial += 1;
                    self.view_serial += 1;
                    return Ok(());
                }
                let options = self.components.options;
                self.preview_component_move(delta, false, options.texture_lock, &drag.snapshot)?;
                self.components.options = options;
                Ok(())
            });
        drag.error = result.as_ref().err().cloned();
        self.components.serial += 1;
        drag.snapshot = self.component_stamp();
        self.components.drag = Some(drag);
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
        if self.components.options.snap && self.spatial.snap {
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
        if let Some(preview) = &self.components.preview {
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
        if self.components.drag.take().is_some() {
            self.components.preview = None;
            self.components.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interaction::Interaction;
    use crate::session::{SelectMode, component_picking::Kind};
    use map_core::ObjectId;
    use render::Projection;

    fn fixture() -> Session {
        let mut session = Session::new(Err("no assets".into()));
        let ObjectId::Solid(solid) = session
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let shape = session.component_topology(&[solid]).unwrap().remove(0);
        let face = shape
            .faces
            .iter()
            .find(|face| {
                face.vertices
                    .iter()
                    .all(|i| shape.vertices[*i].position.z == 64.0)
            })
            .unwrap();
        session
            .set_component_picking(Some(Kind::Face), &[solid], &session.component_stamp())
            .unwrap();
        session
            .select_components(
                &[subobject::Component {
                    solid,
                    sides: vec![face.side],
                }],
                SelectMode::Replace,
                &session.component_stamp(),
            )
            .unwrap();
        session.document.mark_saved();
        session
    }

    fn begin(session: &mut Session, handle: Handle) -> u64 {
        session
            .begin_component_drag(handle, None, &session.component_stamp())
            .unwrap()
    }

    #[test]
    fn component_drags_use_total_offsets_keep_valid_geometry_and_atomic_history() {
        let mut session = fixture();
        let original = session.document.vmf().clone();
        let revision = session.document.revision();
        let scope = session.component_solids();
        session
            .set_component_move_options(
                Options {
                    texture_lock: true,
                    ..Options::default()
                },
                &session.component_stamp(),
            )
            .unwrap();
        let token = begin(&mut session, Handle::Z);
        session
            .update_component_drag(token, None, Some(DVec2::new(19.0, 0.0)))
            .unwrap();
        assert_eq!(
            session.components.preview.as_ref().unwrap().delta,
            DVec3::Z * 16.0
        );
        assert!(session.components.preview.as_ref().unwrap().texture_lock);
        session
            .update_component_drag(token, None, Some(DVec2::new(33.0, 0.0)))
            .unwrap();
        let valid = session.render_document().vmf().clone();
        assert_eq!(session.components.preview.as_ref().unwrap().delta.z, 32.0);
        assert!(
            session
                .update_component_drag(token, None, Some(DVec2::new(-64.0, 0.0)))
                .is_err()
        );
        assert_eq!(session.render_document().vmf(), &valid);
        assert!(session.commit_component_drag(Actor::Agent, token).is_err());
        assert_eq!(session.document.revision(), revision);
        session
            .update_component_drag(token, None, Some(DVec2::new(33.0, 0.0)))
            .unwrap();
        session.commit_component_drag(Actor::Agent, token).unwrap();
        assert_eq!(session.document.vmf(), &valid);
        assert_eq!(session.component_solids(), scope);
        assert!(session.component_picking().is_some());
        session.undo(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &original);
        session.redo(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &valid);
    }

    #[test]
    fn zero_cancel_replacement_and_camera_changes_cannot_commit_stale_drags() {
        let mut session = fixture();
        let original = session.document.vmf().clone();
        let revision = session.document.revision();
        let token = begin(&mut session, Handle::Z);
        session
            .update_component_drag(token, None, Some(DVec2::new(16.0, 0.0)))
            .unwrap();
        session
            .update_component_drag(token, None, Some(DVec2::ZERO))
            .unwrap();
        assert!(session.components.preview.is_none());
        assert_eq!(
            session.commit_component_drag(Actor::Agent, token).unwrap(),
            0
        );
        assert_eq!(session.document.revision(), revision);
        let token = begin(&mut session, Handle::Z);
        let newer = begin(&mut session, Handle::Z);
        assert!(session.cancel_component_drag(token).is_err());
        session
            .update_component_drag(newer, None, Some(DVec2::new(16.0, 0.0)))
            .unwrap();
        session.cancel_component_drag(newer).unwrap();
        assert_eq!(session.render_document().vmf(), &original);
        let token = begin(&mut session, Handle::Z);
        session
            .update_component_drag(token, None, Some(DVec2::new(16.0, 0.0)))
            .unwrap();
        let mut camera = session.camera();
        camera.position.x += 10.0;
        session.set_camera(camera);
        assert!(session.commit_component_drag(Actor::Agent, token).is_err());
        assert_eq!(session.render_document().vmf(), &original);
        let token = begin(&mut session, Handle::Z);
        session
            .preview_component_move(DVec3::Z * 8.0, false, false, &session.component_stamp())
            .unwrap();
        assert!(session.commit_component_drag(Actor::Agent, token).is_err());
        assert_eq!(session.components.preview.as_ref().unwrap().delta.z, 8.0);
        session
            .set_transform_tool(Actor::Agent, session.transform_tool())
            .unwrap();
        assert!(session.component_selection().is_empty());
        assert!(session.component_guide().is_none());
    }

    #[test]
    fn pointer_and_numeric_drags_match_in_every_projection_and_cancel_safely() {
        for projection in [
            Projection::Top,
            Projection::Front,
            Projection::Side,
            Projection::Perspective,
        ] {
            let mut user = fixture();
            let mut agent = fixture();
            agent.document = user.document.clone();
            let camera = if projection == Projection::Perspective {
                Camera::looking_at(Vec3::new(250.0, -300.0, 250.0), Vec3::new(32.0, 32.0, 64.0))
            } else {
                Camera::planar(projection, Vec3::new(32.0, 32.0, 64.0), 256.0)
            };
            user.set_camera(camera);
            agent.set_camera(camera);
            let handle = if projection == Projection::Top {
                Handle::X
            } else {
                Handle::Z
            };
            let guide = user.component_guide().unwrap();
            let shape = handles::shapes(&camera, (1000, 800), &guide)
                .into_iter()
                .find(|s| s.handle == handle)
                .unwrap();
            let point = (shape.segments[0][0] + shape.segments[0][1]) * 0.5;
            let pointer = Pointer {
                position: handles::project(&camera, point, (1000, 800))
                    .unwrap()
                    .as_dvec2(),
                size: (1000, 800),
            };
            let end = Pointer {
                position: handles::project(
                    &camera,
                    point + Vec3::AXES[handle.axis().unwrap()] * 16.0,
                    pointer.size,
                )
                .unwrap()
                .as_dvec2(),
                ..pointer
            };
            assert_eq!(user.pick_component_handle(pointer), Some(handle));
            let mut interaction = Interaction::default();
            assert!(interaction.down(&mut user, pointer, false).unwrap());
            interaction.moved(&mut user, end).unwrap();
            interaction.up(&mut user, end).unwrap();
            let token = begin(&mut agent, handle);
            agent
                .update_component_drag(token, None, Some(DVec2::new(16.0, 0.0)))
                .unwrap();
            agent.commit_component_drag(Actor::Agent, token).unwrap();
            assert_eq!(user.document.vmf(), agent.document.vmf());
            let before = user.document.vmf().clone();
            interaction.down(&mut user, end, false).unwrap();
            interaction.cancel(&mut user);
            interaction.up(&mut user, pointer).unwrap();
            assert_eq!(user.document.vmf(), &before);
        }
    }

    #[test]
    fn plane_handles_accumulate_from_numeric_preview_and_reject_resize_and_bad_inputs() {
        let mut session = fixture();
        session
            .preview_component_move(DVec3::Z * 8.0, false, false, &session.component_stamp())
            .unwrap();
        let token = begin(&mut session, Handle::XZ);
        session
            .update_component_drag(token, None, Some(DVec2::new(4.0, 12.0)))
            .unwrap();
        assert_eq!(
            session.components.preview.as_ref().unwrap().delta,
            DVec3::new(4.0, 0.0, 20.0)
        );
        assert!(
            session
                .update_component_drag(token, None, Some(DVec2::splat(f64::NAN)))
                .is_err()
        );
        session.cancel_component_drag(token).unwrap();
        session.set_camera(Camera::planar(Projection::Front, Vec3::splat(32.0), 256.0));
        let pointer = Pointer {
            position: DVec2::splat(300.0),
            size: (1000, 800),
        };
        let token = session
            .begin_component_drag(Handle::Z, Some(pointer), &session.component_stamp())
            .unwrap();
        assert!(
            session
                .update_component_drag(
                    token,
                    Some(Pointer {
                        size: (800, 800),
                        ..pointer
                    }),
                    None
                )
                .is_err()
        );
        assert!(session.commit_component_drag(Actor::Agent, token).is_err());
        session.cancel_component_drag(token).unwrap();
        assert!(
            session
                .begin_component_drag(Handle::Y, Some(pointer), &session.component_stamp())
                .is_err()
        );
        assert!(
            session
                .begin_component_drag(Handle::Uniform, None, &session.component_stamp())
                .is_err()
        );
    }
}
