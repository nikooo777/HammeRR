use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn mcp_and_human_actions_share_an_owned_rendered_path_and_undo() {
    let session = crate::session::linked_path::tests::fixture();
    let source = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let state = value(server.get_linked_path());
    let preview = value(
        server.set_linked_path(Parameters(
            serde_json::from_value(json!({
                "action":"preview","classname":"path_track","prefix":"rail",
                "points":[[0,0,0],[64,0,0],[64,64,8]],"closed":true,"snapshot":state["snapshot"],
            }))
            .unwrap(),
        )),
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    let expected = lock(&shared).render_document().vmf().clone();
    let old = preview["preview"]["token"].as_u64().unwrap();
    let reviewed = lock(&shared).review_linked_path(Actor::User, old).unwrap();
    let token = reviewed["preview"]["token"].as_u64().unwrap();
    for args in [
        LinkedPathArgs::Apply { token },
        LinkedPathArgs::Cancel { token },
    ] {
        assert!(
            server
                .set_linked_path(Parameters(args))
                .unwrap()
                .is_error
                .unwrap_or(false)
        );
    }
    let reviewed = value(server.set_linked_path(Parameters(LinkedPathArgs::Review { token })));
    let token = reviewed["preview"]["token"].as_u64().unwrap();
    let applied = value(server.set_linked_path(Parameters(LinkedPathArgs::Apply { token })));
    assert_eq!(applied["objects"].as_array().unwrap().len(), 3);
    assert_eq!(lock(&shared).document().vmf(), &expected);
    assert_eq!(lock(&shared).document().undo_labels().count(), 1);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}

#[test]
fn mcp_inspects_and_edits_existing_nodes_through_the_shared_owned_preview() {
    let (session, ids) = crate::session::linked_path::existing_tests::fixture("path_track", false);
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let inspected = value(server.set_linked_path(Parameters(LinkedPathArgs::Inspect {
        id: format!("entity:{}", ids[1]),
    })));
    assert_eq!(inspected["nodes"][0]["targetname"], "route_001");
    let mut nodes = inspected["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| json!({"id":node["id"],"origin":node["origin"]}))
        .collect::<Vec<_>>();
    nodes.reverse();
    nodes[1]["origin"][2] = json!(40.5);
    let state = value(
        server.set_linked_path(Parameters(
            serde_json::from_value(
                json!({"action":"edit","nodes":nodes,"snapshot":inspected["snapshot"]}),
            )
            .unwrap(),
        )),
    );
    assert_eq!(state["preview"]["existing"], true);
    assert_eq!(lock(&shared).document().vmf(), &source);
    let expected = lock(&shared).render_document().vmf().clone();
    let token = state["preview"]["token"].as_u64().unwrap();
    let reviewed = lock(&shared)
        .review_linked_path(Actor::User, token)
        .unwrap();
    assert!(
        server
            .set_linked_path(Parameters(LinkedPathArgs::Apply { token }))
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    let reviewed = value(server.set_linked_path(Parameters(LinkedPathArgs::Review {
        token: reviewed["preview"]["token"].as_u64().unwrap(),
    })));
    value(server.set_linked_path(Parameters(LinkedPathArgs::Apply {
        token: reviewed["preview"]["token"].as_u64().unwrap(),
    })));
    assert_eq!(lock(&shared).document().vmf(), &expected);
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
