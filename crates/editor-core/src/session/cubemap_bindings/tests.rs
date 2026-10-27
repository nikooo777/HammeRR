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
fn unsaved_instance_context_requires_a_source_and_preserves_owned_drafts() {
    let reason =
        "Save the parent VMF before inspecting or editing cubemap bindings with visible instances";
    for actor in [Actor::User, Actor::Agent] {
        for hidden in [false, true] {
            let (_assets, mut session, _, far, face) = test_support::fixture();
            assert_eq!(
                session.get_cubemap_bindings(Query::default()).unwrap()["available"],
                true
            );
            let draft = preview(&mut session, actor, far, face, Operation::Bind);
            let token = draft["draft"]["token"].as_u64().unwrap();
            session
                .edit(actor, "reference unavailable child", |tx| {
                    let ObjectId::Entity(id) = map_core::edit::create_entity(
                        tx,
                        "func_instance",
                        Some(DVec3::ZERO),
                        &[("file".into(), "missing-child.vmf".into())],
                        Vec::new(),
                    )?
                    else {
                        unreachable!()
                    };
                    tx.entity_mut(id)?.hidden = hidden;
                    Ok(())
                })
                .unwrap();
            let source = session.document.vmf().clone();
            let history = session.history_snapshot();
            let selected = session.selected_faces();
            let report = session.get_cubemap_bindings(Query::default()).unwrap();
            if hidden {
                assert_eq!(report["available"], true);
                assert_eq!(report["state"]["context_error"], Value::Null);
                assert!(!report["assignments"].as_array().unwrap().is_empty());
                let replacement = preview(&mut session, actor, far, face, Operation::Bind);
                assert_eq!(replacement["draft"]["valid"], true);
                session
                    .cancel_cubemap_bindings(actor, replacement["draft"]["token"].as_u64().unwrap())
                    .unwrap();
                assert_eq!(session.document.vmf(), &source);
                assert_eq!(session.history_snapshot(), history);
                assert_eq!(session.selected_faces(), selected);
                continue;
            }
            assert_eq!(report["available"], false);
            assert_eq!(report["total"], 0);
            assert_eq!(report["assignments"], json!([]));
            assert_eq!(report["diagnostics"], json!([reason]));
            assert_eq!(report["state"]["context_error"], reason);
            assert_eq!(report["state"]["draft"]["token"], token);
            assert_eq!(report["state"]["draft"]["valid"], false);
            assert_eq!(
                session
                    .preview_cubemap_bindings(
                        actor,
                        Request {
                            helper: far,
                            faces: vec![face],
                            operation: Operation::Bind,
                        },
                        &session.cubemap_bindings_stamp(),
                    )
                    .unwrap_err(),
                reason
            );
            assert_eq!(
                session.review_cubemap_bindings(actor, token).unwrap_err(),
                reason
            );
            assert_eq!(
                session.apply_cubemap_bindings(actor, token).unwrap_err(),
                reason
            );
            assert_eq!(session.cubemap_binding_state()["draft"]["token"], token);
            assert_eq!(session.document.vmf(), &source);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.selected_faces(), selected);
            session.cancel_cubemap_bindings(actor, token).unwrap();
            assert_eq!(session.cubemap_binding_state()["draft"], Value::Null);
            assert_eq!(session.document.vmf(), &source);
        }
    }
}

#[test]
fn root_faces_use_assembled_samples_and_root_binding_edits_keep_one_undo() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = test_support::instance_fixture();
        let session = &mut fixture.session;
        let before = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let selection = session.selected_faces();
        let report = session.get_cubemap_bindings(Query::default()).unwrap();
        assert_eq!(fixture.child_helper, fixture.near);
        assert_eq!(report["available"], true, "{report}");
        assert_eq!(report["face_scope"], "root_authored_faces");
        assert_eq!(report["sample_scope"], "assembled_map");
        assert_eq!(report["child_faces_editable"], false);
        assert_eq!(report["helpers"].as_array().unwrap().len(), 2);
        let row = report["assignments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["side"] == fixture.face.side)
            .unwrap();
        assert_eq!(row["helper"], Value::Null);
        assert_eq!(row["position"], json!([0, 0, 48]));
        assert_eq!(row["helper_source"]["scope"], "instance");
        assert_eq!(row["helper_source"]["owner"], fixture.owner);
        assert_eq!(row["helper_source"]["helper"], fixture.child_helper);
        assert_eq!(row["helper_source"]["file"], "leaf.vmf");
        assert_eq!(
            row["helper_source"]["occurrence"],
            json!([
                {"helper":fixture.owner,"file":"middle.vmf"},
                {"helper":2,"file":"leaf.vmf"}
            ])
        );
        let root_sides: BTreeSet<_> = before.side_ids().collect();
        assert!(report["assignments"].as_array().unwrap().iter().all(|row| {
            root_sides.contains(&(row["side"].as_u64().unwrap() as u32)) && row["face"].is_object()
        }));
        let draft = preview(session, actor, fixture.far, fixture.face, Operation::Bind);
        let row = &draft["draft"]["report"]["assignments"][0];
        assert_eq!(row["helper"], fixture.far);
        assert_eq!(
            row["helper_source"],
            json!({"scope":"root","helper":fixture.far})
        );
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.document.undo_labels().count(), history);
        assert_eq!(session.selected_faces(), selection);
        session
            .apply_cubemap_bindings(actor, draft["draft"]["token"].as_u64().unwrap())
            .unwrap();
        assert_eq!(session.document.undo_labels().count(), history + 1);
        let mut expected = before.clone();
        expected
            .entities
            .iter_mut()
            .find(|entity| entity.id == fixture.far)
            .unwrap()
            .properties
            .set("sides", fixture.face.side.to_string());
        assert_eq!(session.document.vmf(), &expected);
        let removed = preview(session, actor, fixture.far, fixture.face, Operation::Remove);
        assert_eq!(
            removed["draft"]["report"]["assignments"][0]["helper_source"],
            row_for_face(&report, fixture.face)["helper_source"]
        );
        session
            .apply_cubemap_bindings(actor, removed["draft"]["token"].as_u64().unwrap())
            .unwrap();
        assert_eq!(session.document.vmf(), &before);
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &expected);
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.selected_faces(), selection);
    }
}

