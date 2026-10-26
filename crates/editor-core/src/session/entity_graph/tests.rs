use super::*;
use glam::DVec3;
use serde_json::json;
use source_formats::vmf::{Connection, Entity};

fn fixture() -> Session {
    let mut session = Session::new(Err("graph needs no assets".into()));
    session
        .edit(Actor::User, "authored branching graph", |tx| {
            for (id, name, targets) in [
                (
                    10,
                    "start",
                    vec!["branch", "ghost", "branch*", "!activator", "logic_relay"],
                ),
                (20, "branch", vec!["end"]),
                (21, "branch", vec!["!self", "end"]),
                (30, "end", vec!["start"]),
            ] {
                let mut entity = Entity::new(id, "logic_relay");
                entity.properties.set("targetname", name);
                entity.properties.set("origin", format!("{id} 0 64"));
                entity.connections = Some(
                    targets
                        .into_iter()
                        .map(|target| Connection {
                            output: "OnTrigger".into(),
                            value: format!("{target},Trigger,,0,-1"),
                        })
                        .collect(),
                );
                tx.add_entity(entity)?;
            }
            let mut hidden = Entity::new(40, "logic_relay");
            hidden.hidden = true;
            hidden.properties.set("targetname", "branch");
            tx.add_entity(hidden)?;
            tx.entity_mut(10)?
                .connections
                .as_mut()
                .unwrap()
                .push(Connection {
                    output: "Broken".into(),
                    value: "branch,Trigger,,NaN,-1".into(),
                });
            Ok(())
        })
        .unwrap();
    session
}
fn traversal() -> Traversal {
    Traversal {
        roots: vec![10],
        direction: Direction::Outgoing,
        depth: 2,
    }
}
fn query() -> Query {
    Query {
        snapshot: None,
        traversal: traversal(),
        search: String::new(),
        node_offset: 0,
        edge_offset: 0,
        unresolved_offset: 0,
        limit: MAX_PAGE,
    }
}
fn preserved(session: &Session) -> serde_json::Value {
    json!({"map":session.document().vmf().write(&Default::default()),"history":session.history_snapshot(),"selection":session.selected_ids().into_iter().map(crate::identity::id_string).collect::<Vec<_>>(),"camera":format!("{:?}",session.camera()),"scene":session.scene_serial(),"camera_serial":session.camera_serial(),"property":session.property_draft()})
}

#[test]
fn graph_preserves_exact_named_fanout_cycles_unresolved_records_and_page_identity() {
    let session = fixture();
    let before = preserved(&session);
    let report = session.inspect_entity_graph(&query()).unwrap();
    assert_eq!(preserved(&session), before);
    assert_eq!((report.traversal_nodes, report.traversal_edges), (4, 6));
    assert_eq!(
        report
            .nodes
            .rows
            .iter()
            .map(|node| (node.id.as_str(), node.depth))
            .collect::<Vec<_>>(),
        [
            ("entity:10", 0),
            ("entity:20", 1),
            ("entity:21", 1),
            ("entity:30", 2)
        ]
    );
    let edges = report
        .edges
        .rows
        .iter()
        .map(|edge| {
            (
                edge.source.as_str(),
                edge.target.as_str(),
                edge.connection_index,
            )
        })
        .collect::<Vec<_>>();
    assert!(
        edges.contains(&("entity:10", "entity:20", 0))
            && edges.contains(&("entity:10", "entity:21", 0))
    );
    assert!(
        edges.contains(&("entity:21", "entity:21", 0))
            && edges.contains(&("entity:30", "entity:10", 0))
    );
    let unresolved = &report.unresolved.rows;
    assert_eq!(
        unresolved.iter().map(|row| row.status).collect::<Vec<_>>(),
        [
            "missing",
            "wildcard_candidates",
            "runtime_dependent",
            "class_or_runtime",
            "invalid"
        ]
    );
    assert_eq!(unresolved[1].candidates, ["entity:20", "entity:21"]);
    let mut page = query();
    page.limit = 1;
    page.node_offset = 1;
    assert!(session.inspect_entity_graph(&page).is_err());
    page.snapshot = Some(report.snapshot.clone());
    assert_eq!(
        session.inspect_entity_graph(&page).unwrap().nodes.rows[0].id,
        "entity:20"
    );
    page.traversal.direction = Direction::Incoming;
    assert!(session.inspect_entity_graph(&page).is_err());
    page.snapshot = None;
    page.node_offset = 0;
    page.traversal.roots = vec![30];
    let incoming = session.inspect_entity_graph(&page).unwrap();
    assert_eq!(incoming.traversal_nodes, 4);
    page.traversal.direction = Direction::Both;
    page.traversal.depth = 1;
    assert_eq!(
        session.inspect_entity_graph(&page).unwrap().traversal_nodes,
        4
    );
}

