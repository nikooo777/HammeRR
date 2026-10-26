use super::*;
use crate::session::entity_graph::{Direction, Query, Traversal};

#[test]
fn entity_graph_tools_share_snapshot_selection_and_framing_operations() {
    let mut session = Session::new(Err("no assets".into()));
    let mut ids = Vec::new();
    for (name, x) in [("a", 0.0), ("b", 64.0)] {
        let object = session
            .create_entity(
                Actor::Agent,
                "logic_relay",
                Some(DVec3::new(x, 0.0, 64.0)),
                std::collections::BTreeMap::from([("targetname".into(), name.into())]),
            )
            .unwrap();
        let ObjectId::Entity(id) = object else {
            panic!()
        };
        ids.push(id);
    }
    session
        .edit(Actor::Agent, "graph link", |tx| {
            map_core::connections::set(
                tx,
                ids[0],
                None,
                &map_core::connections::Spec {
                    output: "OnTrigger".into(),
                    target: "b".into(),
                    input: "Trigger".into(),
                    parameter: String::new(),
                    delay: 0.0,
                    times: -1,
                },
            )?;
            Ok(())
        })
        .unwrap();
    let traversal = Traversal {
        roots: vec![ids[0]],
        direction: Direction::Outgoing,
        depth: 2,
    };
    let expected = session
        .inspect_entity_graph(&Query {
            snapshot: None,
            traversal: traversal.clone(),
            search: String::new(),
            node_offset: 0,
            edge_offset: 0,
            unresolved_offset: 0,
            limit: 1,
        })
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    for name in [
        "inspect_entity_graph",
        "select_entity_graph",
        "frame_entity_graph",
    ] {
        assert!(server.get_tool(name).is_some());
    }
    let result = server.inspect_entity_graph(Parameters(serde_json::from_value(json!({"roots":[id_string(ObjectId::Entity(ids[0]))],"direction":"outgoing","limit":1})).unwrap())).unwrap();
    let actual: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(actual, json!(expected));
    let result = server.select_entity_graph(Parameters(serde_json::from_value(json!({"roots":[id_string(ObjectId::Entity(ids[0]))],"direction":"outgoing","snapshot":expected.snapshot})).unwrap())).unwrap();
    assert!(!result.is_error.unwrap_or(false));
    assert_eq!(lock(&shared).selected_ids().len(), 2);
    let before = lock(&shared).selected_ids();
    let result = server.frame_entity_graph(Parameters(serde_json::from_value(json!({"roots":[id_string(ObjectId::Entity(ids[0]))],"direction":"outgoing","snapshot":expected.snapshot,"aspect":1.5})).unwrap())).unwrap();
    assert!(!result.is_error.unwrap_or(false));
    assert_eq!(lock(&shared).selected_ids(), before);
    let result = server.select_entity_graph(Parameters(serde_json::from_value(json!({"roots":[id_string(ObjectId::Entity(ids[0]))],"direction":"incoming","snapshot":expected.snapshot})).unwrap())).unwrap();
    assert!(result.is_error.unwrap_or(false));
    assert_eq!(lock(&shared).selected_ids(), before);
    assert!(
        serde_json::from_value::<EntityGraphArgs>(
            json!({"roots":["entity:1"],"direction":"outgoing","path":"map.vmf"})
        )
        .is_err()
    );
}
