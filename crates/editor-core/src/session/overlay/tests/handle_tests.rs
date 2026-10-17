use super::*;
use crate::session::Pointer;
use glam::DVec2;
use render::{Camera, Projection};

const SIZE: (u32, u32) = (900, 700);

fn token(state: &Value) -> u64 {
    state["draft"]["token"].as_u64().unwrap()
}

fn corner_fixture(actor: Actor, detail: bool) -> (tempfile::TempDir, Session, u32, Definition) {
    let (directory, mut session, source) = fixture(actor, detail);
    let created = preview(&mut session, actor, source);
    let state = session.apply_overlay(actor, created).unwrap();
    let ObjectId::Entity(id) =
        crate::identity::parse_id(state["created"].as_str().unwrap()).unwrap()
    else {
        unreachable!()
    };
    session
        .edit(actor, "raw overlay fields", |tx| {
            let entity = tx.entity_mut(id)?;
            entity.properties.set("StartU", "-00.2500");
            entity.properties.set("custom_note", "keep these bytes");
            for index in 0..4 {
                let point = source_formats::vmf::parse_vec3(
                    entity.properties.get(&format!("uv{index}")).unwrap(),
                )
                .unwrap();
                entity.properties.set(
                    format!("uv{index}"),
                    format!("{} {} {}", point.x, point.y, 17.125 + index as f64),
                );
            }
            Ok(())
        })
        .unwrap();
    let (_, definition) = overlay::editable_corners(session.document.vmf(), id).unwrap();
    let state = session
        .preview_overlay_corners(
            actor,
            id,
            definition.uv_points.map(|point| [point[0], point[1]]),
            &session.overlay_stamp(),
        )
        .unwrap();
    assert_eq!(state["draft"]["operation"], "corners");
    (directory, session, id, definition)
}

fn camera(definition: &Definition, planar: bool) -> Camera {
    let center = DVec3::from_array(definition.basis_origin).as_vec3();
    if planar {
        Camera::planar(Projection::Top, center, 300.0)
    } else {
        Camera::looking_at(
            center + DVec3::from_array(definition.basis_normal).as_vec3() * 300.0,
            center,
        )
    }
}

fn begin(session: &mut Session, actor: Actor, camera: Camera) -> Value {
    let old = token(&session.get_overlay_state());
    let state = session
        .begin_overlay_corner_handles(
            actor,
            old,
            camera,
            SIZE,
            &session.overlay_corner_handle_stamp(),
        )
        .unwrap();
    assert_ne!(token(&state), old);
    assert_eq!(state["draft"]["handles"]["valid"], true);
    assert!(session.cancel_overlay_corner_handles(actor, old).is_err());
    state
}

fn pointer(state: &Value, corner: usize) -> Pointer {
    Pointer {
        position: DVec2::new(
            state["draft"]["handles"]["pixels"][corner][0]
                .as_f64()
                .unwrap(),
            state["draft"]["handles"]["pixels"][corner][1]
                .as_f64()
                .unwrap(),
        ),
        size: SIZE,
    }
}

