use super::*;
use crate::session::SelectMode;
use map_core::ObjectId;
use render::Projection;

fn fixture(actor: Actor) -> (Session, FaceId, u64) {
    let mut session = Session::new(Err("no assets".into()));
    let ObjectId::Solid(solid) = session
        .create_block(
            actor,
            DVec3::ZERO,
            DVec3::new(64.0, 64.0, 16.0),
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
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let face = FaceId { solid, side };
    session.create_terrain(actor, &[face], 2, 1).unwrap();
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    let mask = session
        .highlight_terrain_vertices(actor, &[(face, vec![7, 12])])
        .unwrap();
    session.set_camera(Camera::looking_at(
        Vec3::new(180.0, -240.0, 180.0),
        Vec3::new(32.0, 32.0, 16.0),
    ));
    session.document.mark_saved();
    (session, face, mask)
}

#[test]
fn terrain_handles_share_total_offsets_masks_snapping_review_and_history() {
    for actor in [Actor::User, Actor::Agent] {
        for handle in [
            Handle::X,
            Handle::Y,
            Handle::Z,
            Handle::XY,
            Handle::XZ,
            Handle::YZ,
            Handle::View,
        ] {
            let (mut session, face, mask) = fixture(actor);
            let source = session.document.vmf().clone();
            let original = session
                .terrain_grids(&[face], false)
                .unwrap()
                .remove(0)
                .mesh;
            let token = session
                .enable_terrain_move_handles(actor, &[face], mask)
                .unwrap();
            assert!(
                session
                    .enable_terrain_move_handles(actor, &[face], mask)
                    .is_err()
            );
            assert!(
                session
                    .begin_terrain_move_drag(token, Handle::Uniform, None)
                    .is_err()
            );
            session
                .begin_terrain_move_drag(token, handle, None)
                .unwrap();
            session
                .update_terrain_move_drag(
                    actor,
                    token,
                    None,
                    Some(DVec2::new(
                        19.0,
                        if handle.axis().is_some() { 0.0 } else { -19.0 },
                    )),
                )
                .unwrap();
            let expected = if let Some(axis) = handle.axis() {
                DVec3::AXES[axis] * 16.0
            } else if let Some((a, b)) = handle.plane() {
                (DVec3::AXES[a] - DVec3::AXES[b]) * 16.0
            } else {
                (session.camera.right() - session.camera.up()).as_dvec3() * 16.0
            };
            assert!((session.terrain_move_offset(token).unwrap() - expected).length() < 1e-5);
            let preview = session.terrain_grids(&[face], true).unwrap().remove(0).mesh;
            for (id, (&before, &after)) in original
                .positions
                .iter()
                .zip(&preview.positions)
                .enumerate()
            {
                let delta = if [7, 12].contains(&id) {
                    expected
                } else {
                    DVec3::ZERO
                };
                assert!((after - before - delta).length() < 1e-5);
            }
            assert_eq!(session.document.vmf(), &source);
            let preview_token = session.face_preview_token();
            assert!(
                session
                    .update_terrain_move_drag(actor, token, None, Some(DVec2::new(5000.0, 0.0)))
                    .is_err()
            );
            assert_eq!(session.face_preview_token(), preview_token);
            assert!(session.finish_terrain_move_drag(token).is_err());
            session
                .update_terrain_move_drag(
                    actor,
                    token,
                    None,
                    Some(DVec2::new(
                        19.0,
                        if handle.axis().is_some() { 0.0 } else { -19.0 },
                    )),
                )
                .unwrap();
            assert_eq!(session.face_preview_token(), preview_token);
            let preview = session.render_document().vmf().clone();
            session.finish_terrain_move_drag(token).unwrap();
            assert!(session.terrain_move_guide().is_none());
            session.commit_face_preview(actor).unwrap();
            assert_eq!(session.document.vmf(), &preview);
            assert!(session.terrain_vertex_mask(&[face], Some(mask)).is_ok());
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &source);
            session.redo(actor).unwrap();
            assert_eq!(session.document.vmf(), &preview);
        }
    }
}