#[test]
fn complete_selection_and_framing_revalidate_owned_tokens_and_refuse_atomically() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = fixture();
        let mut filtered = query();
        filtered.limit = 1;
        filtered.search = "start".into();
        let report = session.inspect_entity_graph(&filtered).unwrap();
        assert_eq!(report.nodes.rows.len(), 1);
        let selected = session
            .select_entity_graph(actor, &traversal(), &report.snapshot, SelectMode::Replace)
            .unwrap();
        assert_eq!(selected.matched, 4);
        assert_eq!(selected.selected_count, 4);
        assert_eq!(
            session.selected_ids(),
            [10, 20, 21, 30].map(ObjectId::Entity)
        );
        session
            .select_entity_graph(actor, &traversal(), &report.snapshot, SelectMode::Toggle)
            .unwrap();
        assert!(session.selected_ids().is_empty());
        session
            .select(&[ObjectId::Entity(40)], SelectMode::Replace)
            .unwrap();
        let added = session
            .select_entity_graph(actor, &traversal(), &report.snapshot, SelectMode::Add)
            .unwrap();
        assert_eq!((added.matched, added.selected_count), (4, 5));
        session
            .select_entity_graph(actor, &traversal(), &report.snapshot, SelectMode::Remove)
            .unwrap();
        assert_eq!(session.selected_ids(), [ObjectId::Entity(40)]);
        session
            .select(&[ObjectId::Entity(10)], SelectMode::Replace)
            .unwrap();
        let selected = session.selected_ids();
        let map = session.document().vmf().clone();
        session
            .frame_entity_graph(actor, &traversal(), &report.snapshot, 1.5)
            .unwrap();
        assert_eq!(session.selected_ids(), selected);
        assert_eq!(session.document().vmf(), &map);
        let before = preserved(&session);
        assert!(
            session
                .select_entity_graph(actor, &traversal(), &"0".repeat(64), SelectMode::Replace)
                .is_err()
        );
        assert!(
            session
                .frame_entity_graph(actor, &traversal(), &report.snapshot, f32::NAN)
                .is_err()
        );
        assert_eq!(preserved(&session), before);
        session
            .preview_block(DVec3::ZERO, DVec3::splat(32.0))
            .unwrap();
        let draft = session.block_preview();
        let before = preserved(&session);
        assert!(
            session
                .select_entity_graph(actor, &traversal(), &report.snapshot, SelectMode::Replace)
                .is_err()
        );
        assert!(
            session
                .frame_entity_graph(actor, &traversal(), &report.snapshot, 1.0)
                .is_err()
        );
        assert_eq!(preserved(&session), before);
        assert_eq!(session.block_preview(), draft);
        session.cancel_block_preview();
        let foreign = fixture();
        let foreign_report = foreign.inspect_entity_graph(&query()).unwrap();
        let before = preserved(&session);
        assert!(
            session
                .select_entity_graph(
                    actor,
                    &traversal(),
                    &foreign_report.snapshot,
                    SelectMode::Add
                )
                .is_err()
        );
        assert_eq!(preserved(&session), before);
        session
            .edit(actor, "change graph", |tx| {
                tx.entity_mut(20)?.properties.set("targetname", "renamed");
                Ok(())
            })
            .unwrap();
        let before = preserved(&session);
        assert!(
            session
                .select_entity_graph(actor, &traversal(), &report.snapshot, SelectMode::Replace)
                .is_err()
        );
        assert_eq!(preserved(&session), before);
        let mut invalid = query();
        invalid.traversal.depth = MAX_DEPTH + 1;
        assert!(session.inspect_entity_graph(&invalid).is_err());
        invalid = query();
        invalid.traversal.roots = vec![40];
        assert!(session.inspect_entity_graph(&invalid).is_err());
        assert_eq!(preserved(&session), before);
        for origin in [None, Some("NaN 0 64"), Some("inf 0 64")] {
            session
                .edit(actor, "unavailable graph root bounds", |tx| {
                    let entity = tx.entity_mut(20)?;
                    entity.properties.remove("origin");
                    if let Some(origin) = origin {
                        entity.properties.set("origin", origin);
                    }
                    Ok(())
                })
                .unwrap();
            let mut mixed = query();
            mixed.traversal.roots = vec![10, 20];
            let report = session.inspect_entity_graph(&mixed).unwrap();
            assert!(
                report
                    .nodes
                    .rows
                    .iter()
                    .any(|node| node.id == "entity:20" && node.root)
            );
            let before = preserved(&session);
            let error = session
                .frame_entity_graph(actor, &mixed.traversal, &report.snapshot, 1.5)
                .err()
                .unwrap();
            assert!(
                error.contains("entity 20") && error.contains("bounds"),
                "{error}"
            );
            assert_eq!(preserved(&session), before);
        }
    }
}

#[test]
fn graph_node_admission_refuses_large_fanout_before_selection_or_partial_report() {
    let mut session = Session::new(Err("no assets".into()));
    session
        .edit(Actor::User, "over-limit named fan-out", |tx| {
            let mut source = Entity::new(10, "logic_relay");
            source.connections = Some(vec![Connection {
                output: "OnTrigger".into(),
                value: "branch,Trigger,,0,-1".into(),
            }]);
            tx.add_entity(source)?;
            for id in 20..20 + MAX_GRAPH_NODES as u32 {
                let mut target = Entity::new(id, "logic_relay");
                target.properties.set("targetname", "branch");
                tx.add_entity(target)?;
            }
            Ok(())
        })
        .unwrap();
    session
        .select(&[ObjectId::Entity(10)], SelectMode::Replace)
        .unwrap();
    let before = preserved(&session);
    assert!(
        session
            .inspect_entity_graph(&query())
            .unwrap_err()
            .contains("node limit")
    );
    let snapshot = session.entity_graph_snapshot(&traversal(), &[10]);
    assert!(
        session
            .select_entity_graph(Actor::Agent, &traversal(), &snapshot, SelectMode::Replace)
            .unwrap_err()
            .contains("node limit")
    );
    assert_eq!(preserved(&session), before);
}
