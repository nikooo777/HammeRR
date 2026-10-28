use render::Camera;

use crate::session::{Actor, Pointer, SelectMode, Session};

const DRAG_THRESHOLD: f64 = 4.0;

struct ComponentClick {
    start: Pointer,
    camera: Camera,
    snapshot: String,
    view_serial: u64,
    control: bool,
}

struct PartitionClick {
    start: Pointer,
    camera: Camera,
    token: u64,
    view_serial: u64,
    control: bool,
}

struct ClipPointClick {
    start: Pointer,
    camera: Camera,
    stamp: String,
    view_serial: u64,
}

struct AreaDrag {
    filters: Option<u64>,
    start: Pointer,
    camera: Camera,
    mode: SelectMode,
    epoch: u64,
    generation: u64,
    revision: u64,
}

#[derive(Default)]
pub struct Interaction {
    clip_drag: Option<u64>,
    clip_point: Option<ClipPointClick>,
    component: Option<ComponentClick>,
    partition: Option<PartitionClick>,
    component_drag: Option<u64>,
    area: Option<AreaDrag>,
    handle_epoch: Option<u64>,
    paint_token: Option<u64>,
}

impl Interaction {
    pub fn down(
        &mut self,
        session: &mut Session,
        pointer: Pointer,
        control: bool,
    ) -> Result<bool, String> {
        pointer.validate()?;
        if let Some(token) = session.partition_preview_token() {
            *self = Self::default();
            self.partition = Some(PartitionClick {
                start: pointer,
                camera: session.camera(),
                token,
                view_serial: session.view_serial(),
                control,
            });
            return Ok(true);
        }
        if let Some(handle) = session.pick_clip_handle(pointer) {
            *self = Self::default();
            self.clip_drag = Some(session.begin_clip_drag(
                handle,
                Some(pointer),
                &session.clip_point_stamp(),
            )?);
            return Ok(true);
        }
        if session.clip_points().is_some() {
            *self = Self::default();
            self.clip_point = Some(ClipPointClick {
                start: pointer,
                camera: session.camera(),
                stamp: session.clip_point_stamp(),
                view_serial: session.view_serial(),
            });
            return Ok(true);
        }
        if !control && let Some(handle) = session.pick_component_handle(pointer) {
            self.component_drag = Some(session.begin_component_drag(
                handle,
                Some(pointer),
                &session.component_stamp(),
            )?);
            self.component = None;
            self.area = None;
            self.handle_epoch = None;
            self.paint_token = None;
            return Ok(true);
        }
        if session.component_picking().is_some() {
            self.area = None;
            self.handle_epoch = None;
            self.paint_token = None;
            self.component = Some(ComponentClick {
                start: pointer,
                camera: session.camera(),
                snapshot: session.component_stamp(),
                view_serial: session.view_serial(),
                control,
            });
            return Ok(true);
        }
        if session.paint_enabled() {
            let spec = crate::session::paint::Spec {
                camera: session.camera(),
                pointer,
                radius: session.paint_radius(),
                mode: if control {
                    SelectMode::Toggle
                } else {
                    session.area_mode()
                },
                faces: session.face_mode(),
            };
            let operation = session.paint_operation();
            let token = if operation == crate::session::paint::Operation::Selection {
                session.begin_paint_selection(spec)?
            } else {
                session.begin_texture_stroke(spec, operation)?
            };
            self.paint_token = Some(token);
            self.area = None;
            self.handle_epoch = None;
            return Ok(true);
        }
        if let Some(handle) = session.pick_transform_handle(pointer) {
            session.begin_transform_drag(handle, Some(pointer))?;
            self.handle_epoch = Some(session.interaction_epoch());
            self.area = None;
            return Ok(true);
        }
        session.cancel_area_selection();
        self.area = Some(AreaDrag {
            filters: (!session.face_mode()).then(|| session.selection_filter_revision()),
            start: pointer,
            camera: session.camera(),
            mode: if control {
                SelectMode::Toggle
            } else {
                session.area_mode()
            },
            epoch: session.interaction_epoch(),
            generation: session.document_generation(),
            revision: session.document().revision(),
        });
        Ok(false)
    }

