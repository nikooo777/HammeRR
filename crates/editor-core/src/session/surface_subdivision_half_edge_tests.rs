use super::*;

fn cage(session: &mut Session, actor: Actor) -> Vec<FaceId> {
    let mut faces = Vec::new();
    for (mins, maxs, normal, power, corner) in [
        ([0.0, 0.0, 48.0], [64.0, 128.0, 64.0], DVec3::Z, 2, 1),
        ([0.0, 0.0, 0.0], [16.0, 64.0, 64.0], DVec3::NEG_X, 3, 3),
        ([0.0, 64.0, 0.0], [16.0, 128.0, 64.0], DVec3::NEG_X, 4, 2),
    ] {
        let ObjectId::Solid(solid) = session
            .create_block(
                actor,
                DVec3::from_array(mins),
                DVec3::from_array(maxs),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let side = session
            .document
            .solid(solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| {
                map_core::Plane::from_points(side.plane.0)
                    .unwrap()
                    .normal
                    .dot(normal)
                    > 0.99
            })
            .unwrap()
            .id;
        let face = FaceId { solid, side };
        session
            .create_terrain(actor, &[face], power, corner)
            .unwrap();
        faces.push(face);
    }
    faces
}

#[test]
fn half_edge_subdivision_uses_both_actors_owned_preview_and_atomic_history() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets".into()));
        let faces = cage(&mut session, actor);
        let before = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let preview = session.preview_terrain_subdivision(actor, &faces).unwrap();
        assert_eq!(
            (preview.changed, preview.shared_edges, preview.vertices),
            (3, 3, 395)
        );
        let token = session.face_preview_token();
        let rounded = session.terrain_grids(&faces, true).unwrap();
        assert!(rounded.iter().all(|grid| grid.subdivided));
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.document.undo_labels().count(), history);
        assert!(
            session
                .preview_terrain_subdivision(
                    actor,
                    &[
                        faces[0],
                        FaceId {
                            solid: faces[1].solid,
                            side: u32::MAX
                        },
                    ]
                )
                .is_err()
        );
        assert_eq!(session.face_preview_token(), token);
        assert_eq!(
            session.terrain_grids(&faces, true).unwrap()[0].mesh,
            rounded[0].mesh
        );
        session.cancel_face_preview();
        assert_eq!(session.document.vmf(), &before);
        session.preview_terrain_subdivision(actor, &faces).unwrap();
        session.commit_face_preview(actor).unwrap();
        let saved = session.document.vmf().clone();
        assert_eq!(session.document.undo_labels().count(), history + 1);
        assert_eq!(
            session
                .preview_terrain_subdivision(actor, &faces)
                .unwrap()
                .changed,
            0
        );
        session.cancel_face_preview();
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &before);
        session.redo(actor).unwrap();
        assert_eq!(session.document.vmf(), &saved);
    }
}
