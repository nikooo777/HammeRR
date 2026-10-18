use super::*;
use source_formats::keyvalues::Node;

fn fixture() -> (Session, Vec<FaceId>) {
    let mut session = Session::new(Err("no assets needed".into()));
    let mut faces = Vec::new();
    for (index, owned) in [false, true].into_iter().enumerate() {
        let start = DVec3::X * index as f64 * 128.0;
        let ObjectId::Solid(solid) = session
            .create_block(
                Actor::User,
                start,
                start + DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            panic!()
        };
        let face = FaceId {
            solid,
            side: session
                .document()
                .solid(solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
                .unwrap()
                .id,
        };
        session
            .edit(Actor::User, "source metadata", |tx| {
                let side = tx
                    .solid_mut(solid)?
                    .sides
                    .iter_mut()
                    .find(|side| side.id == face.side)
                    .unwrap();
                side.u_axis.axis = DVec3::new(2.0, 0.5, 0.0);
                side.v_axis.axis = DVec3::new(0.25, -1.5, 0.0);
                side.u_axis.offset = 13.125;
                side.v_axis.offset = -27.5;
                side.u_axis.scale = -0.375;
                side.v_axis.scale = 0.25;
                side.rotation = 37.0;
                side.lightmap_scale = 8;
                side.smoothing_groups = Some(0x8000_0005);
                side.extra_properties.set("custom", "keep");
                side.extra.push(Node::block(
                    "custom_data",
                    vec![Node::string("entry", "keep")],
                ));
                Ok(())
            })
            .unwrap();
        if owned {
            session
                .edit(Actor::User, "source entity", |tx| {
                    map_core::edit::tie_to_entity(tx, &[solid], "func_brush", &[])
                })
                .unwrap();
        } else {
            session
                .edit(Actor::User, "source terrain", |tx| {
                    map_core::displacement::edit::create(tx, &[face], 2, 0)
                })
                .unwrap();
        }
        faces.push(face);
    }
    (session, faces)
}

#[test]
fn seeded_uv_variation_is_source_relative_order_independent_and_preserves_metadata() {
    let (mut session, faces) = fixture();
    let before = session.document().vmf().clone();
    session
        .vary_face_uv(Actor::User, &faces, 42, 32.0, 16.0, true)
        .unwrap();
    let draft = session.render_document().vmf().clone();
    let offsets = faces
        .iter()
        .map(|face| {
            let original = surface::side(session.document(), *face).unwrap();
            let changed = session.preview_side(*face).unwrap();
            let delta = [
                changed.u_axis.offset - original.u_axis.offset,
                changed.v_axis.offset - original.v_axis.offset,
            ];
            assert!(delta[0].abs() <= 32.0 && delta[1].abs() <= 16.0);
            let mut expected = original.clone();
            expected.u_axis.offset = changed.u_axis.offset;
            expected.v_axis.offset = changed.v_axis.offset;
            assert_eq!(changed, &expected);
            delta
        })
        .collect::<Vec<_>>();
    assert_ne!(offsets[0], offsets[1]);
    session
        .vary_face_uv(
            Actor::Agent,
            &[faces[1], faces[0], faces[1]],
            42,
            32.0,
            16.0,
            true,
        )
        .unwrap();
    assert_eq!(session.render_document().vmf(), &draft);
    assert_eq!(session.document().vmf(), &before);
    session
        .vary_face_uv(Actor::User, &faces, 43, 32.0, 16.0, true)
        .unwrap();
    assert_ne!(session.render_document().vmf(), &draft);
    session
        .vary_face_uv(Actor::User, &faces, 42, 32.0, 0.0, true)
        .unwrap();
    for face in &faces {
        assert_eq!(
            session.preview_side(*face).unwrap().v_axis.offset,
            surface::side(session.document(), *face)
                .unwrap()
                .v_axis
                .offset
        );
    }
    session
        .vary_face_uv(Actor::User, &faces, 42, 32.0, 16.0, false)
        .unwrap();
    assert_eq!(session.document().vmf(), &draft);
    session
        .vary_face_uv(Actor::User, &faces, 42, 32.0, 16.0, false)
        .unwrap();
    for (index, face) in faces.iter().enumerate() {
        let side = surface::side(session.document(), *face).unwrap();
        assert!((side.u_axis.offset - (13.125 + offsets[index][0] * 2.0)).abs() < 1e-12);
        assert!((side.v_axis.offset - (-27.5 + offsets[index][1] * 2.0)).abs() < 1e-12);
    }
}

#[test]
fn uv_variation_invalid_batches_and_zero_maxima_keep_the_previous_draft() {
    let (mut session, faces) = fixture();
    session
        .edit_faces(
            Actor::Agent,
            &faces,
            SurfaceEdit {
                shift_v: Some(91.0),
                ..Default::default()
            },
            false,
            true,
        )
        .unwrap();
    let token = session.face_preview_token();
    let draft = session.render_document().vmf().clone();
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    for preview in [false, true] {
        for (u, v) in [(-1.0, 0.0), (0.0, f64::NAN), (f64::INFINITY, 1.0)] {
            assert!(
                session
                    .vary_face_uv(Actor::User, &faces, 0, u, v, preview)
                    .is_err()
            );
        }
        assert!(
            session
                .vary_face_uv(Actor::User, &[], 0, 1.0, 1.0, preview)
                .is_err()
        );
        let missing = FaceId {
            solid: faces[1].solid,
            side: u32::MAX,
        };
        assert!(
            session
                .vary_face_uv(Actor::User, &[faces[0], missing], 0, 1.0, 1.0, preview)
                .is_err()
        );
        assert_eq!(
            session
                .vary_face_uv(Actor::User, &faces, 0, 0.0, 0.0, preview)
                .unwrap(),
            0
        );
        assert_eq!(session.face_preview_token(), token);
        assert_eq!(session.render_document().vmf(), &draft);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
    }
    session.cancel_face_preview();
    let ObjectId::Entity(owner) = session.document().owner_of_solid(faces[1].solid).unwrap() else {
        panic!()
    };
    session
        .edit(Actor::User, "invalid imported projection", |tx| {
            tx.entity_mut(owner)?
                .solids
                .iter_mut()
                .find(|solid| solid.id == faces[1].solid)
                .unwrap()
                .sides
                .iter_mut()
                .find(|side| side.id == faces[1].side)
                .unwrap()
                .v_axis
                .scale = 0.0;
            Ok(())
        })
        .unwrap();
    session
        .vary_face_uv(Actor::User, &faces[..1], 0, 1.0, 1.0, true)
        .unwrap();
    let token = session.face_preview_token();
    let draft = session.render_document().vmf().clone();
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    for preview in [false, true] {
        assert!(
            session
                .vary_face_uv(Actor::User, &faces, 0, 1.0, 1.0, preview)
                .is_err()
        );
        assert_eq!(session.face_preview_token(), token);
        assert_eq!(session.render_document().vmf(), &draft);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
    }
}

#[test]
fn uv_variation_review_cancel_undo_and_save_reopen_share_the_exact_draft() {
    let (original, faces) = fixture();
    let mut results = Vec::new();
    for actor in [Actor::User, Actor::Agent] {
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        session.replace_document(original.document().clone(), None);
        let before = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        session
            .vary_face_uv(actor, &faces, u32::MAX, 23.5, 7.25, true)
            .unwrap();
        let expired = session.face_preview_token();
        let draft = session.render_document().vmf().clone();
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.document().undo_labels().count(), history);
        session.cancel_face_preview();
        assert_eq!(session.document().vmf(), &before);
        session
            .vary_face_uv(actor, &faces, u32::MAX, 23.5, 7.25, true)
            .unwrap();
        assert!(session.check_face_preview_token(expired).is_err());
        session
            .check_face_preview_token(session.face_preview_token())
            .unwrap();
        assert_eq!(session.commit_face_preview(actor).unwrap(), 2);
        assert_eq!(session.document().vmf(), &draft);
        assert_eq!(session.document().undo_labels().count(), history + 1);
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &before);
        session.redo(actor).unwrap();
        assert_eq!(session.document().vmf(), &draft);
        let path = directory.path().join("uv-variation.vmf");
        session.save(actor, Some(&path)).unwrap();
        let mut reopened = Session::new(Err("no assets needed".into()));
        reopened.open(actor, &path, true).unwrap();
        for face in &faces {
            assert_eq!(
                surface::side(reopened.document(), *face),
                surface::side(session.document(), *face)
            );
        }
        results.push(draft);
    }
    assert_eq!(results[0], results[1]);
}