    pub fn moved(&mut self, session: &mut Session, pointer: Pointer) -> Result<bool, String> {
        if let Some(token) = self.clip_drag {
            if session.clip_drag().is_some_and(|d| d.token == token) {
                session.update_clip_drag(token, Some(pointer), None)?;
            }
            return Ok(true);
        }
        if self.clip_point.is_some() {
            return Ok(true);
        }
        if let Some(token) = self.component_drag {
            if session
                .component_drag()
                .is_some_and(|drag| drag.token == token)
            {
                session.update_component_drag(token, Some(pointer), None)?;
            }
            return Ok(true);
        }
        if self.component.is_some() || self.partition.is_some() {
            return Ok(true);
        }
        if let Some(token) = self.paint_token {
            let Some(stroke) = session
                .paint_stroke()
                .filter(|stroke| stroke.token == token)
            else {
                return Ok(true);
            };
            if stroke.spec.pointer.size != pointer.size {
                session.cancel_paint_selection(token)?;
                return Err("Viewport resized; start a new paint stroke".into());
            }
            session.update_active_stroke(token, &[pointer.position])?;
            return Ok(true);
        }
        if let Some(epoch) = self.handle_epoch {
            if epoch != session.interaction_epoch() {
                self.handle_epoch = None;
                return Ok(true);
            }
            session.update_transform_drag(Some(pointer), None)?;
            return Ok(true);
        }
        let Some(drag) = &self.area else {
            return Ok(false);
        };
        if !Self::area_current(session, drag, pointer) {
            return Ok(false);
        }
        if (pointer.position - drag.start.position).length() < DRAG_THRESHOLD {
            return Ok(false);
        }
        session.preview_area_selection(
            Actor::User,
            drag.camera,
            drag.start,
            pointer.position,
            session.area_rule(),
            drag.mode,
        )?;
        Ok(true)
    }

