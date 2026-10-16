use super::*;
use source_formats::vmf::{Connection, Entity, Vmf};

pub(crate) fn fixture(classname: &str, closed: bool) -> (Session, Vec<u32>) {
    let mut session = tests::fixture();
    let state = session
        .preview_linked_path(
            Actor::User,
            Request {
                classname: classname.into(),
                closed,
                ..tests::request()
            },
            &session.linked_path_stamp(),
        )
        .unwrap();
    let result = session
        .apply_linked_path(Actor::User, token(&state))
        .unwrap();
    let ids = result["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| {
            id.as_str()
                .unwrap()
                .strip_prefix("entity:")
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    (session, ids)
}

fn token(state: &Value) -> u64 {
    state["preview"]["token"].as_u64().unwrap()
}

fn request(session: &Session, ids: &[u32]) -> ChangeRequest {
    ChangeRequest {
        seed: ids[0],
        prefix: "inserted".into(),
        nodes: ids
            .iter()
            .map(|&id| EditNode {
                id: Some(id),
                origin: session.document().entity(id).unwrap().origin().unwrap(),
            })
            .collect(),
    }
}

fn authored(entity: &Entity) -> Entity {
    let mut entity = entity.clone();
    entity.properties.remove("target");
    entity.properties.remove("origin");
    entity
}

#[test]
fn insertion_and_head_middle_tail_removal_keep_retained_identity_and_saved_data() {
    for actor in [Actor::User, Actor::Agent] {
        for classname in ["path_corner", "path_track"] {
            for closed in [false, true] {
                for removed in 0..3 {
                    let (mut session, ids) = fixture(classname, closed);
                    let retained = ids
                        .iter()
                        .copied()
                        .filter(|id| *id != ids[removed])
                        .collect::<Vec<_>>();
                    session
                        .edit(actor, "authored path data", |tx| {
                            let entity = tx.entity_mut(retained[0])?;
                            entity.properties.set("custom_key", "Keep authored data");
                            entity.properties.set("spawnflags", "1025");
                            entity.properties.set("wait", "3.7500");
                            entity.connections = Some(vec![Connection {
                                output: "OnPass".into(),
                                value: "relay,Trigger,,0.25,-1".into(),
                            }]);
                            map_core::edit::create_entity(
                                tx,
                                "info_target",
                                Some(DVec3::ZERO),
                                &[
                                    ("targetname".into(), "relay".into()),
                                    (
                                        "target".into(),
                                        format!(
                                            "route_{:03}",
                                            ids.iter().position(|id| *id == retained[0]).unwrap()
                                                + 1
                                        ),
                                    ),
                                ],
                                Vec::new(),
                            )?;
                            Ok(())
                        })
                        .unwrap();
                    let source = session.document().vmf().clone();
                    let history = session.document().undo_labels().count();
                    let selection = session.selected_ids();
                    let mut change = request(&session, &ids);
                    change.nodes.remove(removed);
                    change.nodes.reverse();
                    change.nodes[0].origin.z += 0.125;
                    change.nodes.insert(
                        1,
                        EditNode {
                            id: None,
                            origin: DVec3::new(192.0, 32.0, 48.0),
                        },
                    );
                    change.nodes.push(EditNode {
                        id: None,
                        origin: DVec3::new(256.0, 32.0, 48.0),
                    });
                    let state = session
                        .preview_linked_path_changes(
                            actor,
                            change.clone(),
                            &session.linked_path_stamp(),
                        )
                        .unwrap();
                    assert_eq!(
                        state["preview"]["seed"],
                        object_name(ObjectId::Entity(ids[0]))
                    );
                    assert_eq!(
                        state["preview"]["removed"],
                        json!([{"id":object_name(ObjectId::Entity(ids[removed])),"targetname":format!("route_{:03}", removed + 1)}])
                    );
                    assert_eq!(session.document().vmf(), &source);
                    assert_eq!(session.selected_ids(), selection);
                    assert_eq!(session.document().undo_labels().count(), history);
                    let expected = session.render_document().vmf().clone();
                    assert!(
                        !expected
                            .entities
                            .iter()
                            .any(|entity| entity.id == ids[removed])
                    );
                    let nodes = state["preview"]["nodes"].as_array().unwrap();
                    for (index, (node, requested)) in nodes.iter().zip(&change.nodes).enumerate() {
                        let planned = expected
                            .entities
                            .iter()
                            .find(|entity| node["id"] == object_name(ObjectId::Entity(entity.id)))
                            .unwrap();
                        assert_eq!(planned.origin(), Some(requested.origin));
                        let target = nodes.get(index + 1).or_else(|| closed.then(|| &nodes[0]));
                        assert_eq!(
                            planned.properties.get("target"),
                            target.map(|node| node["targetname"].as_str().unwrap())
                        );
                        if let Some(id) = requested.id {
                            assert_eq!(planned.id, id);
                            assert_eq!(node["source_id"], object_name(ObjectId::Entity(id)));
                            assert_eq!(
                                authored(planned),
                                authored(session.document().entity(id).unwrap())
                            );
                        } else {
                            assert!(node["source_id"].is_null());
                            assert!(!source.entities.iter().any(|entity| entity.id == planned.id));
                            assert_eq!(planned.properties.get("wait"), Some("2.5"));
                            assert_eq!(planned.properties.get("custom_key"), None);
                            assert!(planned.connections.as_ref().is_none_or(Vec::is_empty));
                        }
                    }
                    let result = session.apply_linked_path(actor, token(&state)).unwrap();
                    assert_eq!(
                        result["objects"],
                        json!(
                            nodes
                                .iter()
                                .map(|node| node["id"].clone())
                                .collect::<Vec<_>>()
                        )
                    );
                    assert_eq!(session.document().vmf(), &expected);
                    assert_eq!(session.document().undo_labels().count(), history + 1);
                    let directory = tempfile::tempdir().unwrap();
                    let path = directory.path().join("changed-path.vmf");
                    session.save(actor, Some(&path)).unwrap();
                    assert_eq!(
                        Vmf::parse(&std::fs::read_to_string(path).unwrap())
                            .unwrap()
                            .entities,
                        expected.entities
                    );
                    session.undo(actor).unwrap();
                    assert_eq!(session.document().vmf(), &source);
                    session.redo(actor).unwrap();
                    assert_eq!(session.document().vmf(), &expected);
                }
            }
        }
    }
}

#[test]
fn removal_rejects_connections_and_external_or_custom_references_without_replacing_a_draft() {
    for case in 0..9 {
        let (mut session, ids) = fixture("path_track", false);
        session
            .edit(Actor::User, "referenced path", |tx| {
                let ObjectId::Entity(external) = map_core::edit::create_entity(
                    tx,
                    "info_target",
                    Some(DVec3::ZERO),
                    &[],
                    Vec::new(),
                )?
                else {
                    unreachable!()
                };
                match case {
                    0 => {
                        tx.entity_mut(ids[1])?.connections = Some(vec![Connection {
                            output: "Custom".into(),
                            value: "opaque authored value".into(),
                        }])
                    }
                    1 => tx
                        .entity_mut(external)?
                        .properties
                        .set("target", "route_002"),
                    2 => tx.entity_mut(external)?.properties.set("target", "route_*"),
                    3 => tx
                        .entity_mut(external)?
                        .properties
                        .set("notes", "ROUTE_002"),
                    4 => {
                        tx.entity_mut(external)?.connections = Some(vec![Connection {
                            output: "OnTrigger".into(),
                            value: "route_002,EnablePath,,0,-1".into(),
                        }])
                    }
                    5 => {
                        tx.entity_mut(external)?.connections = Some(vec![Connection {
                            output: "OnTrigger".into(),
                            value: "!self,SetParent,route_002,0,-1".into(),
                        }])
                    }
                    6 => {
                        let entity = tx.entity_mut(external)?;
                        entity.hidden = true;
                        entity.properties.set("parentname", "route_002,attachment");
                    }
                    7 => tx
                        .entity_mut(ids[0])?
                        .properties
                        .set("custom_reference", "route_002"),
                    8 => {
                        tx.entity_mut(external)?.connections = Some(vec![Connection {
                            output: "Broken".into(),
                            value: "route_*,broken".into(),
                        }])
                    }
                    _ => unreachable!(),
                }
                Ok(())
            })
            .unwrap();
        if case == 2 {
            session.set_definitions(Actor::User, source_formats::fgd::Catalog::default());
        }
        let good = session
            .preview_linked_path_changes(
                Actor::Agent,
                request(&session, &ids),
                &session.linked_path_stamp(),
            )
            .unwrap();
        let source = session.document().vmf().clone();
        let rendered = session.render_document().vmf().clone();
        let history = session.document().undo_labels().count();
        let mut change = request(&session, &ids);
        change.nodes.remove(1);
        let error = session
            .preview_linked_path_changes(Actor::Agent, change, &session.linked_path_stamp())
            .unwrap_err();
        assert!(
            error.contains("Cannot remove route_002"),
            "case {case}: {error}"
        );
        assert_eq!(session.linked_path(), good);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.render_document().vmf(), &rendered);
        assert_eq!(session.document().undo_labels().count(), history);
    }
}

