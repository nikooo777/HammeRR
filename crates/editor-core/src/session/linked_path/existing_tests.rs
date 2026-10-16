use super::*;
use source_formats::vmf::{Connection, Vmf};

pub(crate) fn fixture(classname: &str, closed: bool) -> (Session, Vec<u32>) {
    let mut session = tests::fixture();
    let request = Request {
        classname: classname.into(),
        closed,
        ..tests::request()
    };
    let state = session
        .preview_linked_path(Actor::User, request, &session.linked_path_stamp())
        .unwrap();
    session
        .apply_linked_path(Actor::User, state["preview"]["token"].as_u64().unwrap())
        .unwrap();
    let ids = session
        .document()
        .vmf()
        .entities
        .iter()
        .map(|entity| entity.id)
        .collect::<Vec<_>>();
    session
        .edit(Actor::User, "imported path data", |tx| {
            for &id in &ids {
                let entity = tx.entity_mut(id)?;
                entity.properties.set("custom_key", "Keep me");
                entity.properties.set("spawnflags", "1025");
                entity.properties.set("speed", "imported custom value");
                entity.connections = Some(vec![Connection {
                    output: "OnPass".into(),
                    value: "relay,Trigger,,0.25,-1".into(),
                }]);
            }
            tx.entity_mut(ids[0])?
                .properties
                .set("origin", "0.125 -32.25 8.5000");
            if !closed {
                tx.entity_mut(ids[2])?.properties.set("target", "");
            }
            let ObjectId::Entity(external) = map_core::edit::create_entity(
                tx,
                "func_tracktrain",
                Some(DVec3::ZERO),
                &[
                    ("targetname".into(), "relay".into()),
                    ("target".into(), "route_001".into()),
                ],
                Vec::new(),
            )?
            else {
                unreachable!()
            };
            tx.entity_mut(external)?.connections = Some(vec![Connection {
                output: "OnTrigger".into(),
                value: "route_002,EnablePath,,0,-1".into(),
            }]);
            Ok(())
        })
        .unwrap();
    (session, ids)
}

pub(crate) fn edit_request(session: &Session, ids: &[u32]) -> EditRequest {
    EditRequest {
        nodes: ids
            .iter()
            .map(|&id| ExistingNode {
                id,
                origin: session.document().entity(id).unwrap().origin().unwrap(),
            })
            .collect(),
    }
}

