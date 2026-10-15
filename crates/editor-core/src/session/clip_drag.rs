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

#[cfg(test)]
mod tests {
    use super::*;
    use map_core::clip::Keep;
    use render::Projection;

    fn fixture() -> Session {
        let mut s = Session::new(Err("no assets needed".into()));
        let id = s
            .create_block(
                Actor::User,
                DVec3::splat(-64.0),
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        s.document.mark_saved();
        s.clip_brushes(
            Actor::User,
            &[id],
            Plane {
                normal: DVec3::X,
                dist: 0.0,
            },
            Keep::Both,
            Some("tools/toolsnodraw".into()),
            true,
        )
        .unwrap();
        s
    }

    fn begin(s: &mut Session, handle: Handle) -> u64 {
        s.begin_clip_drag(handle, None, &s.clip_point_stamp())
            .unwrap()
    }

    fn update(s: &mut Session, token: u64, amount: f64) {
        s.update_clip_drag(token, None, Some(DVec2::new(amount, 0.0)))
            .unwrap();
    }

    fn mode(s: &mut Session, mode: Mode) {
        s.set_clip_handle_options(
            Options {
                mode: Some(mode),
                ..s.clip_handles
            },
            &s.clip_point_stamp(),
        )
        .unwrap();
    }

    #[test]
    fn positioning_keeps_history_unchanged_until_apply_and_undo_restores_source() {
        let mut s = fixture();
        let original = s.document.vmf().clone();
        let revision = s.document.revision();
        let history = s.document.undo_labels().count();
        let token = begin(&mut s, Handle::X);
        update(&mut s, token, 19.0);
        assert_eq!(s.clip_preview().unwrap().plane.dist, 16.0);
        assert!(s.commit_clip_preview(Actor::Agent).is_err());
        s.finish_clip_drag(token).unwrap();
        assert_eq!(s.document.revision(), revision);
        assert_eq!(s.document.vmf(), &original);
        assert_eq!(s.document.undo_labels().count(), history);
        assert!(!s.document.is_dirty());
        let plane = s.clip_preview().unwrap().plane;
        mode(&mut s, Mode::Rotate);
        let token = begin(&mut s, Handle::Z);
        update(&mut s, token, 31.0);
        let rotated = s.clip_preview().unwrap();
        let normal = DMat3::from_rotation_z(30_f64.to_radians()) * DVec3::X;
        assert!(rotated.plane.normal.abs_diff_eq(normal, 1e-10));
        assert!((rotated.plane.dist - normal.x * plane.dist).abs() < 1e-10);
        assert_eq!(rotated.keep, Keep::Both);
        assert_eq!(rotated.material.as_deref(), Some("tools/toolsnodraw"));
        s.finish_clip_drag(token).unwrap();
        let preview = s.clip_preview().unwrap().document.vmf().clone();
        s.commit_clip_preview(Actor::Agent).unwrap();
        assert_eq!(s.document.vmf(), &preview);
        assert_eq!(s.document.undo_labels().count(), history + 1);
        s.undo(Actor::Agent).unwrap();
        assert_eq!(s.document.vmf(), &original);
        s.redo(Actor::Agent).unwrap();
        assert_eq!(s.document.vmf(), &preview);
    }

    #[test]
    fn cancel_restores_points_and_plane_and_settings_do_not_resnap_transformed_points() {
        let mut s = fixture();
        let points = clip_points::Draft {
            points: vec![
                DVec3::new(0.0, -64.0, -64.0),
                DVec3::new(0.0, 64.0, -64.0),
                DVec3::new(0.0, 64.0, 64.0),
            ],
            keep: Keep::Back,
            material: None,
            snap: true,
        };
        s.set_clip_points(Actor::User, points.clone(), &s.clip_point_stamp())
            .unwrap();
        let before = s.clip_preview().unwrap().plane;
        mode(&mut s, Mode::Rotate);
        let token = begin(&mut s, Handle::Z);
        update(&mut s, token, 30.0);
        let rotated = s.clip_preview().unwrap().plane;
        for point in &s.clip_points().unwrap().points {
            assert!((rotated.normal.dot(*point) - rotated.dist).abs() < 1e-9);
        }
        s.cancel_clip_drag(token).unwrap();
        assert_eq!(s.clip_points().unwrap().points, points.points);
        assert_eq!(s.clip_preview().unwrap().plane, before);
        let token = begin(&mut s, Handle::Z);
        update(&mut s, token, 30.0);
        s.finish_clip_drag(token).unwrap();
        let transformed = s.clip_points().unwrap().points.clone();
        let mut settings = s.clip_points().unwrap().clone();
        settings.keep = Keep::Both;
        settings.material = Some("tools/toolsnodraw".into());
        s.set_clip_points(Actor::User, settings, &s.clip_point_stamp())
            .unwrap();
        assert_eq!(s.clip_points().unwrap().points, transformed);
        assert!(
            s.clip_preview()
                .unwrap()
                .plane
                .normal
                .abs_diff_eq(rotated.normal, 1e-10)
        );
    }

    #[test]
    fn invalid_updates_retain_valid_draft_and_stale_tokens_cannot_touch_new_work() {
        let mut s = fixture();
        let stamp = s.clip_point_stamp();
        let token = begin(&mut s, Handle::X);
        assert!(s.begin_clip_drag(Handle::X, None, &stamp).is_err());
        update(&mut s, token, 16.0);
        for amount in [
            DVec2::new(f64::NAN, 0.0),
            DVec2::new(1.0, 1.0),
            DVec2::new(70000.0, 0.0),
        ] {
            assert!(s.update_clip_drag(token, None, Some(amount)).is_err());
            assert_eq!(s.clip_preview().unwrap().plane.dist, 16.0);
            assert!(s.finish_clip_drag(token).is_err());
        }
        update(&mut s, token, 32.0);
        s.cancel_clip_drag(token).unwrap();
        let next = begin(&mut s, Handle::X);
        assert_ne!(token, next);
        assert!(s.update_clip_drag(token, None, Some(DVec2::ZERO)).is_err());
        assert!(s.finish_clip_drag(token).is_err());
        assert!(s.cancel_clip_drag(token).is_err());
        assert_eq!(s.clip_drag().unwrap().token, next);
        s.finish_clip_drag(next).unwrap();
        assert_eq!(s.clip_preview().unwrap().plane.dist, 0.0);
        assert!(
            s.set_clip_handle_options(
                Options {
                    angle_step: 0.0,
                    ..s.clip_handles
                },
                &s.clip_point_stamp()
            )
            .is_err()
        );
        assert!(
            s.set_clip_handle_options(
                Options {
                    mode: Some(Mode::Scale),
                    ..s.clip_handles
                },
                &s.clip_point_stamp()
            )
            .is_err()
        );
    }

    #[test]
    fn camera_view_options_and_map_changes_do_not_resurrect_a_drag() {
        let mut s = fixture();
        for action in 0..3 {
            let token = begin(&mut s, Handle::X);
            update(&mut s, token, 16.0);
            match action {
                0 => {
                    let mut camera = s.camera();
                    camera.position.x += 2.0;
                    s.set_camera(camera);
                }
                1 => s.set_view(s.view()),
                _ => mode(&mut s, Mode::Rotate),
            }
            assert!(s.clip_drag().is_none());
            assert_eq!(s.clip_preview().unwrap().plane.dist, 0.0);
            assert!(s.cancel_clip_drag(token).is_err());
        }
        let token = begin(&mut s, Handle::Z);
        s.select(&[], super::super::SelectMode::Replace).unwrap();
        assert!(s.clip_preview().is_none());
        assert!(s.cancel_clip_drag(token).is_err());
    }

    #[test]
    fn replacing_a_plane_invalidates_snapshots_and_loading_a_map_discards_the_drag() {
        let mut s = fixture();
        let snapshot = s.clip_point_stamp();
        let token = begin(&mut s, Handle::X);
        let ids = s.selected_ids();
        s.clip_brushes(
            Actor::Agent,
            &ids,
            Plane {
                normal: DVec3::Y,
                dist: 16.0,
            },
            Keep::Front,
            None,
            true,
        )
        .unwrap();
        assert!(s.begin_clip_drag(Handle::X, None, &snapshot).is_err());
        assert!(s.cancel_clip_drag(token).is_err());
        assert_eq!(s.clip_preview().unwrap().plane.normal, DVec3::Y);
        let token = begin(&mut s, Handle::Y);
        s.new_map(Actor::Agent, true).unwrap();
        assert!(s.clip_preview().is_none());
        assert!(s.cancel_clip_drag(token).is_err());
        assert!(
            s.begin_clip_drag(Handle::X, None, &s.clip_point_stamp())
                .is_err()
        );
    }

    fn pointer(camera: Camera, size: (u32, u32), world: Vec3) -> Pointer {
        Pointer {
            position: handles::project(&camera, world, size).unwrap().as_dvec2(),
            size,
        }
    }

    #[test]
    fn viewport_release_keeps_the_cut_preview_in_perspective_and_each_planar_view() {
        for projection in [
            Projection::Perspective,
            Projection::Top,
            Projection::Front,
            Projection::Side,
        ] {
            let mut s = fixture();
            let ids = s.selected_ids();
            let normal = DVec3::ONE.normalize();
            s.clip_brushes(
                Actor::User,
                &ids,
                Plane { normal, dist: 0.0 },
                Keep::Both,
                None,
                true,
            )
            .unwrap();
            s.set_projection(Actor::User, projection);
            s.frame_selection(Actor::User, 900.0 / 700.0).unwrap();
            let camera = s.camera();
            let guide = s.clip_guide().unwrap();
            let size = (900, 700);
            let shapes = handles::shapes(&camera, size, &guide);
            let shape = shapes
                .iter()
                .find(|shape| shape.handle.axis().is_some())
                .unwrap();
            let axis = shape.handle.axis().unwrap();
            let segment = shape.segments[0];
            let world = (segment[0] + segment[1]) * 0.5;
            let start = pointer(camera, size, world);
            assert_eq!(s.pick_clip_handle(start), Some(shape.handle));
            let end = pointer(camera, size, world + Vec3::AXES[axis] * 32.0);
            let original = s.document.vmf().clone();
            let mut interaction = crate::interaction::Interaction::default();
            assert!(interaction.down(&mut s, start, false).unwrap());
            assert!(interaction.moved(&mut s, end).unwrap());
            assert!(interaction.up(&mut s, end).unwrap());
            assert!(!interaction.active());
            assert!(s.clip_drag().is_none());
            assert!((s.clip_preview().unwrap().plane.dist - normal[axis] * 32.0).abs() < 1e-8);
            assert_eq!(s.document.vmf(), &original);
        }
    }

    #[test]
    fn pointer_rotation_matches_numeric_rotation_and_resize_blocks_finish() {
        let mut s = fixture();
        s.set_projection(Actor::User, Projection::Top);
        mode(&mut s, Mode::Rotate);
        let guide = s.clip_guide().unwrap();
        let camera = s.camera();
        let size = (900, 700);
        let shapes = handles::shapes(&camera, size, &guide);
        let shape = shapes
            .iter()
            .find(|shape| shape.handle == Handle::Z)
            .unwrap();
        let world = shape.segments[4][0];
        let start = pointer(camera, size, world);
        let end = pointer(
            camera,
            size,
            (DMat3::from_rotation_z(30_f64.to_radians()) * world.as_dvec3()).as_vec3(),
        );
        let token = s
            .begin_clip_drag(Handle::Z, Some(start), &s.clip_point_stamp())
            .unwrap();
        s.update_clip_drag(token, Some(end), None).unwrap();
        let result = s.clip_preview().unwrap().plane;
        let resized = Pointer {
            size: (901, 700),
            ..end
        };
        assert!(s.update_clip_drag(token, Some(resized), None).is_err());
        assert!(s.finish_clip_drag(token).is_err());
        s.cancel_clip_drag(token).unwrap();
        let token = begin(&mut s, Handle::Z);
        update(&mut s, token, 30.0);
        assert_eq!(result, s.clip_preview().unwrap().plane);
    }
}
