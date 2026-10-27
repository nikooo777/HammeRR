use super::*;

fn report(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn water_current_mcp_exposes_numeric_world_field_and_forwards_staged_atomic_edits() {
    let key = compiler::WATER_CURRENT_VELOCITY_KEY;
    let shared = Arc::new(Mutex::new(Session::new(Err(
        "authoring needs no assets".into()
    ))));
    let server = EditorServer::new(shared.clone(), None);
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let inspected = report(server.get_property_inspector(Parameters(
        serde_json::from_value(json!({"world":true})).unwrap(),
    )));
    let field = inspected["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["field"]["key"] == key)
        .unwrap();
    assert_eq!(field["field"]["control"], "vector");
    assert_eq!(field["field"]["label"], "Liquid current · world units/s");
    assert_eq!(field["field"]["components"], json!(["0", "0", "0"]));
    assert_eq!(field["field"]["component_labels"], json!(["X", "Y", "Z"]));
    assert_eq!(field["stored_count"], 0);
    assert!(field["values"][0]["stored"].is_null());
    assert_eq!(field["values"][0]["effective"], "0 0 0");
    let opened = report(server.begin_property_edit(Parameters(
        serde_json::from_value(json!({"world":true})).unwrap(),
    )));
    let token = opened["draft"]["token"].as_u64().unwrap();
    let staged = report(
        server.update_property_edit(Parameters(
            serde_json::from_value(json!({"token":token,"changes":[
                {"action":"component","key":key,"index":0,"value":64},
                {"action":"component","key":key,"index":1,"value":-16}
            ]}))
            .unwrap(),
        )),
    );
    assert_eq!(staged, report(server.get_property_edit()));
    let expected = lock(&shared).preview_property_edit().unwrap();
    assert_eq!(staged["preview"], serde_json::to_value(expected).unwrap());
    let failed = server
        .update_property_edit(Parameters(
            serde_json::from_value(json!({"token":token,"changes":[
                {"action":"set","key":"skyname","value":"must_not_apply"},
                {"action":"set","key":key,"value":"100 100 0"}
            ]}))
            .unwrap(),
        ))
        .unwrap();
    assert!(failed.is_error.unwrap_or(false));
    assert_eq!(report(server.get_property_edit()), staged);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    report(server.cancel_property_edit(Parameters(PropertyTokenArgs { token })));
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    let opened = report(server.begin_property_edit(Parameters(
        serde_json::from_value(json!({"world":true})).unwrap(),
    )));
    let token = opened["draft"]["token"].as_u64().unwrap();
    report(
        server.update_property_edit(Parameters(
            serde_json::from_value(json!({"token":token,"changes":[
                {"action":"set","key":key,"value":"64 -16 0"}
            ]}))
            .unwrap(),
        )),
    );
    report(server.commit_property_edit(Parameters(PropertyTokenArgs { token })));
    assert_eq!(
        lock(&shared).document().vmf().world.properties.get(key),
        Some("64 -16 0")
    );
    assert_eq!(lock(&shared).document().undo_labels().count(), 1);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(server.rendering_status()["initialized"], false);
}