#[test]
fn existing_paths_reorder_and_reposition_without_replacing_identity_or_custom_data() {
    for actor in [Actor::User, Actor::Agent] {
        for classname in ["path_corner", "path_track"] {
            for closed in [false, true] {
                let (mut session, ids) = fixture(classname, closed);
                let source = session.document().vmf().clone();
                let history = session.document().undo_labels().count();
                let selection = session.selected_ids();
                let inspected = session.inspect_existing_linked_path(ids[1]).unwrap();
                assert_eq!(inspected["closed"], closed);
                assert_eq!(inspected["nodes"].as_array().unwrap().len(), 3);
                if !closed {
                    assert_eq!(
                        inspected["nodes"][0]["id"],
                        object_name(ObjectId::Entity(ids[0]))
                    );
                }
                let mut request = edit_request(&session, &ids);
                request.nodes.swap(0, 2);
                request.nodes[1].origin.z += 8.125;
                let state = session
                    .preview_existing_linked_path(
                        actor,
                        request.clone(),
                        inspected["snapshot"].as_str().unwrap(),
                    )
                    .unwrap();
                let token = state["preview"]["token"].as_u64().unwrap();
                assert_eq!(state["preview"]["existing"], true);
                assert_eq!(session.document().vmf(), &source);
                assert_eq!(session.selected_ids(), selection);
                assert_eq!(session.document().undo_labels().count(), history);
                let expected = session.render_document().vmf().clone();
                assert_eq!(expected.entities.len(), source.entities.len());
                for original in &source.entities {
                    let entity = expected
                        .entities
                        .iter()
                        .find(|entity| entity.id == original.id)
                        .unwrap();
                    assert_eq!(entity.connections, original.connections);
                    assert_eq!(entity.editor, original.editor);
                    let untouched = |entity: &source_formats::vmf::Entity| {
                        entity
                            .properties
                            .0
                            .iter()
                            .filter(|(key, _)| {
                                !key.eq_ignore_ascii_case("origin")
                                    && !key.eq_ignore_ascii_case("target")
                            })
                            .cloned()
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(untouched(entity), untouched(original));
                    if !ids.contains(&original.id) {
                        assert_eq!(entity, original);
                    }
                }
                for (index, node) in request.nodes.iter().enumerate() {
                    let entity = expected
                        .entities
                        .iter()
                        .find(|entity| entity.id == node.id)
                        .unwrap();
                    assert_eq!(entity.origin(), Some(node.origin));
                    let next = request
                        .nodes
                        .get(index + 1)
                        .or_else(|| closed.then(|| &request.nodes[0]));
                    let target = next.map(|node| {
                        session
                            .document()
                            .entity(node.id)
                            .unwrap()
                            .properties
                            .get("targetname")
                            .unwrap()
                    });
                    assert_eq!(entity.properties.get("target"), target);
                }
                session.apply_linked_path(actor, token).unwrap();
                assert_eq!(session.document().vmf(), &expected);
                assert_eq!(session.document().undo_labels().count(), history + 1);
                assert_eq!(
                    session.document().undo_labels().next(),
                    Some("edit linked path")
                );
                session.undo(actor).unwrap();
                assert_eq!(session.document().vmf(), &source);
                session.redo(actor).unwrap();
                assert_eq!(session.document().vmf(), &expected);
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join("existing-path.vmf");
                session.save(actor, Some(&path)).unwrap();
                assert_eq!(
                    Vmf::parse(&std::fs::read_to_string(path).unwrap())
                        .unwrap()
                        .entities,
                    expected.entities
                );
            }
        }
    }
}

#[test]
fn existing_path_noop_cancel_review_and_replay_preserve_authored_bytes_and_history() {
    let (mut session, ids) = fixture("path_corner", false);
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let request = edit_request(&session, &ids);
    let state = session
        .preview_existing_linked_path(Actor::Agent, request.clone(), &session.linked_path_stamp())
        .unwrap();
    let token = state["preview"]["token"].as_u64().unwrap();
    assert_eq!(session.render_document().vmf(), &source);
    assert!(session.apply_linked_path(Actor::User, token).is_err());
    assert!(session.cancel_linked_path(Actor::User, token).is_err());
    let reviewed = session.review_linked_path(Actor::User, token).unwrap();
    let token2 = reviewed["preview"]["token"].as_u64().unwrap();
    assert_ne!(token, token2);
    assert!(session.apply_linked_path(Actor::Agent, token).is_err());
    session.cancel_linked_path(Actor::User, token2).unwrap();
    assert_eq!(session.document().vmf(), &source);
    let state = session
        .preview_existing_linked_path(Actor::User, request, &session.linked_path_stamp())
        .unwrap();
    session
        .apply_linked_path(Actor::User, state["preview"]["token"].as_u64().unwrap())
        .unwrap();
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    assert!(session.linked_path()["preview"].is_null());
    let mut request = edit_request(&session, &ids);
    request.nodes.reverse();
    let state = session
        .preview_existing_linked_path(Actor::User, request, &session.linked_path_stamp())
        .unwrap();
    session
        .geometry
        .linked_path
        .preview
        .as_mut()
        .unwrap()
        .document
        .edit("tamper", |tx| {
            tx.entity_mut(ids[0])?
                .properties
                .set("custom_key", "Changed");
            Ok(())
        })
        .unwrap();
    assert!(
        session
            .apply_linked_path(Actor::User, state["preview"]["token"].as_u64().unwrap())
            .is_err()
    );
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
}

#[test]
fn ambiguous_unsupported_graphs_and_invalid_edits_preserve_the_current_draft() {
    for case in 0..12 {
        let (mut session, ids) = fixture("path_track", false);
        let request = edit_request(&session, &ids);
        session
            .edit(Actor::User, "unsupported graph", |tx| {
                match case {
                    0 => tx.entity_mut(ids[0])?.properties.set("target", "missing"),
                    1 => tx.entity_mut(ids[0])?.properties.set("target", "route_*"),
                    2 => tx
                        .entity_mut(ids[0])?
                        .properties
                        .set("altpath", "route_003"),
                    3 => tx.entity_mut(ids[2])?.properties.set("target", "route_002"),
                    4 => tx
                        .entity_mut(ids[2])?
                        .properties
                        .set("classname", "path_corner"),
                    5 => tx.entity_mut(ids[1])?.hidden = true,
                    6 => tx
                        .entity_mut(ids[1])?
                        .properties
                        .set("parentname", "parent"),
                    7 => tx.entity_mut(ids[0])?.properties.set("target", "relay"),
                    8 => {
                        map_core::edit::create_entity(
                            tx,
                            "info_target",
                            Some(DVec3::ZERO),
                            &[("targetname".into(), "ROUTE_002".into())],
                            Vec::new(),
                        )?;
                    }
                    9 => {
                        map_core::edit::create_entity(
                            tx,
                            "path_track",
                            Some(DVec3::ZERO),
                            &[
                                ("targetname".into(), "extra".into()),
                                ("target".into(), "route_002".into()),
                            ],
                            Vec::new(),
                        )?;
                    }
                    10 => tx
                        .entity_mut(ids[0])?
                        .properties
                        .0
                        .push(("TARGET".into(), "route_003".into())),
                    11 => {
                        map_core::edit::create_entity(
                            tx,
                            "path_track",
                            Some(DVec3::ZERO),
                            &[
                                ("targetname".into(), "extra".into()),
                                ("altpath".into(), "route_002".into()),
                            ],
                            Vec::new(),
                        )?;
                    }
                    _ => unreachable!(),
                }
                Ok(())
            })
            .unwrap();
        let mut creation = tests::request();
        creation.prefix = "safe".into();
        let good = session
            .preview_linked_path(Actor::User, creation, &session.linked_path_stamp())
            .unwrap();
        let source = session.document().vmf().clone();
        let rendered = session.render_document().vmf().clone();
        let history = session.document().undo_labels().count();
        assert!(
            session.inspect_existing_linked_path(ids[0]).is_err(),
            "case {case}"
        );
        assert!(
            session
                .preview_existing_linked_path(Actor::User, request, &session.linked_path_stamp())
                .is_err(),
            "case {case}"
        );
        assert_eq!(session.linked_path(), good);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.render_document().vmf(), &rendered);
        assert_eq!(session.document().undo_labels().count(), history);
    }
    let (mut session, ids) = fixture("path_corner", false);
    let source = session.document().vmf().clone();
    let request = edit_request(&session, &ids);
    for case in 0..5 {
        let mut invalid = request.clone();
        match case {
            0 => {
                invalid.nodes.pop();
            }
            1 => invalid.nodes[1].id = invalid.nodes[0].id,
            2 => invalid.nodes[1].origin = DVec3::splat(f64::NAN),
            3 => invalid.nodes[1].origin = DVec3::splat(crate::spatial::MAX_EXTENT + 1.0),
            4 => invalid.nodes[1].origin = invalid.nodes[0].origin,
            _ => unreachable!(),
        }
        assert!(
            session
                .preview_existing_linked_path(Actor::Agent, invalid, &session.linked_path_stamp())
                .is_err()
        );
        assert_eq!(session.document().vmf(), &source);
    }
    let snapshot = session.linked_path_stamp();
    session
        .set_entity_properties(
            Actor::User,
            ids[0],
            &[("custom_key".into(), Some("New data".into()))],
        )
        .unwrap();
    assert!(
        session
            .preview_existing_linked_path(Actor::Agent, request, &snapshot)
            .is_err()
    );
}

