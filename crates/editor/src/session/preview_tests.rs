use super::*;
use map_core::clip::Keep;
use map_core::edit::Transform;
use map_core::{Plane, edit};

fn check_draft(session: &Session, source: &source_formats::vmf::Vmf, history: usize) {
    assert_eq!(session.document.vmf(), source);
    assert_eq!(session.document.undo_labels().count(), history);
    assert_eq!(session.document.redo_labels().count(), 1);
    assert_eq!(session.render_document().undo_labels().count(), 1);
    assert_eq!(session.render_document().redo_labels().count(), 0);
}

#[test]
fn history_free_drafts_share_transitions_stale_guards_and_undo_for_both_actors() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets required".into()));
        let id = session
            .create_block(actor, DVec3::ZERO, DVec3::splat(64.0), "test", false)
            .unwrap();
        let ObjectId::Solid(solid) = id else {
            unreachable!()
        };
        for index in 0..100 {
            let delta = DVec3::X * if index % 2 == 0 { 1.0 } else { -1.0 };
            session
                .edit(actor, "move", |tx| edit::translate(tx, &[id], delta, true))
                .unwrap();
        }
        session.undo(actor).unwrap();
        let source = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let face = FaceId {
            solid,
            side: session.document.solid(solid).unwrap().sides[0].id,
        };
        session
            .edit_faces(
                actor,
                &[face],
                SurfaceEdit {
                    shift_u: Some(7.0),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
        let face_token = session.face_preview_token();
        check_draft(&session, &source, history);

        session
            .preview_transform(
                actor,
                &[id],
                Transform {
                    translation: DVec3::Y * 16.0,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        assert!(!session.has_face_preview());
        assert!(session.check_face_preview_token(face_token).is_err());
        check_draft(&session, &source, history);

        session
            .clip_brushes(
                actor,
                &[id],
                Plane {
                    normal: DVec3::X,
                    dist: 32.0,
                },
                Keep::Front,
                None,
                true,
            )
            .unwrap();
        assert!(session.transform_preview.is_none());
        check_draft(&session, &source, history);

        session
            .preview_hollow(
                &[id],
                map_core::hollow::Spec {
                    thickness: 8.0,
                    outward: false,
                    open_faces: Default::default(),
                },
                &session.hollow_stamp(),
            )
            .unwrap();
        let hollow_token = session.hollow_state()["preview"]["token"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(session.clip_preview.is_none());
        check_draft(&session, &source, history);

        session
            .create_primitive(actor, map_core::primitive::Spec::default(), true)
            .unwrap();
        assert!(session.commit_hollow(actor, hollow_token).is_err());
        check_draft(&session, &source, history);
        let token = session.primitive_preview().unwrap().token;
        session.commit_primitive_preview(actor, token).unwrap();
        assert_eq!(session.document.undo_labels().count(), history + 1);
        assert_eq!(session.document.redo_labels().count(), 0);
        session
            .edit_faces(
                actor,
                &[face],
                SurfaceEdit {
                    shift_u: Some(9.0),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &source);
        assert!(!session.has_face_preview());
        assert!(session.primitive_preview().is_none());
        assert!(session.commit_primitive_preview(actor, token).is_err());
    }
}
