use super::*;
use map_core::surface::{PaintMode, SurfaceSample};

impl Session {
    pub fn surface_view_options(
        &self,
        mode: Option<render::SurfaceView>,
        group: Option<u32>,
    ) -> Result<render::ViewOptions, String> {
        let mut view = self.view();
        if let Some(mode) = mode {
            view.surface = mode;
        }
        if let Some(group) = group {
            if !(1..=32).contains(&group) {
                return Err("Choose a smoothing group from 1 to 32".into());
            }
            view.smoothing_group = group;
        }
        Ok(view)
    }

    pub fn set_surface_view(
        &mut self,
        mode: render::SurfaceView,
        group: u32,
    ) -> Result<(), String> {
        let view = self.surface_view_options(Some(mode), Some(group))?;
        self.set_view(view);
        Ok(())
    }

    pub fn sample_face_texture(&mut self, face: FaceId) -> Result<(), String> {
        let sample = surface::sample(&self.document, face).map_err(|error| error.to_string())?;
        self.surface_sample = Some(sample);
        self.view_serial += 1;
        Ok(())
    }

    pub fn surface_sample(&self) -> Option<&SurfaceSample> {
        self.surface_sample.as_ref()
    }

    pub fn paint_face_texture(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        mode: PaintMode,
        preview: bool,
    ) -> Result<usize, String> {
        if faces.is_empty() {
            return Err("select at least one target face".into());
        }
        let sample = self
            .surface_sample
            .as_ref()
            .ok_or("sample a face texture first")?;
        let mut edits = Vec::new();
        for face in faces.iter().copied().collect::<BTreeSet<_>>() {
            let edit = surface::paint_edit(&self.document, face, sample, mode)
                .map_err(|error| error.to_string())?;
            edits.push((face, edit));
        }
        self.preview_or_apply_faces(actor, edits, preview)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lighting_previews_tokens_and_diagnostic_settings_share_session_state() {
        for actor in [Actor::User, Actor::Agent] {
            let mut session = Session::new(Err("no assets needed".into()));
            let ObjectId::Solid(id) = session
                .create_block(actor, DVec3::ZERO, DVec3::splat(64.0), "brick/test", false)
                .unwrap()
            else {
                panic!()
            };
            let face = FaceId {
                solid: id,
                side: session.document().solid(id).unwrap().sides[0].id,
            };
            session.select_faces(&[face], SelectMode::Replace).unwrap();
            let before = session.document().vmf().clone();
            let history = session.document().undo_labels().count();
            let patch = SurfaceEdit {
                lightmap_scale: Some(8),
                smoothing_add: surface::smoothing_mask(&[1, 32]).unwrap(),
                ..Default::default()
            };
            session
                .edit_faces(actor, &[face], patch.clone(), false, true)
                .unwrap();
            let token = session.face_preview_token();
            assert_eq!(session.preview_side(face).unwrap().lightmap_scale, 8);
            assert_eq!(session.document().vmf(), &before);
            assert_eq!(session.document().undo_labels().count(), history);
            session
                .set_surface_view(render::SurfaceView::LightmapDensity, 32)
                .unwrap();
            assert_eq!(session.face_preview_token(), token);
            assert_eq!(session.view().surface, render::SurfaceView::LightmapDensity);
            let view = session.view();
            assert!(
                session
                    .set_surface_view(render::SurfaceView::Textured, 0)
                    .is_err()
            );
            assert_eq!(session.view(), view);
            session
                .edit_faces(
                    actor,
                    &[face],
                    SurfaceEdit {
                        lightmap_scale: Some(4),
                        ..patch
                    },
                    false,
                    true,
                )
                .unwrap();
            assert!(session.check_face_preview_token(token).is_err());
            session
                .check_face_preview_token(session.face_preview_token())
                .unwrap();
            session.commit_face_preview(actor).unwrap();
            assert_eq!(session.document().undo_labels().count(), history + 1);
            assert_eq!(
                surface::side(session.document(), face)
                    .unwrap()
                    .smoothing_groups,
                Some(0x8000_0001)
            );
            session.undo(actor).unwrap();
            assert_eq!(session.document().vmf(), &before);
            assert!(session.check_face_preview_token(token).is_err());
        }
    }

    #[test]
    fn sampling_is_read_only_and_paint_previews_are_atomic_and_cancellable() {
        let mut session = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(id) = session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/source",
                false,
            )
            .unwrap()
        else {
            panic!()
        };
        let brush = session.document().solid(id).unwrap();
        let top = FaceId {
            solid: id,
            side: brush
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
                .unwrap()
                .id,
        };
        let wall = FaceId {
            solid: id,
            side: brush
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Y)
                .unwrap()
                .id,
        };
        session
            .edit_faces(
                Actor::User,
                &[top],
                SurfaceEdit {
                    material: Some("metal/sample".into()),
                    shift_u: Some(13.0),
                    ..Default::default()
                },
                false,
                false,
            )
            .unwrap();
        let revision = session.document().revision();
        session.sample_face_texture(top).unwrap();
        assert_eq!(session.document().revision(), revision);
        session.select_faces(&[wall], SelectMode::Replace).unwrap();
        let original = session.document().vmf().clone();
        assert!(
            session
                .paint_face_texture(Actor::Agent, &[wall], PaintMode::Projection, true)
                .is_err()
        );
        assert!(!session.has_face_preview());
        session
            .paint_face_texture(Actor::Agent, &[wall], PaintMode::Wrap, true)
            .unwrap();
        assert_eq!(session.document().vmf(), &original);
        assert_eq!(session.preview_side(wall).unwrap().material, "METAL/SAMPLE");
        let invalid = FaceId {
            solid: id,
            side: u32::MAX,
        };
        assert!(
            session
                .paint_face_texture(Actor::Agent, &[wall, invalid], PaintMode::Material, true)
                .is_err()
        );
        assert!(session.has_face_preview());
        assert_eq!(session.preview_faces(), vec![wall]);
        session.cancel_face_preview();
        assert_eq!(session.document().vmf(), &original);
        session
            .paint_face_texture(Actor::User, &[wall], PaintMode::Wrap, true)
            .unwrap();
        session.commit_face_preview(Actor::Agent).unwrap();
        assert_eq!(
            surface::side(session.document(), wall).unwrap().material,
            "METAL/SAMPLE"
        );
        session.undo(Actor::User).unwrap();
        assert_eq!(session.document().vmf(), &original);
        session.redo(Actor::User).unwrap();
        session
            .edit_faces(
                Actor::User,
                &[top],
                SurfaceEdit {
                    material: Some("wood/new".into()),
                    ..Default::default()
                },
                false,
                false,
            )
            .unwrap();
        assert_eq!(session.surface_sample().unwrap().material, "METAL/SAMPLE");
        session
            .paint_face_texture(Actor::Agent, &[wall], PaintMode::Material, true)
            .unwrap();
        session.select_faces(&[top], SelectMode::Replace).unwrap();
        assert!(!session.has_face_preview());
        session.replace_document(Document::empty(), None);
        assert!(session.surface_sample().is_none());
    }
}
