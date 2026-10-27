use super::super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn terrain_fixture() -> (SharedSession, EditorServer, Value) {
    let mut session = Session::new(Err("no assets needed".into()));
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(128.0),
            "test",
            false,
        )
        .unwrap()
    else {
        panic!()
    };
    let side = session
        .document()
        .solid(solid)
        .unwrap()
        .sides
        .iter()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let face = map_core::surface::FaceId { solid, side };
    session.create_terrain(Actor::Agent, &[face], 3, 2).unwrap();
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let sculpt = json!({
        "faces":[{"solid":solid,"side":side}],
        "mode":"raise", "center":[64.0,64.0,128.0], "radius":32.0, "amount":4.0,
    });
    (shared, server, sculpt)
}

#[test]
fn select_protocol_reports_cleared_handles_and_preserves_noop_previews() {
    let mut session = Session::new(Err("no assets needed".into()));
    let source = session
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(128.0),
            "test",
            false,
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let stamp = lock(&shared).hollow_stamp();
    let hollow = value(
        server.preview_hollow(Parameters(
            serde_json::from_value(json!({
                "thickness":8.0,"stamp":stamp,
            }))
            .unwrap(),
        )),
    );
    assert_eq!(hollow["handle"]["kind"], "hollow");
    assert_eq!(hollow["handle"]["token"], hollow["preview"]["token"]);
    let memory = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    for result in [
        server.delete_objects(Parameters(IdsArgs {
            ids: Some(Vec::new()),
        })),
        server.transform_objects(Parameters(
            serde_json::from_value(json!({"ids":[],"translation":[32,0,0],"preview":true}))
                .unwrap(),
        )),
    ] {
        let result = result.unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(
            result.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("explicit ids must not be empty")
        );
    }
    assert_eq!(lock(&shared).document().vmf(), &memory);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(lock(&shared).selected_ids(), [source]);
    assert_eq!(
        value(server.get_hollow_preview())["handle"],
        hollow["handle"]
    );
    let unchanged = value(server.select(Parameters(
        serde_json::from_value(json!({"ids":[id_string(source)]})).unwrap(),
    )));
    assert_eq!(unchanged["cleared_preview_kinds"], json!([]));
    assert_eq!(
        value(server.get_hollow_preview())["handle"],
        hollow["handle"]
    );
    let snapshot = lock(&shared).repeat_stamp();
    let array = value(
        server.preview_repeat_array(Parameters(
            serde_json::from_value(json!({
                "count":2,"translation":[256.0,0.0,0.0],"snapshot":snapshot,
            }))
            .unwrap(),
        )),
    );
    assert_eq!(array["handle"]["kind"], "repeat_array");
    assert_eq!(
        array["handle"]["token"],
        array["preview"]["token"].as_u64().unwrap().to_string()
    );
    assert!(value(server.get_hollow_preview())["handle"].is_null());
    let cleared = value(server.select(Parameters(
        serde_json::from_value(json!({"ids":[]})).unwrap(),
    )));
    assert_eq!(cleared["cleared_preview_kinds"], json!(["repeat_array"]));
    assert!(value(server.get_repeat_array())["handle"].is_null());
    let stale = server
        .apply_repeat_array(Parameters(
            serde_json::from_value(json!({"token":array["preview"]["token"]})).unwrap(),
        ))
        .unwrap();
    assert!(stale.is_error.unwrap_or(false));
}

#[test]
fn terrain_protocol_handles_follow_stroke_and_face_owners_through_finish_and_cancel() {
    let (shared, server, sculpt) = terrain_fixture();
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let preview = value(
        server.preview_terrain_sculpt(Parameters(serde_json::from_value(sculpt.clone()).unwrap())),
    );
    assert_eq!(preview["handle"], preview["state"]["handle"]);
    assert_eq!(preview["handle"]["kind"], "face_preview");
    assert_eq!(
        preview["handle"]["token"],
        preview["state"]["preview_token"]
    );
    let cancelled = value(server.cancel_face_preview(Parameters(
        serde_json::from_value(json!({"token":preview["handle"]["token"]})).unwrap(),
    )));
    assert!(cancelled["handle"].is_null());

    let begun = value(server.begin_terrain_stroke(Parameters(
        serde_json::from_value(json!({"sculpt":sculpt})).unwrap(),
    )));
    assert_eq!(begun["handle"]["kind"], "terrain_stroke");
    assert_eq!(begun["handle"]["token"], begun["token"]);
    let update = json!({"token":begun["token"],"points":[[64.0,64.0,128.0]]});
    let updated = value(
        server.update_terrain_stroke(Parameters(serde_json::from_value(update.clone()).unwrap())),
    );
    assert_eq!(updated["handle"], begun["handle"]);
    assert_eq!(updated["stroke"]["handle"], begun["handle"]);
    assert_eq!(updated["state"]["handle"]["kind"], "face_preview");
    assert_eq!(
        updated["state"]["handle"]["token"],
        updated["state"]["preview_token"]
    );
    let finished = value(server.finish_terrain_stroke(Parameters(
        serde_json::from_value(json!({"token":begun["token"]})).unwrap(),
    )));
    assert_eq!(finished["handle"], updated["state"]["handle"]);
    assert_eq!(finished["state"]["handle"], finished["handle"]);
    assert!(finished["stroke"]["handle"].is_null());
    let stale = server
        .update_terrain_stroke(Parameters(serde_json::from_value(update).unwrap()))
        .unwrap();
    assert!(stale.is_error.unwrap_or(false));
    let cancelled = value(server.cancel_face_preview(Parameters(
        serde_json::from_value(json!({"token":finished["handle"]["token"]})).unwrap(),
    )));
    assert!(cancelled["handle"].is_null());
    assert!(value(server.get_terrain_stroke())["handle"].is_null());
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
}

