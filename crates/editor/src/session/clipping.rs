use glam::{DVec2, DVec3};
use map_core::clip::{self, ClipResult, Keep};
use map_core::{Document, ObjectId, Plane};
use render::Projection;

use super::{Actor, SelectMode, Session};

#[derive(Clone)]
pub struct ClipPreview {
    pub document: Document,
    pub ids: Vec<ObjectId>,
    pub plane: Plane,
    pub keep: Keep,
    pub material: Option<String>,
    pub bounds: [DVec3; 2],
    pub result: ClipResult,
    generation: u64,
    revision: u64,
}

impl Session {
    pub fn can_clip_selection(&self) -> bool {
        !self.selection.is_empty()
            && self.selection.iter().all(|id| match id {
                ObjectId::Solid(id) => self.document.solid(*id).is_some(),
                ObjectId::Entity(id) => self
                    .document
                    .entity(*id)
                    .is_some_and(|entity| !entity.solids.is_empty()),
            })
    }

    pub fn clip_tool(&self) -> bool {
        self.clip_tool
    }

    pub fn set_clip_tool(&mut self, actor: Actor, enabled: bool) -> Result<(), String> {
        if enabled && !self.can_clip_selection() {
            return Err("Select brushes or brush entities before starting a cut".into());
        }
        if self.clip_tool == enabled {
            return Ok(());
        }
        self.cancel_clip_preview();
        self.clear_hollow_preview();
        self.clear_paste_preview();
        self.clear_repeat_preview();
        self.clear_carve_preview();
        self.clear_merge_preview();
        self.clear_component_edit();
        self.clear_primitive_preview();
        self.clear_volume_selection();
        self.cancel_transform_preview();
        self.cancel_area_selection();
        self.clip_tool = enabled;
        if enabled {
            self.paint.enabled = false;
            self.spatial.draw_block = false;
            self.cancel_block_preview();
            self.cancel_face_preview();
            if self.camera.projection == Projection::Perspective {
                self.set_projection(actor, Projection::Top);
            }
        }
        self.view_serial += 1;
        Ok(())
    }

    pub fn clip_epoch(&self) -> u64 {
        self.clip_epoch
    }

    pub fn clip_preview(&self) -> Option<&ClipPreview> {
        self.clip_preview.as_ref()
    }

    pub fn cancel_clip_preview(&mut self) {
        self.clip_drag = None;
        self.clip_epoch += 1;
        if self.clip_points.take().is_some() {
            self.scene_serial += 1;
            self.view_serial += 1;
        }
        self.clear_clip_preview();
    }

