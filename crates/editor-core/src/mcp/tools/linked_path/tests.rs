use super::*;

#[test]
fn linked_path_surface_picks_share_exact_owned_rows_and_one_undo_without_graphics() {
    let mut session = crate::session::linked_path::tests::fixture();
    let floor = session
        .create_block(
            Actor::User,
            DVec3::new(-128.0, -128.0, -16.0),
            DVec3::new(128.0, 128.0, 0.0),
            "dev/test",
            false,
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let call = |arguments: Value| {
        value(server.set_linked_path(Parameters(serde_json::from_value(arguments).unwrap())))
    };
    let state = value(server.get_linked_path());
    let draft = call(
        json!({"action":"preview","classname":"path_corner","prefix":"surface","points":[[-64,0,16],[64,0,16]],"snapshot":state["snapshot"]}),
    );
    let armed = call(
        json!({"action":"begin_pick","token":draft["preview"]["token"],"row":1,"width":511,"height":511,"camera":{"projection":"top","center":[3.125,-5.25,0],"ortho_height":256},"snapshot":draft["pick_snapshot"]}),
    );
    let token = armed["preview"]["token"].as_u64().unwrap();
    assert_eq!(armed["preview"]["picking"]["row"], 1);
    let miss: LinkedPathArgs = serde_json::from_value(
        json!({"action":"pick_ray","token":token,"origin":[1000,1000,1000],"direction":[0,0,1]}),
    )
    .unwrap();
    assert!(
        server
            .set_linked_path(Parameters(miss))
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    assert_eq!(value(server.get_linked_path()), armed);
    let picked = call(
        json!({"action":"pick_pixel","token":token,"x":255.5,"y":255.5,"width":511,"height":511}),
    );
    assert_eq!(picked["preview"]["nodes"][0], draft["preview"]["nodes"][0]);
    assert_eq!(
        picked["preview"]["nodes"][1]["origin"],
        json!([3.125, -5.25, 0.0])
    );
    assert_eq!(
        picked["preview"]["last_pick"]["target"],
        crate::identity::id_string(floor)
    );
    assert!(picked["preview"]["picking"].is_null());
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    assert_eq!(lock(&shared).camera(), camera);
    let old: LinkedPathArgs =
        serde_json::from_value(json!({"action":"cancel_pick","token":token})).unwrap();
    assert!(
        server
            .set_linked_path(Parameters(old))
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    let expected = lock(&shared).render_document().vmf().clone();
    let user = lock(&shared)
        .review_linked_path(Actor::User, picked["preview"]["token"].as_u64().unwrap())
        .unwrap();
    let foreign: LinkedPathArgs = serde_json::from_value(json!({"action":"begin_pick","token":user["preview"]["token"],"row":0,"width":511,"height":511,"snapshot":user["pick_snapshot"]})).unwrap();
    assert!(
        server
            .set_linked_path(Parameters(foreign))
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    let user_pick = {
        let mut session = lock(&shared);
        let snapshot = session.linked_path_pick_stamp();
        session
            .begin_linked_path_pick(
                Actor::User,
                user["preview"]["token"].as_u64().unwrap(),
                0,
                camera,
                (511, 511),
                &snapshot,
            )
            .unwrap()
    };
    let foreign_cancel: LinkedPathArgs = serde_json::from_value(
        json!({"action":"cancel_pick","token":user_pick["preview"]["token"]}),
    )
    .unwrap();
    assert!(
        server
            .set_linked_path(Parameters(foreign_cancel))
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    assert_eq!(value(server.get_linked_path()), user_pick);
    let reviewed = call(json!({"action":"review","token":user_pick["preview"]["token"]}));
    call(json!({"action":"apply","token":reviewed["preview"]["token"]}));
    assert_eq!(lock(&shared).document().vmf(), &expected);
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}

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

#[test]
fn mcp_changes_path_membership_with_explicit_source_ids_and_deletion_review() {
    let (session, ids) =
        crate::session::linked_path::node_changes_tests::fixture("path_track", true);
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let inspected = value(server.set_linked_path(Parameters(LinkedPathArgs::Inspect {
        id: format!("entity:{}", ids[0]),
    })));
    let args = json!({
        "action":"edit", "seed":inspected["seed"], "prefix":"inserted",
        "snapshot":inspected["snapshot"],
        "nodes":[
            {"id":format!("entity:{}",ids[1]),"origin":[64,0,0]},
            {"origin":[96,0,0]},
            {"id":format!("entity:{}",ids[2]),"origin":[128,0,0]},
        ],
    });
    let draft = value(server.set_linked_path(Parameters(serde_json::from_value(args).unwrap())));
    assert_eq!(
        draft["preview"]["removed"],
        json!([{"id":format!("entity:{}",ids[0]),"targetname":"route_001"}])
    );
    assert_eq!(
        draft["preview"]["nodes"][0]["source_id"],
        format!("entity:{}", ids[1])
    );
    assert!(draft["preview"]["nodes"][1]["source_id"].is_null());
    assert_eq!(draft["preview"]["nodes"][1]["targetname"], "inserted_001");
    assert_eq!(lock(&shared).document().vmf(), &source);
    let expected = lock(&shared).render_document().vmf().clone();
    let reviewed = lock(&shared)
        .review_linked_path(Actor::User, draft["preview"]["token"].as_u64().unwrap())
        .unwrap();
    assert_eq!(reviewed["preview"]["nodes"], draft["preview"]["nodes"]);
    lock(&shared)
        .apply_linked_path(Actor::User, reviewed["preview"]["token"].as_u64().unwrap())
        .unwrap();
    assert_eq!(lock(&shared).document().vmf(), &expected);
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
}