#[test]
fn terrain_pointer_handles_match_numeric_axes_and_planes_in_all_projections() {
    for (projection, handle) in [
        (Projection::Top, Handle::X),
        (Projection::Top, Handle::XY),
        (Projection::Front, Handle::Z),
        (Projection::Front, Handle::XZ),
        (Projection::Side, Handle::Z),
        (Projection::Side, Handle::YZ),
        (Projection::Perspective, Handle::X),
        (Projection::Perspective, Handle::View),
    ] {
        let (mut session, face, mask) = fixture(Actor::User);
        let center = Vec3::new(32.0, 32.0, 16.0);
        let camera = if projection == Projection::Perspective {
            session.camera()
        } else {
            Camera::planar(projection, center, 256.0)
        };
        session.set_camera(camera);
        let token = session
            .enable_terrain_move_handles(Actor::User, &[face], mask)
            .unwrap();
        let guide = session.terrain_move_guide().unwrap();
        let shape = handles::shapes(&camera, (1000, 800), &guide)
            .into_iter()
            .find(|shape| shape.handle == handle)
            .unwrap();
        let point = if handle.axis().is_some() {
            (shape.segments[0][0] + shape.segments[0][1]) * 0.5
        } else {
            shape
                .segments
                .iter()
                .map(|segment| segment[0])
                .sum::<Vec3>()
                / shape.segments.len() as f32
        };
        let pointer = Pointer {
            position: handles::project(&camera, point, (1000, 800))
                .unwrap()
                .as_dvec2(),
            size: (1000, 800),
        };
        assert_eq!(session.pick_terrain_move_handle(pointer), Some(handle));
        session
            .begin_terrain_move_drag(token, handle, Some(pointer))
            .unwrap();
        let drag = session
            .terrain_handles
            .active
            .as_ref()
            .unwrap()
            .drag
            .as_ref()
            .unwrap();
        let delta = if let Some(axis) = handle.axis() {
            DVec3::AXES[axis] * 16.0
        } else {
            let (u, v) = drag.axes();
            u * 16.0 + v * 32.0
        };
        let end = Pointer {
            position: handles::project(&camera, point + delta.as_vec3(), pointer.size)
                .unwrap()
                .as_dvec2(),
            ..pointer
        };
        session
            .update_terrain_move_drag(Actor::User, token, Some(end), None)
            .unwrap();
        assert!((session.terrain_move_offset(token).unwrap() - delta).length() < 1e-4);
        let preview = session.face_preview_token();
        assert!(
            session
                .update_terrain_move_drag(
                    Actor::User,
                    token,
                    Some(Pointer {
                        size: (800, 800),
                        ..end
                    }),
                    None
                )
                .is_err()
        );
        assert_eq!(session.face_preview_token(), preview);
        assert!(session.finish_terrain_move_drag(token).is_err());
        session
            .update_terrain_move_drag(Actor::User, token, Some(end), None)
            .unwrap();
        session.finish_terrain_move_drag(token).unwrap();
        session.commit_face_preview(Actor::User).unwrap();
    }
}

#[test]
fn terrain_handles_reject_stale_changes_and_cancel_only_the_owned_preview() {
    let (mut session, face, mask) = fixture(Actor::Agent);
    let source = session.document.vmf().clone();
    let token = session
        .enable_terrain_move_handles(Actor::Agent, &[face], mask)
        .unwrap();
    session
        .begin_terrain_move_drag(token, Handle::Z, None)
        .unwrap();
    session
        .update_terrain_move_drag(Actor::Agent, token, None, Some(DVec2::new(32.0, 0.0)))
        .unwrap();
    session
        .preview_terrain_resolution(Actor::Agent, &[face], 3)
        .unwrap();
    let foreign = session.face_preview_token();
    assert!(session.terrain_move_guide().is_none());
    assert!(session.finish_terrain_move_drag(token).is_err());
    session.cancel_terrain_move_handles(token).unwrap();
    assert_eq!(session.face_preview_token(), foreign);
    session.cancel_face_preview();
    let token = session
        .enable_terrain_move_handles(Actor::Agent, &[face], mask)
        .unwrap();
    session
        .begin_terrain_move_drag(token, Handle::Z, None)
        .unwrap();
    session
        .update_terrain_move_drag(Actor::Agent, token, None, Some(DVec2::ZERO))
        .unwrap();
    session.finish_terrain_move_drag(token).unwrap();
    session.commit_face_preview(Actor::Agent).unwrap();
    assert_eq!(session.document.vmf(), &source);
    for change in 0..4 {
        let mask = session
            .highlight_terrain_vertices(Actor::Agent, &[(face, vec![7])])
            .unwrap();
        let token = session
            .enable_terrain_move_handles(Actor::Agent, &[face], mask)
            .unwrap();
        session
            .begin_terrain_move_drag(token, Handle::Z, None)
            .unwrap();
        session
            .update_terrain_move_drag(Actor::Agent, token, None, Some(DVec2::new(16.0, 0.0)))
            .unwrap();
        match change {
            0 => session.set_camera(Camera::looking_at(Vec3::splat(400.0), Vec3::ZERO)),
            1 => {
                session
                    .highlight_terrain_vertices(Actor::Agent, &[(face, vec![8])])
                    .unwrap();
            }
            2 => {
                session
                    .set_spatial(
                        Actor::Agent,
                        crate::spatial::Update {
                            grid_size: Some(8.0),
                            ..Default::default()
                        },
                    )
                    .unwrap();
            }
            _ => {
                session
                    .create_entity(
                        Actor::Agent,
                        "light",
                        Some(DVec3::splat(64.0)),
                        Default::default(),
                    )
                    .unwrap();
            }
        }
        assert!(session.terrain_move_guide().is_none());
        assert!(
            session
                .update_terrain_move_drag(Actor::Agent, token, None, Some(DVec2::ZERO))
                .is_err()
        );
        assert!(session.finish_terrain_move_drag(token).is_err());
        assert!(session.cancel_terrain_move_handles(token + 1).is_err());
        session.cancel_terrain_move_handles(token).unwrap();
        assert!(session.face_preview_token().is_none());
    }
}
