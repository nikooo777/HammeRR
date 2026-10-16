use super::*;
use glam::{DVec2, Vec3};

fn fixture(actor: Actor) -> (Session, u64) {
    let mut session = super::super::tests::fixture();
    session
        .create_block(
            actor,
            DVec3::new(-128.0, -128.0, -16.0),
            DVec3::new(128.0, 128.0, 0.0),
            "brick/test",
            false,
        )
        .unwrap();
    let request = super::super::tests::request();
    let state = session
        .preview_linked_path(actor, request, &session.linked_path_stamp())
        .unwrap();
    (session, state["preview"]["token"].as_u64().unwrap())
}

fn begin(session: &mut Session, actor: Actor, token: u64, row: usize) -> u64 {
    session
        .begin_linked_path_pick(
            actor,
            token,
            row,
            session.camera(),
            (511, 511),
            &session.linked_path_pick_stamp(),
        )
        .unwrap()["preview"]["token"]
        .as_u64()
        .unwrap()
}

fn ray() -> Ray {
    Ray {
        origin: DVec3::new(3.25, 7.75, 100.0),
        direction: -DVec3::Z,
    }
}

#[test]
fn row_surface_position_is_exact_owned_draft_only_and_applies_one_undo() {
    for actor in [Actor::User, Actor::Agent] {
        let (mut session, token) = fixture(actor);
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let old = session.linked_path()["preview"]["points"].clone();
        let armed = begin(&mut session, actor, token, 1);
        assert!(session.apply_linked_path(actor, armed).is_err());
        let before_miss = session.linked_path();
        assert!(
            session
                .pick_linked_path_ray(
                    actor,
                    armed,
                    Ray {
                        origin: DVec3::splat(1000.0),
                        direction: DVec3::Z
                    }
                )
                .is_err()
        );
        assert_eq!(session.linked_path(), before_miss);
        let other = if actor == Actor::User {
            Actor::Agent
        } else {
            Actor::User
        };
        assert!(session.pick_linked_path_ray(other, armed, ray()).is_err());
        let hit = session.pick_linked_path_ray(actor, armed, ray()).unwrap();
        assert_ne!(hit["preview"]["token"], armed);
        assert!(hit["preview"]["picking"].is_null());
        assert_eq!(hit["preview"]["points"][0], old[0]);
        assert_eq!(hit["preview"]["points"][2], old[2]);
        assert_eq!(hit["preview"]["points"][1], json!([3.25, 7.75, 0.0]));
        assert_eq!(
            hit["preview"]["last_pick"]["normal"],
            json!([0.0, 0.0, 1.0])
        );
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        session
            .apply_linked_path(actor, hit["preview"]["token"].as_u64().unwrap())
            .unwrap();
        assert_eq!(session.document().undo_labels().count(), history + 1);
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &source);
    }
}

#[test]
fn view_camera_and_owner_changes_reject_picks_but_owned_cancel_cleans_stale_gesture() {
    let (mut session, token) = fixture(Actor::User);
    let armed = begin(&mut session, Actor::User, token, 0);
    let before = session.linked_path()["preview"]["points"].clone();
    let mut view = session.view();
    view.entity_images = !view.entity_images;
    session.set_view(view);
    assert!(
        session
            .pick_linked_path_ray(Actor::User, armed, ray())
            .is_err()
    );
    assert!(
        session
            .cancel_linked_path_pick(Actor::Agent, armed)
            .is_err()
    );
    let state = session.cancel_linked_path_pick(Actor::User, armed).unwrap();
    assert_eq!(state["preview"]["points"], before);
    assert!(state["preview"]["picking"].is_null());
    let armed = begin(
        &mut session,
        Actor::User,
        state["preview"]["token"].as_u64().unwrap(),
        0,
    );
    let mut camera = session.camera();
    camera.position += Vec3::X;
    session.set_camera(camera);
    assert!(
        session
            .pick_linked_path_ray(Actor::User, armed, ray())
            .is_err()
    );
    let state = session.cancel_linked_path_pick(Actor::User, armed).unwrap();
    let reviewed = session
        .review_linked_path(Actor::Agent, state["preview"]["token"].as_u64().unwrap())
        .unwrap();
    assert!(
        session
            .begin_linked_path_pick(
                Actor::User,
                reviewed["preview"]["token"].as_u64().unwrap(),
                0,
                session.camera(),
                (511, 511),
                &session.linked_path_pick_stamp()
            )
            .is_err()
    );
}