    pub fn up(&mut self, session: &mut Session, pointer: Pointer) -> Result<bool, String> {
        if let Some(click) = self.partition.take() {
            if session.partition_preview_token() == Some(click.token)
                && click.camera == session.camera()
                && click.view_serial == session.view_serial()
                && click.start.size == pointer.size
                && pointer.validate().is_ok()
                && pointer.position.distance(click.start.position) < DRAG_THRESHOLD
            {
                let picks =
                    session.pick_component_partitions(click.token, click.camera, pointer)?;
                let partitions = picks
                    .into_iter()
                    .take(1)
                    .map(|pick| pick.partition)
                    .collect::<Vec<_>>();
                session.select_component_partitions(
                    click.token,
                    &partitions,
                    if click.control {
                        SelectMode::Toggle
                    } else {
                        SelectMode::Replace
                    },
                )?;
            }
            return Ok(true);
        }
        if let Some(token) = self.clip_drag.take() {
            if session.clip_drag().is_some_and(|d| d.token == token) {
                session.update_clip_drag(token, Some(pointer), None)?;
                session.finish_clip_drag(token)?;
            }
            return Ok(true);
        }
        if let Some(click) = self.clip_point.take() {
            if click.stamp == session.clip_point_stamp()
                && click.camera == session.camera()
                && click.view_serial == session.view_serial()
                && click.start.size == pointer.size
                && pointer.validate().is_ok()
                && pointer.position.distance(click.start.position) < DRAG_THRESHOLD
            {
                session.pick_clip_point(Actor::User, click.camera, pointer, &click.stamp)?;
            }
            return Ok(true);
        }
        if let Some(token) = self.component_drag.take() {
            if session
                .component_drag()
                .is_some_and(|drag| drag.token == token)
            {
                session.update_component_drag(token, Some(pointer), None)?;
                session.commit_component_drag(Actor::User, token)?;
            }
            return Ok(true);
        }
        if let Some(click) = self.component.take() {
            if click.snapshot != session.component_stamp()
                || click.camera != session.camera()
                || click.view_serial != session.view_serial()
                || click.start.size != pointer.size
                || pointer.validate().is_err()
                || pointer.position.distance(click.start.position) >= DRAG_THRESHOLD
            {
                return Ok(true);
            }
            let Some(picking) = session.component_picking().cloned() else {
                return Ok(true);
            };
            let picks = session.pick_components(
                click.camera,
                pointer,
                picking.kind,
                &picking.solids.iter().copied().collect::<Vec<_>>(),
                crate::session::component_picking::PICK_RADIUS,
            )?;
            let components = picks
                .candidates
                .into_iter()
                .take(1)
                .map(|candidate| candidate.component)
                .collect::<Vec<_>>();
            session.select_components(
                &components,
                if click.control {
                    SelectMode::Toggle
                } else {
                    SelectMode::Replace
                },
                &click.snapshot,
            )?;
            return Ok(true);
        }
        if let Some(token) = self.paint_token.take() {
            let Some(stroke) = session
                .paint_stroke()
                .filter(|stroke| stroke.token == token)
            else {
                return Ok(true);
            };
            if stroke.spec.pointer.size != pointer.size {
                session.cancel_paint_selection(token)?;
                return Err("Viewport resized; start a new paint stroke".into());
            }
            session.update_active_stroke(token, &[pointer.position])?;
            session.commit_active_stroke(Actor::User, token)?;
            return Ok(true);
        }
        if let Some(epoch) = self.handle_epoch.take() {
            if epoch != session.interaction_epoch() {
                return Ok(true);
            }
            let moved = session
                .transform_drag()
                .and_then(|drag| drag.pointer)
                .is_some_and(|start| {
                    (pointer.position - start.position).length() >= DRAG_THRESHOLD
                });
            if session.transform_preview().is_some() || moved {
                session.update_transform_drag(Some(pointer), None)?;
                session.commit_transform_preview(Actor::User)?;
            } else {
                session.cancel_transform_preview();
            }
            return Ok(true);
        }
        let Some(drag) = self.area.take() else {
            return Ok(false);
        };
        if !Self::area_current(session, &drag, pointer) {
            return Ok(true);
        }
        if (pointer.position - drag.start.position).length() < DRAG_THRESHOLD {
            return Ok(false);
        }
        session.preview_area_selection(
            Actor::User,
            drag.camera,
            drag.start,
            pointer.position,
            session.area_rule(),
            drag.mode,
        )?;
        session.commit_area_selection()?;
        Ok(true)
    }

    fn area_current(session: &Session, drag: &AreaDrag, pointer: Pointer) -> bool {
        drag.filters
            .is_none_or(|revision| revision == session.selection_filter_revision())
            && drag.epoch == session.interaction_epoch()
            && drag.generation == session.document_generation()
            && drag.revision == session.document().revision()
            && drag.camera == session.camera()
            && drag.start.size == pointer.size
    }

    pub fn cancel(&mut self, session: &mut Session) {
        if let Some(token) = self.clip_drag.take() {
            let _ = session.cancel_clip_drag(token);
        }
        self.clip_point = None;
        if let Some(token) = self.component_drag.take() {
            let _ = session.cancel_component_drag(token);
        }
        self.component = None;
        self.partition = None;
        self.area = None;
        self.handle_epoch = None;
        if let Some(token) = self.paint_token.take() {
            let _ = session.cancel_paint_selection(token);
            return;
        }
        session.cancel_transform_preview();
        session.cancel_area_selection();
    }

    pub fn active(&self) -> bool {
        self.clip_drag.is_some()
            || self.component.is_some()
            || self.partition.is_some()
            || self.clip_point.is_some()
            || self.component_drag.is_some()
            || self.area.is_some()
            || self.handle_epoch.is_some()
            || self.paint_token.is_some()
    }
}
