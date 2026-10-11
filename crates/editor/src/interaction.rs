use render::Camera;

use crate::session::{Actor, Pointer, SelectMode, Session};

const DRAG_THRESHOLD: f64 = 4.0;

struct AreaDrag {
    start: Pointer,
    camera: Camera,
    mode: SelectMode,
    epoch: u64,
    generation: u64,
    revision: u64,
}

#[derive(Default)]
pub struct Interaction {
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
        drag.epoch == session.interaction_epoch()
            && drag.generation == session.document_generation()
            && drag.revision == session.document().revision()
            && drag.camera == session.camera()
            && drag.start.size == pointer.size
    }

    pub fn cancel(&mut self, session: &mut Session) {
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
        self.area.is_some() || self.handle_epoch.is_some() || self.paint_token.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::TransformTool;
    use glam::{DVec2, DVec3, Vec3};
    use map_core::region::Match;
    use render::Projection;
    use render::handles::{self, Handle, Mode};

    fn fixture(projection: Projection, mode: Option<Mode>) -> Session {
        let mut session = Session::new(Err("no assets needed".into()));
        session
            .create_block(
                Actor::User,
                DVec3::new(-32.0, -32.0, 0.0),
                DVec3::new(32.0, 32.0, 64.0),
                "brick/test",
                false,
            )
            .unwrap();
        let center = Vec3::new(0.0, 0.0, 32.0);
        session.set_camera(if projection == Projection::Perspective {
            Camera::looking_at(Vec3::new(-400.0, -400.0, 350.0), center)
        } else {
            Camera::planar(projection, center, 512.0)
        });
        session
            .set_transform_tool(
                Actor::User,
                TransformTool {
                    mode,
                    ..Default::default()
                },
            )
            .unwrap();
        session
    }

    #[test]
    fn paint_gestures_share_strokes_and_preserve_replacements_on_cancel_or_release() {
        for projection in [
            Projection::Top,
            Projection::Front,
            Projection::Side,
            Projection::Perspective,
        ] {
            let mut session = fixture(projection, None);
            let expected = session.selected_ids();
            session.select(&[], SelectMode::Replace).unwrap();
            session.set_paint_tool(Actor::User, true, 12.0).unwrap();
            let pointer = Pointer {
                position: DVec2::new(400.0, 300.0),
                size: (800, 600),
            };
            let mut interaction = Interaction::default();
            interaction.down(&mut session, pointer, false).unwrap();
            assert!(session.selected_ids().is_empty());
            let token = session.paint_stroke().unwrap().token;
            session
                .update_paint_selection(token, &[pointer.position])
                .unwrap();
            interaction.up(&mut session, pointer).unwrap();
            assert_eq!(session.selected_ids(), expected);
            interaction.down(&mut session, pointer, true).unwrap();
            interaction.moved(&mut session, pointer).unwrap();
            interaction.up(&mut session, pointer).unwrap();
            assert!(session.selected_ids().is_empty());
            for cancel in [true, false] {
                interaction.down(&mut session, pointer, false).unwrap();
                let spec = session.paint_stroke().unwrap().spec;
                let replacement = session.begin_paint_selection(spec).unwrap();
                interaction.moved(&mut session, pointer).unwrap();
                if cancel {
                    interaction.cancel(&mut session);
                } else {
                    interaction.up(&mut session, pointer).unwrap();
                }
                assert_eq!(session.paint_stroke().unwrap().token, replacement);
                session.cancel_paint_selection(replacement).unwrap();
            }
            interaction.down(&mut session, pointer, false).unwrap();
            assert!(
                interaction
                    .moved(
                        &mut session,
                        Pointer {
                            size: (801, 600),
                            ..pointer
                        }
                    )
                    .is_err()
            );
            assert!(session.paint_stroke().is_none());
            assert!(session.selected_ids().is_empty());
            assert_eq!(session.document().undo_labels().count(), 1);
        }
    }

    #[test]
    fn texture_gestures_commit_the_shared_preview_in_every_projection() {
        for projection in [
            Projection::Top,
            Projection::Front,
            Projection::Side,
            Projection::Perspective,
        ] {
            let mut session = fixture(projection, None);
            let selection = session.selected_ids();
            let center = session.camera().right().as_dvec3() * 300.0;
            let map_core::ObjectId::Solid(source) = session
                .create_block(
                    Actor::Agent,
                    center - DVec3::splat(16.0),
                    center + DVec3::splat(16.0),
                    "metal/sample",
                    false,
                )
                .unwrap()
            else {
                unreachable!()
            };
            let face = map_core::surface::FaceId {
                solid: source,
                side: session.document().solid(source).unwrap().sides[0].id,
            };
            session.sample_face_texture(face).unwrap();
            session.select(&selection, SelectMode::Replace).unwrap();
            session
                .set_paint_operation(
                    Actor::User,
                    true,
                    6.0,
                    crate::session::paint::Operation::Material,
                )
                .unwrap();
            let pointer = Pointer {
                position: DVec2::new(400.0, 300.0),
                size: (800, 600),
            };
            let history = session.document().undo_labels().count();
            let mut interaction = Interaction::default();
            interaction.down(&mut session, pointer, false).unwrap();
            interaction.moved(&mut session, pointer).unwrap();
            let preview = session.render_document().vmf().clone();
            interaction.up(&mut session, pointer).unwrap();
            assert_eq!(session.document().vmf(), &preview);
            assert_eq!(session.selected_ids(), selection);
            assert_eq!(session.document().undo_labels().count(), history + 1);
        }
    }

    #[test]
    fn pointer_drags_and_numeric_mcp_amounts_produce_identical_transforms_in_every_view() {
        let size = (1200, 800);
        for projection in [
            Projection::Top,
            Projection::Front,
            Projection::Side,
            Projection::Perspective,
        ] {
            for mode in [Mode::Move, Mode::Rotate, Mode::Scale] {
                let mut user = fixture(projection, Some(mode));
                let mut agent = fixture(projection, Some(mode));
                let editor = user.document().vmf().world.solids[0].editor.clone();
                let map_core::ObjectId::Solid(id) = agent.selected_ids()[0] else {
                    unreachable!()
                };
                agent
                    .edit(Actor::Agent, "fixture metadata", |tx| {
                        tx.solid_mut(id)?.editor = editor;
                        Ok(())
                    })
                    .unwrap();
                let guide = user.transform_guide().unwrap();
                let axis = if mode == Mode::Rotate {
                    projection.axes().map_or(2, |axes| axes.2)
                } else {
                    projection.axes().map_or(0, |axes| axes.0)
                };
                let handle = [Handle::X, Handle::Y, Handle::Z][axis];
                let span = handles::length(&user.camera(), guide.origin, size.1);
                let (start, end, amount) = if mode == Mode::Rotate {
                    let angle = 0.3_f32;
                    let turn = if projection == Projection::Perspective {
                        60.0_f32
                    } else {
                        90.0_f32
                    };
                    let u = guide.axes[(axis + 1) % 3] * span;
                    let v = guide.axes[(axis + 2) % 3] * span;
                    (
                        guide.origin + u * angle.cos() + v * angle.sin(),
                        guide.origin
                            + u * (angle + turn.to_radians()).cos()
                            + v * (angle + turn.to_radians()).sin(),
                        f64::from(turn),
                    )
                } else {
                    let start = guide.origin
                        + guide.axes[axis] * span * if mode == Mode::Scale { 1.0 } else { 0.6 };
                    (
                        start,
                        start + guide.axes[axis] * if mode == Mode::Scale { span } else { 32.0 },
                        if mode == Mode::Scale { 2.0 } else { 32.0 },
                    )
                };
                let pointer = Pointer {
                    position: handles::project(&user.camera(), start, size)
                        .unwrap()
                        .as_dvec2(),
                    size,
                };
                let end = Pointer {
                    position: handles::project(&user.camera(), end, size)
                        .unwrap()
                        .as_dvec2(),
                    size,
                };
                assert_eq!(
                    user.pick_transform_handle(pointer),
                    Some(handle),
                    "{projection:?} {mode:?}"
                );
                let original = user.document().vmf().clone();
                let mut interaction = Interaction::default();
                assert!(interaction.down(&mut user, pointer, false).unwrap());
                if mode != Mode::Scale {
                    assert!(interaction.moved(&mut user, end).unwrap());
                }
                assert_eq!(user.document().vmf(), &original);
                assert!(interaction.up(&mut user, end).unwrap());
                agent.begin_transform_drag(handle, None).unwrap();
                agent
                    .update_transform_drag(None, Some(DVec2::new(amount, 0.0)))
                    .unwrap();
                agent.commit_transform_preview(Actor::Agent).unwrap();
                assert_eq!(
                    user.document().vmf(),
                    agent.document().vmf(),
                    "{projection:?} {mode:?}"
                );
                assert_eq!(user.document().undo_labels().count(), 2);
            }
        }
    }

    #[test]
    fn cancellation_and_agent_replacement_prevent_stale_pointer_releases() {
        let mut session = fixture(Projection::Top, Some(Mode::Move));
        let original = session.document().vmf().clone();
        let pointer = Pointer {
            position: DVec2::new(660.0, 400.0),
            size: (1200, 800),
        };
        let mut interaction = Interaction::default();
        assert!(interaction.down(&mut session, pointer, false).unwrap());
        session.cancel_transform_preview();
        assert!(
            interaction
                .up(
                    &mut session,
                    Pointer {
                        position: pointer.position + DVec2::X * 50.0,
                        ..pointer
                    }
                )
                .unwrap()
        );
        assert_eq!(session.document().vmf(), &original);
        assert!(interaction.down(&mut session, pointer, false).unwrap());
        let end = Pointer {
            position: pointer.position + DVec2::X * 50.0,
            ..pointer
        };
        interaction.moved(&mut session, end).unwrap();
        let preview = session.transform_preview().unwrap();
        let (ids, transform) = (preview.ids.clone(), preview.transform);
        session
            .preview_transform(Actor::Agent, &ids, transform, Some(transform.pivot))
            .unwrap();
        assert!(session.transform_drag().is_none());
        assert!(interaction.up(&mut session, end).unwrap());
        assert_eq!(session.document().vmf(), &original);
        assert!(session.transform_preview().is_some());
        session.cancel_transform_preview();
        session
            .set_transform_tool(Actor::Agent, TransformTool::default())
            .unwrap();
        let start = Pointer {
            position: DVec2::new(400.0, 250.0),
            ..pointer
        };
        let end = Pointer {
            position: DVec2::new(800.0, 550.0),
            ..pointer
        };
        assert!(!interaction.down(&mut session, start, false).unwrap());
        interaction.moved(&mut session, end).unwrap();
        session
            .preview_area_selection(
                Actor::Agent,
                session.camera(),
                start,
                end.position,
                Match::Contained,
                SelectMode::Toggle,
            )
            .unwrap();
        assert!(interaction.up(&mut session, end).unwrap());
        assert!(session.area_preview().is_some());
        assert_eq!(session.document().vmf(), &original);
        session.cancel_area_selection();
        interaction.down(&mut session, start, false).unwrap();
        session.set_camera(Camera::planar(Projection::Front, Vec3::ZERO, 512.0));
        assert!(interaction.up(&mut session, end).unwrap());
        assert!(session.area_preview().is_none());
    }

    #[test]
    fn rectangle_gestures_preview_then_commit_without_map_edits_and_clicks_stay_clicks() {
        let mut session = fixture(Projection::Top, None);
        let id = session.selected_ids()[0];
        session.select(&[], SelectMode::Replace).unwrap();
        let original = session.document().vmf().clone();
        let pointer = Pointer {
            position: DVec2::new(400.0, 250.0),
            size: (1200, 800),
        };
        let end = Pointer {
            position: DVec2::new(800.0, 550.0),
            ..pointer
        };
        let mut interaction = Interaction::default();
        assert!(!interaction.down(&mut session, pointer, false).unwrap());
        assert!(interaction.moved(&mut session, end).unwrap());
        assert!(session.selected_ids().is_empty());
        assert_eq!(session.area_preview().unwrap().matches.objects, vec![id]);
        interaction.up(&mut session, end).unwrap();
        assert_eq!(session.selected_ids(), vec![id]);
        assert_eq!(session.document().vmf(), &original);
        assert_eq!(session.document().undo_labels().count(), 1);
        interaction.down(&mut session, pointer, false).unwrap();
        assert!(
            !interaction
                .up(
                    &mut session,
                    Pointer {
                        position: pointer.position + DVec2::splat(1.0),
                        ..pointer
                    }
                )
                .unwrap()
        );
        interaction.down(&mut session, pointer, true).unwrap();
        interaction.moved(&mut session, end).unwrap();
        interaction.up(&mut session, end).unwrap();
        assert!(session.selected_ids().is_empty());
    }
}
