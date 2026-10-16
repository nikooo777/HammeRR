use super::*;
use crate::session::SelectMode;
use glam::DVec3;

fn preview(
    session: &mut Session,
    actor: Actor,
    helper: u32,
    face: FaceId,
    operation: Operation,
) -> Value {
    session
        .preview_cubemap_bindings(
            actor,
            Request {
                helper,
                faces: vec![face],
                operation,
            },
            &session.cubemap_bindings_stamp(),
        )
        .unwrap()
}

#[test]
fn binding_and_removal_review_actual_choices_and_preserve_source_in_one_undo() {
    for actor in [Actor::User, Actor::Agent] {
        let (_assets, mut session, near, far, face) = test_support::fixture();
        let before = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let selection = session.selected_faces();
        let report = session
            .get_cubemap_bindings(Query {
                limit: 1,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(report["assignments"].as_array().unwrap().len(), 1);
        assert!(report["total"].as_u64().unwrap() > 1);
        let draft = preview(&mut session, actor, far, face, Operation::Bind);
        assert_eq!(draft["draft"]["report"]["assignments"][0]["helper"], far);
        assert_eq!(draft["draft"]["report"]["assignments"][0]["manual"], true);
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.document.undo_labels().count(), history);
        assert_eq!(session.selected_faces(), selection);
        let token = draft["draft"]["token"].as_u64().unwrap();
        session.apply_cubemap_bindings(actor, token).unwrap();
        assert_eq!(session.document.undo_labels().count(), history + 1);
        let mut expected = before.clone();
        expected
            .entities
            .iter_mut()
            .find(|entity| entity.id == far)
            .unwrap()
            .properties
            .set("sides", face.side.to_string());
        assert_eq!(session.document.vmf(), &expected);
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &before);
        session.redo(actor).unwrap();
        let draft = preview(&mut session, actor, far, face, Operation::Remove);
        assert_eq!(draft["draft"]["report"]["assignments"][0]["helper"], near);
        assert_eq!(draft["draft"]["report"]["assignments"][0]["manual"], false);
        let token = draft["draft"]["token"].as_u64().unwrap();
        session.apply_cubemap_bindings(actor, token).unwrap();
        assert_eq!(session.document.vmf(), &before);
    }
}

#[test]
fn foreign_replaced_and_stale_tokens_cannot_mutate_or_discard_a_draft() {
    let (_assets, mut session, _, far, face) = test_support::fixture();
    let before = session.document.vmf().clone();
    let draft = preview(&mut session, Actor::Agent, far, face, Operation::Bind);
    let token = draft["draft"]["token"].as_u64().unwrap();
    assert!(session.apply_cubemap_bindings(Actor::User, token).is_err());
    assert!(session.cancel_cubemap_bindings(Actor::User, token).is_err());
    let reviewed = session.review_cubemap_bindings(Actor::User, token).unwrap();
    let owned = reviewed["draft"]["token"].as_u64().unwrap();
    assert_ne!(owned, token);
    assert!(
        session
            .cancel_cubemap_bindings(Actor::Agent, token)
            .is_err()
    );
    assert!(
        session
            .preview_cubemap_bindings(
                Actor::Agent,
                Request {
                    helper: far,
                    faces: vec![FaceId {
                        solid: face.solid,
                        side: u32::MAX
                    }],
                    operation: Operation::Bind
                },
                &session.cubemap_bindings_stamp()
            )
            .is_err()
    );
    assert_eq!(session.cubemap_binding_state()["draft"]["token"], owned);
    session.select_faces(&[], SelectMode::Replace).unwrap();
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    assert!(session.apply_cubemap_bindings(Actor::User, owned).is_err());
    session.cancel_cubemap_bindings(Actor::User, owned).unwrap();
    let draft = preview(&mut session, Actor::User, far, face, Operation::Bind);
    let token = draft["draft"]["token"].as_u64().unwrap();
    session
        .edit(Actor::Agent, "change then undo", |tx| {
            tx.entity_mut(far)?.properties.set("custom", "changed");
            Ok(())
        })
        .unwrap();
    session.undo(Actor::Agent).unwrap();
    assert_eq!(session.document.vmf(), &before);
    assert!(session.apply_cubemap_bindings(Actor::User, token).is_err());
    session.cancel_cubemap_bindings(Actor::User, token).unwrap();
    let draft = preview(&mut session, Actor::User, far, face, Operation::Bind);
    let token = draft["draft"]["token"].as_u64().unwrap();
    session.reload_material_assets(Actor::Agent);
    assert!(session.apply_cubemap_bindings(Actor::User, token).is_err());
    session.cancel_cubemap_bindings(Actor::User, token).unwrap();
    assert_eq!(session.document.vmf(), &before);
}