#[test]
fn stale_terrain_protocol_handle_cannot_claim_or_cancel_a_foreign_face_preview() {
    let (_shared, server, sculpt) = terrain_fixture();
    let begun = value(server.begin_terrain_stroke(Parameters(
        serde_json::from_value(json!({"sculpt":sculpt})).unwrap(),
    )));
    let update = json!({"token":begun["token"],"points":[[64.0,64.0,128.0]]});
    value(
        server.update_terrain_stroke(Parameters(serde_json::from_value(update.clone()).unwrap())),
    );
    let foreign =
        value(server.preview_terrain_sculpt(Parameters(serde_json::from_value(sculpt).unwrap())));
    let stale = value(server.get_terrain_stroke());
    assert_eq!(stale["stale"], true);
    assert!(stale["handle"].is_null());
    let rejected = server
        .update_terrain_stroke(Parameters(serde_json::from_value(update).unwrap()))
        .unwrap();
    assert!(rejected.is_error.unwrap_or(false));
    let cancelled = value(server.cancel_terrain_stroke(Parameters(
        serde_json::from_value(json!({"token":begun["token"]})).unwrap(),
    )));
    assert!(cancelled["handle"].is_null());
    assert!(cancelled["stroke"]["handle"].is_null());
    assert_eq!(cancelled["state"]["handle"], foreign["handle"]);
}

#[test]
fn property_and_connection_protocol_handles_preserve_legacy_tokens_and_closed_results() {
    let mut session = Session::new(Err("no assets needed".into()));
    let entity = session
        .create_entity(
            Actor::Agent,
            "logic_relay",
            Some(DVec3::ZERO),
            BTreeMap::new(),
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared, None);
    let property = value(server.begin_property_edit(Parameters(
        serde_json::from_value(json!({"world":true})).unwrap(),
    )));
    assert_eq!(property["handle"]["kind"], "property_edit");
    assert_eq!(
        property["handle"]["token"],
        property["draft"]["token"].as_u64().unwrap().to_string()
    );
    let updated = value(server.update_property_edit(Parameters(
        serde_json::from_value(json!({"token":property["draft"]["token"],"changes":[]})).unwrap(),
    )));
    assert_eq!(updated["handle"], property["handle"]);
    let connection = value(server.begin_connection_edit(Parameters(
        serde_json::from_value(json!({"id":id_string(entity)})).unwrap(),
    )));
    assert_eq!(connection["handle"]["kind"], "connection_edit");
    assert_eq!(
        connection["handle"]["token"],
        connection["token"].as_u64().unwrap().to_string()
    );
    let updated = value(server.update_connection_draft(Parameters(serde_json::from_value(json!({"token":connection["token"],"output":"OnTrigger","target":"!self","input":"Trigger"})).unwrap())));
    assert_eq!(updated["handle"], connection["handle"]);
    value(server.select(Parameters(
        serde_json::from_value(json!({"ids":[]})).unwrap(),
    )));
    assert_eq!(
        value(server.get_property_edit())["handle"],
        property["handle"]
    );
    let closed = value(server.cancel_property_edit(Parameters(
        serde_json::from_value(json!({"token":property["draft"]["token"]})).unwrap(),
    )));
    assert!(closed["handle"].is_null());
    let closed = value(server.commit_connection_edit(Parameters(
        serde_json::from_value(json!({"token":connection["token"]})).unwrap(),
    )));
    assert!(closed["handle"].is_null());
}