#[test]
fn captured_pixel_camera_and_dimensions_match_exact_surface_query() {
    let (mut session, token) = fixture(Actor::Agent);
    let camera = Camera {
        position: Vec3::new(20.0, 30.0, 100.0),
        projection: render::Projection::Top,
        ortho_height: 256.0,
        ..session.camera()
    };
    let armed = session
        .begin_linked_path_pick(
            Actor::Agent,
            token,
            2,
            camera,
            (511, 511),
            &session.linked_path_pick_stamp(),
        )
        .unwrap()["preview"]["token"]
        .as_u64()
        .unwrap();
    assert!(
        session
            .pick_linked_path_pixel(
                Actor::Agent,
                armed,
                Pointer {
                    position: DVec2::splat(255.5),
                    size: (512, 511)
                }
            )
            .is_err()
    );
    assert!(
        session
            .pick_linked_path_pixel(
                Actor::Agent,
                armed,
                Pointer {
                    position: DVec2::splat(-0.5),
                    size: (511, 511)
                }
            )
            .is_err()
    );
    let state = session
        .pick_linked_path_pixel(
            Actor::Agent,
            armed,
            Pointer {
                position: DVec2::splat(255.5),
                size: (511, 511),
            },
        )
        .unwrap();
    assert_eq!(state["preview"]["points"][2], json!([20.0, 30.0, 0.0]));
}

#[test]
fn coincident_surface_position_rejects_without_replacing_the_armed_draft() {
    let (mut session, token) = fixture(Actor::User);
    let armed = begin(&mut session, Actor::User, token, 0);
    let hit = session
        .pick_linked_path_ray(Actor::User, armed, ray())
        .unwrap();
    let armed = begin(
        &mut session,
        Actor::User,
        hit["preview"]["token"].as_u64().unwrap(),
        1,
    );
    let before = session.linked_path();
    assert!(
        session
            .pick_linked_path_ray(Actor::User, armed, ray())
            .unwrap_err()
            .contains("distinct")
    );
    assert_eq!(session.linked_path(), before);
}

#[test]
fn existing_row_pick_preserves_names_ids_properties_connections_and_other_rows() {
    let (mut session, ids) = super::super::existing_tests::fixture("path_track", false);
    session
        .create_block(
            Actor::User,
            DVec3::new(-128.0, -128.0, -16.0),
            DVec3::new(128.0, 128.0, 0.0),
            "brick/test",
            false,
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let request = super::super::existing_tests::edit_request(&session, &ids);
    let state = session
        .preview_existing_linked_path(Actor::User, request, &session.linked_path_stamp())
        .unwrap();
    let armed = begin(
        &mut session,
        Actor::User,
        state["preview"]["token"].as_u64().unwrap(),
        1,
    );
    let picked = session
        .pick_linked_path_ray(Actor::User, armed, ray())
        .unwrap();
    assert_eq!(session.document().vmf(), &source);
    session
        .apply_linked_path(Actor::User, picked["preview"]["token"].as_u64().unwrap())
        .unwrap();
    let mut expected = source.clone();
    expected
        .entities
        .iter_mut()
        .find(|entity| entity.id == ids[1])
        .unwrap()
        .properties
        .set("origin", "3.25 7.75 0");
    assert_eq!(session.document().vmf(), &expected);
    assert_eq!(session.document().undo_labels().count(), history + 1);
    let request = super::super::existing_tests::edit_request(&session, &ids);
    let state = session
        .preview_existing_linked_path(Actor::User, request, &session.linked_path_stamp())
        .unwrap();
    let armed = begin(
        &mut session,
        Actor::User,
        state["preview"]["token"].as_u64().unwrap(),
        1,
    );
    let picked = session
        .pick_linked_path_ray(Actor::User, armed, ray())
        .unwrap();
    session
        .apply_linked_path(Actor::User, picked["preview"]["token"].as_u64().unwrap())
        .unwrap();
    assert_eq!(session.document().undo_labels().count(), history + 1);
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document().vmf(), &source);
}

#[test]
fn invalid_begin_context_and_rays_preserve_the_previous_owned_preview() {
    let (mut session, token) = fixture(Actor::Agent);
    let before = session.linked_path();
    for (row, size) in [(64, (511, 511)), (0, (0, 511)), (0, (8193, 511))] {
        assert!(
            session
                .begin_linked_path_pick(
                    Actor::Agent,
                    token,
                    row,
                    session.camera(),
                    size,
                    &session.linked_path_pick_stamp()
                )
                .is_err()
        );
        assert_eq!(session.linked_path(), before);
    }
    assert!(
        session
            .begin_linked_path_pick(
                Actor::Agent,
                token,
                0,
                session.camera(),
                (511, 511),
                "stale"
            )
            .is_err()
    );
    assert_eq!(session.linked_path(), before);
    let armed = begin(&mut session, Actor::Agent, token, 0);
    let before = session.linked_path();
    for ray in [
        Ray {
            origin: DVec3::ZERO,
            direction: DVec3::ZERO,
        },
        Ray {
            origin: DVec3::splat(f64::NAN),
            direction: -DVec3::Z,
        },
    ] {
        assert!(
            session
                .pick_linked_path_ray(Actor::Agent, armed, ray)
                .is_err()
        );
        assert_eq!(session.linked_path(), before);
    }
}