#[test]
fn viewport_corner_drags_share_plane_motion_preservation_and_one_atomic_apply() {
    for actor in [Actor::User, Actor::Agent] {
        for detail in [false, true] {
            for planar in [false, true] {
                let (_directory, mut session, id, original) = corner_fixture(actor, detail);
                let before = session.document.vmf().clone();
                let history = session.document.undo_labels().count();
                let selection = session.selected_ids();
                let old_camera = session.camera();
                let lines = session.build_scene().lines.len();
                let camera = camera(&original, planar);
                let state = begin(&mut session, actor, camera);
                assert_eq!(session.build_scene().lines.len(), lines + 32);
                assert!(session.apply_overlay(actor, token(&state)).is_err());
                let mut start = pointer(&state, 0);
                start.position += DVec2::new(2.5, -1.25);
                let state = session
                    .pick_overlay_corner_handle(actor, token(&state), start)
                    .unwrap();
                assert_eq!(state["draft"]["handles"]["dragging"], 0);
                let state = session
                    .move_overlay_corner_handle(actor, token(&state), start)
                    .unwrap();
                assert_eq!(session.overlay.draft.as_ref().unwrap().definition, original);
                let end = Pointer {
                    position: start.position + DVec2::new(14.5, -9.25),
                    ..start
                };
                let state = session
                    .move_overlay_corner_handle(actor, token(&state), end)
                    .unwrap();
                let moved = session.overlay.draft.as_ref().unwrap().definition.clone();
                let ray_delta = |pointer: Pointer| {
                    let (ray_origin, direction) = pointer.ray(camera);
                    let normal = DVec3::from_array(original.basis_u)
                        .cross(DVec3::from_array(original.basis_v));
                    ray_origin
                        + direction
                            * ((DVec3::from_array(original.basis_origin) - ray_origin).dot(normal)
                                / direction.dot(normal))
                };
                let expected =
                    DVec3::from_array(original.corners()[0]) + ray_delta(end) - ray_delta(start);
                assert!(DVec3::from_array(moved.corners()[0]).abs_diff_eq(expected, 0.002));
                assert_eq!(&moved.uv_points[1..], &original.uv_points[1..]);
                assert_eq!(moved.uv_points[0][2], original.uv_points[0][2]);
                assert_eq!(moved.basis_origin, original.basis_origin);
                assert_eq!(moved.basis_u, original.basis_u);
                assert_eq!(moved.basis_v, original.basis_v);
                let state = session
                    .finish_overlay_corner_handles(actor, token(&state))
                    .unwrap();
                assert!(state["draft"]["handles"].is_null());
                assert_eq!(session.document.vmf(), &before);
                assert_eq!(session.document.undo_labels().count(), history);
                assert_eq!(session.selected_ids(), selection);
                assert_eq!(session.camera(), old_camera);
                session.apply_overlay(actor, token(&state)).unwrap();
                assert_eq!(
                    overlay::parse(session.document.entity(id).unwrap()).unwrap(),
                    moved
                );
                for (key, value) in before
                    .entities
                    .iter()
                    .find(|entity| entity.id == id)
                    .unwrap()
                    .properties
                    .iter()
                {
                    if !["uv0", "uv1", "uv2", "uv3"].contains(&key) {
                        assert_eq!(
                            session.document.entity(id).unwrap().properties.get(key),
                            Some(value)
                        );
                    }
                }
                assert_eq!(session.document.undo_labels().count(), history + 1);
                let after = session.document.vmf().clone();
                session.undo(actor).unwrap();
                assert_eq!(session.document.vmf(), &before);
                session.redo(actor).unwrap();
                assert_eq!(session.document.vmf(), &after);
            }
        }
    }
}

#[test]
fn corner_handle_misses_invalid_motion_and_foreign_tokens_preserve_the_valid_draft() {
    let (_directory, mut session, _id, original) = corner_fixture(Actor::User, false);
    let state = begin(&mut session, Actor::User, camera(&original, false));
    let original_token = token(&state);
    let miss = Pointer {
        position: DVec2::new(1.0, 1.0),
        size: SIZE,
    };
    assert!(
        session
            .pick_overlay_corner_handle(Actor::User, original_token, miss)
            .is_err()
    );
    assert_eq!(session.overlay_corner_handle_state(), state);
    let start = pointer(&state, 0);
    assert!(
        session
            .pick_overlay_corner_handle(Actor::Agent, original_token, start)
            .is_err()
    );
    assert_eq!(session.overlay_corner_handle_state(), state);
    let state = session
        .pick_overlay_corner_handle(Actor::User, original_token, start)
        .unwrap();
    assert!(
        session
            .move_overlay_corner_handle(Actor::User, original_token, start)
            .is_err()
    );
    let valid = Pointer {
        position: start.position + DVec2::new(-10.0, 6.0),
        ..start
    };
    let state = session
        .move_overlay_corner_handle(Actor::User, token(&state), valid)
        .unwrap();
    let opposing = pointer(&state, 2);
    for invalid in [
        Pointer {
            position: opposing.position + (opposing.position - start.position) * 0.25,
            ..start
        },
        Pointer {
            position: DVec2::new(f64::NAN, 5.0),
            ..start
        },
        Pointer {
            size: (SIZE.0 + 1, SIZE.1),
            ..valid
        },
        Pointer {
            position: DVec2::new(-1.0, 10.0),
            ..start
        },
    ] {
        assert!(
            session
                .move_overlay_corner_handle(Actor::User, token(&state), invalid)
                .is_err()
        );
        assert_eq!(session.overlay_corner_handle_state(), state);
    }
    assert!(
        session
            .finish_overlay_corner_handles(Actor::Agent, token(&state))
            .is_err()
    );
    assert!(
        session
            .cancel_overlay_corner_handles(Actor::Agent, token(&state))
            .is_err()
    );
    assert_eq!(session.overlay_corner_handle_state(), state);
    let state = session
        .cancel_overlay_corner_handles(Actor::User, token(&state))
        .unwrap();
    assert_eq!(session.overlay.draft.as_ref().unwrap().definition, original);
    assert!(state["draft"]["handles"].is_null());
}