    pub fn clear_clip_preview(&mut self) {
        self.clip_drag = None;
        if self.clip_preview.take().is_some() {
            self.clip_epoch += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn clip_line_plane(
        &self,
        projection: Projection,
        start: DVec2,
        end: DVec2,
    ) -> Result<Plane, String> {
        let (horizontal, vertical, depth) = projection
            .axes()
            .ok_or("Choose Top, Front or Side to draw a cut")?;
        if !start.is_finite() || !end.is_finite() {
            return Err("Cut endpoints must be finite".into());
        }
        let mut a = DVec3::ZERO;
        a[horizontal] = start.x;
        a[vertical] = start.y;
        a[depth] = self.spatial.elevation;
        let mut b = a;
        b[horizontal] = end.x;
        b[vertical] = end.y;
        a = self.spatial.snap_point(a);
        b = self.spatial.snap_point(b);
        let mut c = a;
        c[depth] += 64.0;
        Plane::from_points([a, b, c]).ok_or("Draw a nonzero cut line after snapping".into())
    }

    pub fn clip_brushes(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
        plane: Plane,
        keep: Keep,
        material: Option<String>,
        preview: bool,
    ) -> Result<ClipResult, String> {
        let plane = clip::normalized_plane(plane).map_err(|error| error.to_string())?;
        if preview {
            if let Some(previous) = &self.clip_preview
                && previous.ids == ids
                && previous.plane == plane
                && previous.keep == keep
                && previous.material == material
            {
                return Ok(previous.result.clone());
            }
            let mut document = self.document.preview_snapshot();
            let result = document
                .edit("preview clip", |tx| {
                    clip::clip(tx, ids, plane, keep, material.as_deref())
                })
                .map_err(|error| error.to_string())?;
            let (mins, maxs) = map_core::query::bounds_of(&self.document, ids)
                .ok_or("Selected brushes have no bounds")?;
            self.select(ids, SelectMode::Replace)?;
            self.clip_drag = None;
            self.cancel_transform_preview();
            self.clear_hollow_preview();
            self.clear_paste_preview();
            self.clear_repeat_preview();
            self.clear_carve_preview();
            self.clear_merge_preview();
            self.clear_component_edit();
            self.clear_primitive_preview();
            self.clear_volume_selection();
            self.cancel_area_selection();
            self.clip_tool = true;
            self.clip_points = None;
            self.spatial.draw_block = false;
            self.cancel_face_preview();
            self.cancel_block_preview();
            self.clip_preview = Some(ClipPreview {
                document,
                ids: ids.to_vec(),
                plane,
                keep,
                material,
                bounds: [mins, maxs],
                result: result.clone(),
                generation: self.document_generation,
                revision: self.document.revision(),
            });
            self.clip_epoch += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
            Ok(result)
        } else {
            let result = self.edit(actor, "clip brushes", |tx| {
                clip::clip(tx, ids, plane, keep, material.as_deref())
            })?;
            let mut selection = ids.to_vec();
            selection.extend(&result.objects);
            self.select(&selection, SelectMode::Replace)?;
            self.clip_tool = false;
            self.view_serial += 1;
            Ok(result)
        }
    }

    pub fn commit_clip_preview(&mut self, actor: Actor) -> Result<ClipResult, String> {
        if self.clip_drag.is_some() {
            return Err(
                "Finish or cancel positioning the cut plane before applying the cut".into(),
            );
        }
        let preview = self.clip_preview.take().ok_or("No clip preview to apply")?;
        self.scene_serial += 1;
        self.view_serial += 1;
        if preview.generation != self.document_generation
            || preview.revision != self.document.revision()
        {
            return Err("The map changed; draw the cut again".into());
        }
        self.clip_brushes(
            actor,
            &preview.ids,
            preview.plane,
            preview.keep,
            preview.material,
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use map_core::query::Ray;

    fn fixture() -> (Session, ObjectId) {
        let mut session = Session::new(Err("no assets required".into()));
        let id = session
            .create_block(
                Actor::User,
                DVec3::splat(-32.0),
                DVec3::splat(32.0),
                "brick/test",
                false,
            )
            .unwrap();
        session.document.mark_saved();
        (session, id)
    }

    #[test]
    fn shared_cut_previews_preserve_document_history_and_pick_the_rendered_geometry() {
        let (mut session, id) = fixture();
        let original = session.document.vmf().clone();
        let revision = session.document.revision();
        let plane = Plane {
            normal: DVec3::X,
            dist: 0.0,
        };
        let result = session
            .clip_brushes(Actor::Agent, &[id], plane, Keep::Front, None, true)
            .unwrap();
        assert!(session.clip_tool());
        assert_eq!(session.document.vmf(), &original);
        assert_eq!(session.document.revision(), revision);
        assert!(!session.document.is_dirty());
        assert_eq!(session.document.undo_labels().count(), 1);
        assert_eq!(
            session
                .pick(&Ray {
                    origin: DVec3::new(-100.0, 0.0, 0.0),
                    direction: DVec3::X
                })
                .unwrap()
                .point
                .x,
            0.0
        );
        let serial = session.scene_serial();
        session
            .clip_brushes(Actor::User, &[id], plane, Keep::Front, None, true)
            .unwrap();
        assert_eq!(session.scene_serial(), serial);
        let camera = session.camera().frame_bounds(
            DVec3::splat(-32.0).as_vec3(),
            DVec3::splat(32.0).as_vec3(),
            1.0,
        );
        session.set_camera(camera);
        assert!(session.clip_preview().is_some());
        session.cancel_clip_preview();
        assert_eq!(
            session
                .pick(&Ray {
                    origin: DVec3::new(-100.0, 0.0, 0.0),
                    direction: DVec3::X
                })
                .unwrap()
                .point
                .x,
            -32.0
        );
        assert!(session.commit_clip_preview(Actor::User).is_err());
        let preview = session
            .clip_brushes(Actor::Agent, &[id], plane, Keep::Front, None, true)
            .unwrap();
        assert_eq!(preview, result);
        let committed = session.commit_clip_preview(Actor::User).unwrap();
        assert_eq!(committed, preview);
        assert_eq!(session.selected_ids(), committed.objects);
        assert!(!session.clip_tool());
        assert!(session.clip_preview().is_none());
        assert_eq!(session.document.undo_labels().count(), 2);
        assert_eq!(session.recent_log().next().unwrap().actor, Actor::User);
        let changed = session.document.vmf().clone();
        session.undo(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &original);
        session.redo(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &changed);
    }

    #[test]
    fn selection_document_settings_and_tool_changes_invalidate_cuts() {
        let (mut session, id) = fixture();
        let plane = Plane {
            normal: DVec3::Z,
            dist: 0.0,
        };
        for change in 0..5 {
            session.select(&[id], SelectMode::Replace).unwrap();
            session
                .clip_brushes(Actor::Agent, &[id], plane, Keep::Both, None, true)
                .unwrap();
            let epoch = session.clip_epoch();
            match change {
                0 => session.select(&[], SelectMode::Replace).unwrap(),
                1 => session
                    .set_spatial(
                        Actor::Agent,
                        crate::spatial::Update {
                            grid_size: Some(8.0),
                            ..Default::default()
                        },
                    )
                    .unwrap(),
                2 => {
                    session
                        .transform_objects(
                            Actor::Agent,
                            &[id],
                            map_core::edit::Transform {
                                translation: DVec3::X,
                                ..Default::default()
                            },
                            None,
                        )
                        .unwrap();
                }
                3 => session.set_face_mode(true),
                _ => session.set_clip_tool(Actor::Agent, false).unwrap(),
            }
            assert!(session.clip_epoch() > epoch);
            assert!(session.clip_preview().is_none());
            assert!(session.commit_clip_preview(Actor::User).is_err());
        }
        session.select(&[id], SelectMode::Replace).unwrap();
        session
            .clip_brushes(Actor::Agent, &[id], plane, Keep::Both, None, true)
            .unwrap();
        session.new_map(Actor::User, true).unwrap();
        assert!(session.clip_preview().is_none());
        assert!(!session.clip_tool());
    }

    #[test]
    fn human_and_agent_planar_cuts_match_with_fractional_snapping() {
        for projection in [Projection::Top, Projection::Front, Projection::Side] {
            let (mut human, id) = fixture();
            let mut agent = Session::new(Err("no assets required".into()));
            agent.document = human.document.clone();
            agent.select(&[id], SelectMode::Replace).unwrap();
            for session in [&mut human, &mut agent] {
                session
                    .set_spatial(
                        Actor::Agent,
                        crate::spatial::Update {
                            grid_size: Some(0.25),
                            ..Default::default()
                        },
                    )
                    .unwrap();
            }
            let start = DVec2::new(-14.1, -17.1);
            let end = DVec2::new(18.1, 20.1);
            let plane = human.clip_line_plane(projection, start, end).unwrap();
            let a = human
                .clip_brushes(
                    Actor::User,
                    &[id],
                    plane,
                    Keep::Both,
                    Some("brick/cap".into()),
                    true,
                )
                .unwrap();
            let plane = agent.clip_line_plane(projection, start, end).unwrap();
            let b = agent
                .clip_brushes(
                    Actor::Agent,
                    &[id],
                    plane,
                    Keep::Both,
                    Some("brick/cap".into()),
                    false,
                )
                .unwrap();
            assert_eq!(a, b);
            assert_eq!(human.render_document().vmf(), agent.document.vmf());
            human.commit_clip_preview(Actor::User).unwrap();
            assert_eq!(human.document.vmf(), agent.document.vmf());
            assert!(
                human
                    .clip_line_plane(projection, DVec2::ZERO, DVec2::splat(0.1))
                    .is_err()
            );
        }
    }
}