#[test]
fn inserted_names_avoid_hidden_collisions_and_reject_existing_reference_acquisition() {
    for case in 0..4 {
        let (mut session, ids) = fixture("path_corner", false);
        session
            .edit(Actor::User, "new name constraints", |tx| {
                let ObjectId::Entity(external) = map_core::edit::create_entity(
                    tx,
                    "info_target",
                    Some(DVec3::ZERO),
                    &[("targetname".into(), "INSERTED_001".into())],
                    Vec::new(),
                )?
                else {
                    unreachable!()
                };
                let entity = tx.entity_mut(external)?;
                entity.hidden = true;
                match case {
                    0 => {}
                    1 => entity.properties.set("target", "inserted*"),
                    2 => entity.properties.set("notes", "inserted_001_2"),
                    3 => {
                        entity.connections = Some(vec![Connection {
                            output: "OnTrigger".into(),
                            value: "inserted*,EnablePath,,0,-1".into(),
                        }])
                    }
                    _ => unreachable!(),
                }
                Ok(())
            })
            .unwrap();
        if case == 1 {
            session.set_definitions(Actor::User, source_formats::fgd::Catalog::default());
        }
        let good = session
            .preview_linked_path_changes(
                Actor::User,
                request(&session, &ids),
                &session.linked_path_stamp(),
            )
            .unwrap();
        let mut change = request(&session, &ids);
        change.nodes.insert(
            0,
            EditNode {
                id: None,
                origin: DVec3::new(-128.0, 0.0, 0.0),
            },
        );
        let result =
            session.preview_linked_path_changes(Actor::User, change, &session.linked_path_stamp());
        if case == 0 {
            assert_eq!(
                result.unwrap()["preview"]["nodes"][0]["targetname"],
                "inserted_001_2"
            );
        } else {
            assert!(
                result
                    .unwrap_err()
                    .contains("acquire an existing reference")
            );
            assert_eq!(session.linked_path(), good);
        }
    }
}