#[test]
fn ambiguous_authored_face_ids_and_hidden_helpers_never_silently_retarget() {
    let (_assets, mut session, near, far, face) = test_support::fixture();
    session
        .edit(Actor::Agent, "ambiguous imported face IDs", |tx| {
            let solid = tx.solid_mut(face.solid)?;
            solid
                .sides
                .iter_mut()
                .find(|side| side.id != face.side)
                .unwrap()
                .id = face.side;
            Ok(())
        })
        .unwrap();
    let before = session.document.vmf().clone();
    assert!(
        session
            .preview_cubemap_bindings(
                Actor::User,
                Request {
                    helper: far,
                    faces: vec![face],
                    operation: Operation::Bind
                },
                &session.cubemap_bindings_stamp()
            )
            .unwrap_err()
            .contains("ambiguous")
    );
    assert_eq!(session.document.vmf(), &before);
    session.undo(Actor::Agent).unwrap();
    session
        .set_visibility(
            Actor::Agent,
            "hide",
            &[ObjectId::Entity(near), ObjectId::Entity(far)],
        )
        .unwrap();
    let before = session.document.vmf().clone();
    assert!(
        session
            .preview_cubemap_bindings(
                Actor::User,
                Request {
                    helper: far,
                    faces: vec![face],
                    operation: Operation::Bind
                },
                &session.cubemap_bindings_stamp()
            )
            .unwrap_err()
            .contains("hidden")
    );
    let report = session
        .get_cubemap_bindings(Query {
            helper: Some(far),
            ..Default::default()
        })
        .unwrap();
    assert!(
        report["helpers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|helper| helper["hidden"] == true)
    );
    assert!(report["assignments"].as_array().unwrap().is_empty());
    assert_eq!(session.document.vmf(), &before);
}

#[test]
fn invalid_manual_ownership_visibility_and_filtered_targets_fail_atomically() {
    let (assets, mut session, near, far, face) = test_support::fixture();
    let first = preview(&mut session, Actor::User, near, face, Operation::Bind);
    session
        .apply_cubemap_bindings(Actor::User, first["draft"]["token"].as_u64().unwrap())
        .unwrap();
    let before = session.document.vmf().clone();
    assert!(
        session
            .preview_cubemap_bindings(
                Actor::User,
                Request {
                    helper: far,
                    faces: vec![face],
                    operation: Operation::Bind
                },
                &session.cubemap_bindings_stamp()
            )
            .unwrap_err()
            .contains("ownership")
    );
    assert_eq!(session.document.vmf(), &before);
    session
        .set_visibility(Actor::Agent, "hide", &[ObjectId::Entity(near)])
        .unwrap();
    let hidden_owner = session.document.vmf().clone();
    assert!(
        session
            .preview_cubemap_bindings(
                Actor::User,
                Request {
                    helper: far,
                    faces: vec![face],
                    operation: Operation::Bind
                },
                &session.cubemap_bindings_stamp()
            )
            .unwrap_err()
            .contains("ownership")
    );
    assert_eq!(session.document.vmf(), &hidden_owner);
    session.undo(Actor::Agent).unwrap();
    session
        .set_visibility(Actor::Agent, "hide", &[ObjectId::Solid(face.solid)])
        .unwrap();
    assert!(
        session
            .preview_cubemap_bindings(
                Actor::User,
                Request {
                    helper: near,
                    faces: vec![face],
                    operation: Operation::Remove
                },
                &session.cubemap_bindings_stamp()
            )
            .is_err()
    );
    let report = session.get_cubemap_bindings(Query::default()).unwrap();
    assert!(!report["diagnostics"].as_array().unwrap().is_empty());
    assert!(report["assignments"].as_array().unwrap().is_empty());
    session.undo(Actor::Agent).unwrap();
    session
        .edit(Actor::Agent, "bad imported binding", |tx| {
            tx.entity_mut(far)?.properties.set("sides", "bad side");
            Ok(())
        })
        .unwrap();
    let before = session.document.vmf().clone();
    assert!(
        session
            .preview_cubemap_bindings(
                Actor::User,
                Request {
                    helper: far,
                    faces: vec![face],
                    operation: Operation::Bind
                },
                &session.cubemap_bindings_stamp()
            )
            .is_err()
    );
    assert_eq!(session.document.vmf(), &before);
    session.undo(Actor::Agent).unwrap();
    std::fs::create_dir_all(assets.path().join("materials/tools")).unwrap();
    std::fs::write(
        assets.path().join("materials/tools/toolsblack.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    session
        .edit(Actor::Agent, "exclude bound brush", |tx| {
            map_core::compile_region::apply(
                tx,
                Some(map_core::compile_region::Definition {
                    enabled: true,
                    bounds: map_core::compile_region::Bounds {
                        mins: DVec3::new(-32.0, -32.0, 32.0),
                        maxs: DVec3::new(160.0, 32.0, 128.0),
                    },
                }),
            )
        })
        .unwrap();
    let before = session.document.vmf().clone();
    let error = session
        .preview_cubemap_bindings(
            Actor::User,
            Request {
                helper: near,
                faces: vec![face],
                operation: Operation::Remove,
            },
            &session.cubemap_bindings_stamp(),
        )
        .unwrap_err();
    assert!(
        error.contains("Requested cubemap face") && error.contains("compile region"),
        "{error}"
    );
    assert_eq!(session.document.vmf(), &before);
}
