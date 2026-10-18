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
    fn object_filter_revisions_consume_old_clicks_and_marquees_but_preserve_face_clicks() {
        let mut session = fixture(Projection::Top, None);
        let pointer = Pointer {
            position: DVec2::new(320.0, 240.0),
            size: (640, 480),
        };
        let before = session.selected_ids();
        let mut interaction = Interaction::default();
        assert!(!interaction.down(&mut session, pointer, false).unwrap());
        let serials = (
            session.scene_serial(),
            session.view_serial(),
            session.interaction_epoch(),
        );
        let flags = session.selection_filters();
        session
            .set_selection_filters(
                Actor::Agent,
                crate::session::selection_filters::SelectionFilters {
                    world_brushes: false,
                    ..flags
                },
            )
            .unwrap();
        session.set_selection_filters(Actor::User, flags).unwrap();
        assert_eq!(
            (
                session.scene_serial(),
                session.view_serial(),
                session.interaction_epoch()
            ),
            serials
        );
        assert!(
            !interaction
                .moved(
                    &mut session,
                    Pointer {
                        position: pointer.position + DVec2::splat(20.0),
                        ..pointer
                    }
                )
                .unwrap()
        );
        assert!(interaction.up(&mut session, pointer).unwrap());
        assert_eq!(session.selected_ids(), before);
        assert!(session.area_preview().is_none());
        session.set_face_mode(true);
        assert!(!interaction.down(&mut session, pointer, false).unwrap());
        session
            .set_selection_filters(
                Actor::Agent,
                crate::session::selection_filters::SelectionFilters {
                    point_entities: false,
                    ..flags
                },
            )
            .unwrap();
        assert!(!interaction.up(&mut session, pointer).unwrap());
    }

    #[test]
    fn temporary_visibility_consumes_old_clicks_and_area_releases_after_hide_and_show() {
        let mut session = fixture(Projection::Top, None);
        let id = session.selected_ids()[0];
        let pointer = Pointer {
            position: DVec2::new(320.0, 240.0),
            size: (640, 480),
        };
        let mut interaction = Interaction::default();
        assert!(!interaction.down(&mut session, pointer, false).unwrap());
        session
            .set_editor_visibility(
                Actor::Agent,
                crate::session::editor_visibility::Action::Hide(vec![id]),
                &session.working_region_stamp(),
            )
            .unwrap();
        session
            .set_editor_visibility(
                Actor::User,
                crate::session::editor_visibility::Action::ShowAll,
                &session.working_region_stamp(),
            )
            .unwrap();
        assert!(interaction.up(&mut session, pointer).unwrap());
        assert_eq!(session.selected_ids(), [id]);
        let empty = Pointer {
            position: DVec2::new(20.0, 20.0),
            ..pointer
        };
        assert!(!interaction.down(&mut session, empty, false).unwrap());
        interaction
            .moved(
                &mut session,
                Pointer {
                    position: empty.position + DVec2::splat(40.0),
                    ..empty
                },
            )
            .unwrap();
        session
            .set_editor_visibility(
                Actor::Agent,
                crate::session::editor_visibility::Action::Hide(vec![id]),
                &session.working_region_stamp(),
            )
            .unwrap();
        assert!(session.area_preview().is_none());
        assert!(interaction.up(&mut session, pointer).unwrap());
        assert_eq!(session.selected_ids(), [id]);
    }

    #[test]
    fn partition_clicks_share_highlights_and_reject_stale_releases_without_edits() {
        use crate::session::components::CreaseEdges;
        use map_core::subobject;
        let mut session = fixture(Projection::Top, None);
        let id = session.component_solids()[0];
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let side = session.component_topology(&[id]).unwrap()[0]
            .faces
            .iter()
            .find(|face| {
                let brush = subobject::solid(session.document(), id).unwrap().1;
                map_core::Plane::from_points(
                    brush
                        .sides
                        .iter()
                        .find(|side| side.id == face.side)
                        .unwrap()
                        .plane
                        .0,
                )
                .unwrap()
                .normal
                .z > 0.99
            })
            .unwrap()
            .side;
        session
            .select_components(
                &[subobject::Component {
                    solid: id,
                    sides: vec![side],
                }],
                SelectMode::Replace,
                &session.component_stamp(),
            )
            .unwrap();
        let pair =
            subobject::crease_pairs(subobject::solid(session.document(), id).unwrap().1, side)
                .unwrap()[0]
                .edges;
        let mut token = session
            .preview_component_partition(
                &[CreaseEdges {
                    solid: id,
                    edges: pair,
                }],
                &session.component_stamp(),
            )
            .unwrap();
        let mut interaction = Interaction::default();
        let point = DVec3::new(-16.0, 0.0, 64.0);
        let pointer = Pointer {
            position: handles::project(&session.camera(), point.as_vec3(), (1100, 700))
                .unwrap()
                .as_dvec2(),
            size: (1100, 700),
        };
        let chosen = session
            .pick_component_partitions(token, session.camera(), pointer)
            .unwrap()
            .remove(0)
            .partition;
        for control in [false, true] {
            assert!(interaction.down(&mut session, pointer, control).unwrap());
            assert!(interaction.active());
            assert!(interaction.moved(&mut session, pointer).unwrap());
            assert!(interaction.up(&mut session, pointer).unwrap());
            assert!(!interaction.active());
            let selection =
                session.component_report(&[id]).unwrap()["preview"]["partition"]["selection"]
                    .clone();
            assert_eq!(
                selection,
                if control {
                    serde_json::json!([])
                } else {
                    serde_json::json!([{"solid":chosen.solid,"partition":chosen.partition}])
                }
            );
        }
        for change in 0..5 {
            interaction.down(&mut session, pointer, false).unwrap();
            let selection =
                session.component_report(&[id]).unwrap()["preview"]["partition"]["selection"]
                    .clone();
            let mut release = pointer;
            match change {
                0 => token = session.raise_component_partition(token, 16.125).unwrap(),
                1 => session.set_camera(Camera::planar(Projection::Top, Vec3::ZERO, 512.0)),
                2 => release.size = (1200, 700),
                3 => release.position.x += 10.0,
                _ => interaction.cancel(&mut session),
            }
            interaction.up(&mut session, release).unwrap();
            assert_eq!(
                session.component_report(&[id]).unwrap()["preview"]["partition"]["selection"],
                selection
            );
        }
        interaction
            .down(
                &mut session,
                Pointer {
                    position: DVec2::ONE,
                    size: (1100, 700),
                },
                false,
            )
            .unwrap();
        interaction
            .up(
                &mut session,
                Pointer {
                    position: DVec2::ONE,
                    size: (1100, 700),
                },
            )
            .unwrap();
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        session.cancel_component_move(token).unwrap();
    }

    #[test]
    fn viewport_component_clicks_share_query_selection_and_preserve_scope() {
        use crate::session::component_picking::{Kind, PICK_RADIUS};
        for projection in [
            Projection::Top,
            Projection::Front,
            Projection::Side,
            Projection::Perspective,
        ] {
            let mut session = fixture(projection, Some(Mode::Move));
            let ids = session.component_solids();
            session
                .set_component_picking(Some(Kind::Vertex), &ids, &session.component_stamp())
                .unwrap();
            assert!(session.transform_guide().is_none());
            let shape = session.component_topology(&ids).unwrap().remove(0);
            let pointer = Pointer {
                position: handles::project(
                    &session.camera(),
                    shape.vertices[0].position.as_vec3(),
                    (1200, 800),
                )
                .unwrap()
                .as_dvec2(),
                size: (1200, 800),
            };
            let original = session.document().vmf().clone();
            let expected = session
                .pick_components(session.camera(), pointer, Kind::Vertex, &ids, PICK_RADIUS)
                .unwrap()
                .candidates[0]
                .component
                .clone();
            let mut interaction = Interaction::default();
            assert!(interaction.down(&mut session, pointer, false).unwrap());
            assert!(interaction.up(&mut session, pointer).unwrap());
            assert_eq!(
                session.component_selection(),
                &std::collections::BTreeSet::from([expected])
            );
            assert!(interaction.down(&mut session, pointer, true).unwrap());
            assert!(interaction.up(&mut session, pointer).unwrap());
            assert!(session.component_selection().is_empty());
            assert_eq!(session.component_solids(), ids);
            assert_eq!(session.document().vmf(), &original);
            assert!(session.area_preview().is_none());
            let empty = Pointer {
                position: DVec2::splat(1.0),
                ..pointer
            };
            interaction.down(&mut session, pointer, false).unwrap();
            interaction.up(&mut session, pointer).unwrap();
            interaction.down(&mut session, empty, false).unwrap();
            interaction.up(&mut session, empty).unwrap();
            assert!(session.component_selection().is_empty());
            assert_eq!(session.component_solids(), ids);
            for changed in 0..5 {
                interaction.down(&mut session, pointer, false).unwrap();
                let mut release = pointer;
                match changed {
                    0 => release.size.0 += 1,
                    1 => release.position.x += 20.0,
                    2 => {
                        let mut camera = session.camera();
                        camera.position.x += 1.0;
                        session.set_camera(camera);
                    }
                    3 => session
                        .select_components(&[], SelectMode::Replace, &session.component_stamp())
                        .unwrap(),
                    _ => {
                        let mut view = session.view();
                        view.tool_textures = !view.tool_textures;
                        session.set_view(view);
                    }
                }
                assert!(interaction.up(&mut session, release).unwrap());
                assert!(session.component_selection().is_empty());
            }
            interaction.down(&mut session, pointer, false).unwrap();
            session.clear_component_edit();
            assert!(interaction.up(&mut session, pointer).unwrap());
            assert_eq!(session.document().vmf(), &original);
        }
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
