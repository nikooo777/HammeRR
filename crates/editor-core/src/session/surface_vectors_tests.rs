use super::*;

#[test]
fn direct_vector_session_previews_retain_invalid_drafts_cancel_and_share_one_undo() {
    let mut results = Vec::new();
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets".into()));
        let ObjectId::Solid(id) = session
            .create_block(actor, DVec3::ZERO, DVec3::splat(64.0), "brick/test", false)
            .unwrap()
        else {
            unreachable!()
        };
        let face = FaceId {
            solid: id,
            side: session
                .document()
                .solid(id)
                .unwrap()
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
                .unwrap()
                .id,
        };
        session.select_faces(&[face], SelectMode::Replace).unwrap();
        let before = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let patch = SurfaceEdit {
            u_axis: Some([Some(2.0), Some(0.5), Some(0.0)]),
            v_axis: Some([Some(0.25), Some(-1.5), Some(0.0)]),
            shift_u: Some(13.125),
            scale_u: Some(-0.375),
            ..Default::default()
        };
        session
            .edit_faces(actor, &[face], patch.clone(), false, true)
            .unwrap();
        let token = session.face_preview_token().unwrap();
        let draft = session.face_preview_document().unwrap().vmf().clone();
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.document().undo_labels().count(), history);
        assert!(
            session
                .edit_faces(
                    actor,
                    &[face],
                    SurfaceEdit {
                        u_axis: Some([Some(0.0); 3]),
                        ..Default::default()
                    },
                    false,
                    true
                )
                .is_err()
        );
        assert_eq!(session.face_preview_token(), Some(token));
        assert_eq!(session.face_preview_document().unwrap().vmf(), &draft);
        assert_eq!(session.document().vmf(), &before);
        session.cancel_face_preview();
        assert_eq!(session.document().vmf(), &before);
        session
            .edit_faces(actor, &[face], patch, false, true)
            .unwrap();
        assert!(session.check_face_preview_token(Some(token)).is_err());
        session.commit_face_preview(actor).unwrap();
        assert_eq!(session.document().vmf(), &draft);
        assert_eq!(session.document().undo_labels().count(), history + 1);
        let after = session.document().vmf().clone();
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &before);
        session.redo(actor).unwrap();
        assert_eq!(session.document().vmf(), &after);
        results.push(after);
    }
    assert_eq!(results[0], results[1]);
}