#[test]
fn corner_handles_reject_stale_context_but_allow_owned_exact_cancellation() {
    for change in ["camera", "view", "scene", "selection", "source", "document"] {
        let (_directory, mut session, _id, original) = corner_fixture(Actor::User, false);
        let state = begin(&mut session, Actor::User, camera(&original, false));
        let start = pointer(&state, 0);
        let state = session
            .pick_overlay_corner_handle(Actor::User, token(&state), start)
            .unwrap();
        let end = Pointer {
            position: start.position + DVec2::new(-12.0, 8.0),
            ..start
        };
        let state = session
            .move_overlay_corner_handle(Actor::User, token(&state), end)
            .unwrap();
        match change {
            "camera" => session.set_camera(session.camera()),
            "view" => session.set_view(render::ViewOptions {
                tool_textures: !session.view().tool_textures,
                ..session.view()
            }),
            "scene" => session.scene_serial += 1,
            "selection" => session.select(&[], SelectMode::Replace).unwrap(),
            "source" => {
                let source = session.overlay.draft.as_ref().unwrap().source;
                session
                    .edit(Actor::User, "move bound source", |tx| {
                        edit::transform(
                            tx,
                            &[ObjectId::Solid(source.solid)],
                            edit::Transform {
                                translation: DVec3::Z,
                                ..Default::default()
                            },
                        )
                    })
                    .unwrap();
            }
            "document" => session.new_map(Actor::User, true).unwrap(),
            _ => unreachable!(),
        }
        let after_change = session.document.vmf().clone();
        assert!(
            session
                .move_overlay_corner_handle(Actor::User, token(&state), end)
                .is_err(),
            "{change}"
        );
        assert!(
            session
                .finish_overlay_corner_handles(Actor::User, token(&state))
                .is_err(),
            "{change}"
        );
        let state = session
            .cancel_overlay_corner_handles(Actor::User, token(&state))
            .unwrap();
        assert_eq!(
            session.overlay.draft.as_ref().unwrap().definition,
            original,
            "{change}"
        );
        assert!(state["draft"]["handles"].is_null());
        assert_eq!(session.document.vmf(), &after_change);
    }
}

#[test]
fn corner_handle_admission_rejects_noncorner_stale_foreign_and_invalid_viewports() {
    let (_directory, mut session, _id, original) = corner_fixture(Actor::User, false);
    let state = session.overlay_corner_handle_state();
    let token = token(&state);
    let camera = camera(&original, false);
    for (actor, size, snapshot, camera) in [
        (
            Actor::Agent,
            SIZE,
            session.overlay_corner_handle_stamp(),
            camera,
        ),
        (Actor::User, SIZE, "stale".into(), camera),
        (
            Actor::User,
            (0, SIZE.1),
            session.overlay_corner_handle_stamp(),
            camera,
        ),
        (
            Actor::User,
            (8193, SIZE.1),
            session.overlay_corner_handle_stamp(),
            camera,
        ),
        (
            Actor::User,
            SIZE,
            session.overlay_corner_handle_stamp(),
            Camera {
                position: glam::Vec3::NAN,
                ..camera
            },
        ),
    ] {
        assert!(
            session
                .begin_overlay_corner_handles(actor, token, camera, size, &snapshot)
                .is_err()
        );
        assert_eq!(session.overlay_corner_handle_state(), state);
    }
    let face = session.overlay.draft.as_ref().unwrap().source;
    let token = preview(&mut session, Actor::User, face);
    assert!(
        session
            .begin_overlay_corner_handles(
                Actor::User,
                token,
                camera,
                SIZE,
                &session.overlay_corner_handle_stamp()
            )
            .is_err()
    );
}

#[test]
fn reviewing_active_handles_transfers_the_last_valid_draft_and_closes_the_old_gesture() {
    for (owner, reviewer) in [(Actor::User, Actor::Agent), (Actor::Agent, Actor::User)] {
        let (_directory, mut session, _id, original) = corner_fixture(owner, false);
        let before = session.document.vmf().clone();
        let state = begin(&mut session, owner, camera(&original, false));
        let start = pointer(&state, 0);
        let state = session
            .pick_overlay_corner_handle(owner, token(&state), start)
            .unwrap();
        let end = Pointer {
            position: start.position + DVec2::new(-9.5, 6.25),
            ..start
        };
        let state = session
            .move_overlay_corner_handle(owner, token(&state), end)
            .unwrap();
        let moved = session.overlay.draft.as_ref().unwrap().definition.clone();
        session.set_camera(session.camera());
        let reviewed = session.review_overlay(reviewer, token(&state)).unwrap();
        assert_ne!(token(&reviewed), token(&state));
        assert_eq!(reviewed["draft"]["owner"], reviewer.label());
        assert!(reviewed["draft"]["handles"].is_null());
        assert_eq!(session.overlay.draft.as_ref().unwrap().definition, moved);
        assert!(
            session
                .move_overlay_corner_handle(owner, token(&state), end)
                .is_err()
        );
        assert!(
            session
                .cancel_overlay_corner_handles(owner, token(&state))
                .is_err()
        );
        assert!(session.apply_overlay(owner, token(&reviewed)).is_err());
        assert_eq!(session.document.vmf(), &before);
        session.apply_overlay(reviewer, token(&reviewed)).unwrap();
        session.undo(reviewer).unwrap();
        assert_eq!(session.document.vmf(), &before);
    }
}