#[test]
fn membership_bounds_ownership_noop_and_stale_changes_preserve_source_and_history() {
    let (mut session, ids) = fixture("path_track", true);
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let original = request(&session, &ids);
    let good = session
        .preview_linked_path_changes(Actor::Agent, original.clone(), &session.linked_path_stamp())
        .unwrap();
    for case in 0..8 {
        let mut change = original.clone();
        match case {
            0 => change.nodes[1].id = change.nodes[0].id,
            1 => change.nodes[1].id = Some(u32::MAX),
            2 => change.nodes.iter_mut().for_each(|node| node.id = None),
            3 => change.nodes.truncate(1),
            4 => change.nodes.extend((0..62).map(|index| EditNode {
                id: None,
                origin: DVec3::new(index as f64, -128.0, 0.0),
            })),
            5 => change.nodes[1].origin = change.nodes[0].origin,
            6 => change.nodes[1].origin.x = f64::NAN,
            7 => change.seed = u32::MAX,
            _ => unreachable!(),
        }
        assert!(
            session
                .preview_linked_path_changes(Actor::Agent, change, &session.linked_path_stamp())
                .is_err(),
            "case {case}"
        );
        assert_eq!(session.linked_path(), good);
    }
    assert!(
        session
            .apply_linked_path(Actor::User, token(&good))
            .is_err()
    );
    let reviewed = session
        .review_linked_path(Actor::User, token(&good))
        .unwrap();
    assert!(
        session
            .cancel_linked_path(Actor::Agent, token(&reviewed))
            .is_err()
    );
    session
        .apply_linked_path(Actor::User, token(&reviewed))
        .unwrap();
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    let mut change = original;
    change.nodes.insert(
        1,
        EditNode {
            id: None,
            origin: DVec3::new(-64.0, 0.0, 0.0),
        },
    );
    let draft = session
        .preview_linked_path_changes(Actor::User, change.clone(), &session.linked_path_stamp())
        .unwrap();
    session
        .cancel_linked_path(Actor::User, token(&draft))
        .unwrap();
    assert_eq!(session.document().vmf(), &source);
    let draft = session
        .preview_linked_path_changes(Actor::User, change, &session.linked_path_stamp())
        .unwrap();
    session.definitions_serial += 1;
    assert!(
        session
            .apply_linked_path(Actor::User, token(&draft))
            .is_err()
    );
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    session
        .cancel_linked_path(Actor::User, token(&draft))
        .unwrap();
}