#[test]
fn existing_edits_validate_changed_fields_and_work_without_definitions() {
    for fgd in [
        "@SolidClass = path_corner []",
        "@PointClass = path_corner [ target(target_destination) readonly : \"Next\" ]",
        "@PointClass = path_corner [ origin(origin) readonly : \"Position\" ]",
    ] {
        let (mut session, ids) = fixture("path_corner", false);
        session.set_definitions(
            Actor::Agent,
            source_formats::fgd::Catalog::parse(fgd).unwrap(),
        );
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let mut request = edit_request(&session, &ids);
        request.nodes.reverse();
        request.nodes[1].origin.z += 1.0;
        assert!(
            session
                .preview_existing_linked_path(Actor::Agent, request, &session.linked_path_stamp())
                .is_err()
        );
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
    }
    let (mut session, ids) = fixture("path_track", true);
    session.set_definitions(Actor::User, source_formats::fgd::Catalog::default());
    let mut request = edit_request(&session, &ids);
    request.nodes.reverse();
    let state = session
        .preview_existing_linked_path(Actor::Agent, request, &session.linked_path_stamp())
        .unwrap();
    session
        .apply_linked_path(Actor::Agent, state["preview"]["token"].as_u64().unwrap())
        .unwrap();
    assert_eq!(
        session
            .document()
            .entity(ids[0])
            .unwrap()
            .properties
            .get("custom_key"),
        Some("Keep me")
    );
}
