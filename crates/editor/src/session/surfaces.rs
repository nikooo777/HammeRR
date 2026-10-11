use super::*;
use map_core::surface::{PaintMode, SurfaceSample};

impl Session {
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