#[test]
fn changed_paths_accept_two_and_sixty_four_nodes_and_reject_parent_or_branch_defaults() {
    for count in [MIN_POINTS, MAX_POINTS] {
        let (mut session, ids) = fixture("path_corner", true);
        let source = session.document().vmf().clone();
        let mut change = request(&session, &ids);
        if count == MIN_POINTS {
            change.nodes.pop();
        } else {
            change.nodes.extend((3..count).map(|index| EditNode {
                id: None,
                origin: DVec3::new(index as f64 * 16.0, -128.0, 0.0),
            }));
        }
        let state = session
            .preview_linked_path_changes(Actor::User, change, &session.linked_path_stamp())
            .unwrap();
        assert_eq!(state["preview"]["nodes"].as_array().unwrap().len(), count);
        assert_eq!(state["preview"]["nodes"][count - 1]["target"], "route_001");
        session
            .apply_linked_path(Actor::User, token(&state))
            .unwrap();
        assert_eq!(session.document().vmf().entities.len(), count);
        session.undo(Actor::User).unwrap();
        assert_eq!(session.document().vmf(), &source);
    }
    for fgd in [
        "@SolidClass = path_corner []",
        "@PointClass = path_corner [ target(target_destination) readonly : \"Next\" ]",
        "@PointClass = path_corner [ altpath(target_destination) : \"Alternate\" : \"branch\" ]",
        "@PointClass = path_corner [ parentname(target_destination) : \"Parent\" : \"parent\" ]",
    ] {
        let (mut session, ids) = fixture("path_corner", false);
        session.set_definitions(
            Actor::User,
            source_formats::fgd::Catalog::parse(fgd).unwrap(),
        );
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let mut change = request(&session, &ids);
        change.nodes.insert(
            1,
            EditNode {
                id: None,
                origin: DVec3::new(-128.0, 0.0, 0.0),
            },
        );
        assert!(
            session
                .preview_linked_path_changes(Actor::User, change, &session.linked_path_stamp())
                .is_err(),
            "{fgd}"
        );
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
    }
}