fn row_for_face(report: &Value, face: FaceId) -> &Value {
    report["assignments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["side"] == face.side)
        .unwrap()
}

#[test]
fn original_instance_and_material_sources_stay_frozen_across_refresh_review_and_apply() {
    for actor in [Actor::User, Actor::Agent] {
        for name in [
            "parent.vmf",
            "middle.vmf",
            "leaf.vmf",
            "checker.vmt",
            "child.vmt",
        ] {
            let mut fixture = test_support::instance_fixture();
            let path = if name.ends_with(".vmt") {
                fixture.assets.path().join("materials/test").join(name)
            } else {
                fixture.directory.path().join(name)
            };
            let session = &mut fixture.session;
            let source = session.document.vmf().clone();
            let history = session.history_snapshot();
            let selection = session.selected_faces();
            let report = session.get_cubemap_bindings(Query::default()).unwrap();
            let stamp = report["report_snapshot"].as_str().unwrap();
            let draft = preview(session, actor, fixture.far, fixture.face, Operation::Bind);
            let token = draft["draft"]["token"].as_u64().unwrap();
            let mut bytes = std::fs::read(&path).unwrap();
            bytes.push(b'\n');
            std::fs::write(&path, bytes).unwrap();
            assert_ne!(stamp, session.cubemap_binding_report_stamp());
            assert_eq!(session.cubemap_binding_state()["draft"]["valid"], false);
            let refreshed = session.get_cubemap_bindings(Query::default()).unwrap();
            if name == "parent.vmf" {
                assert_eq!(refreshed["available"], false, "{refreshed}");
                assert_eq!(refreshed["total"], 0);
                assert_eq!(refreshed["assignments"], json!([]));
                assert!(
                    refreshed["diagnostics"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|value| {
                            value.as_str().is_some_and(|message| {
                                message.contains("Map source binding changed")
                                    && message.contains("reopen or Save As")
                            })
                        }),
                    "{refreshed}"
                );
            } else {
                assert_eq!(refreshed["available"], true, "{name}: {refreshed}");
                assert_eq!(refreshed["assignments"], report["assignments"]);
            }
            assert_eq!(refreshed["state"]["draft"]["valid"], false);
            assert!(session.review_cubemap_bindings(actor, token).is_err());
            assert!(session.apply_cubemap_bindings(actor, token).is_err());
            assert_eq!(session.cubemap_binding_state()["draft"]["token"], token);
            assert_eq!(session.document.vmf(), &source);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.selected_faces(), selection);
            session.cancel_cubemap_bindings(actor, token).unwrap();
        }
    }
}

#[test]
fn child_plan_failures_and_generated_face_requests_never_publish_root_only_success() {
    let mut fixture = test_support::instance_fixture();
    let session = &mut fixture.session;
    let control = compiler::Control::default();
    let context =
        compiler::instances::SourceContext::capture(session.path().unwrap(), &control).unwrap();
    let prepared = compiler::instances::resolve_for_compile(
        session.document.vmf(),
        Some(&context),
        &session.game,
        &control,
        &mut compiler::Report::default(),
    )
    .unwrap();
    let generated = prepared.generated_brushes().next().unwrap().solid;
    let face = FaceId {
        solid: generated.id,
        side: generated.sides[0].id,
    };
    drop(prepared);
    let before = session.document.vmf().clone();
    let history = session.history_snapshot();
    let draft = preview(
        session,
        Actor::User,
        fixture.far,
        fixture.face,
        Operation::Bind,
    );
    let token = draft["draft"]["token"].as_u64().unwrap();
    assert!(
        session
            .preview_cubemap_bindings(
                Actor::User,
                Request {
                    helper: fixture.far,
                    faces: vec![face],
                    operation: Operation::Bind
                },
                &session.cubemap_bindings_stamp()
            )
            .is_err()
    );
    assert_eq!(session.cubemap_binding_state()["draft"]["token"], token);
    let path = fixture.directory.path().join("middle.vmf");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut middle = Vmf::parse(&text).unwrap();
    middle.entities[1].properties.set("origin", "0 0 0");
    std::fs::write(&path, middle.write(&Default::default())).unwrap();
    let report = session
        .get_cubemap_bindings(Query {
            helper: Some(fixture.far),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(report["available"], false);
    assert_eq!(report["assignments"], json!([]));
    assert!(
        report["diagnostics"][0]
            .as_str()
            .unwrap()
            .contains("collide"),
        "{report}"
    );
    assert!(session.apply_cubemap_bindings(Actor::User, token).is_err());
    assert_eq!(session.document.vmf(), &before);
    assert_eq!(session.history_snapshot(), history);
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
