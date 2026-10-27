use super::*;

fn report(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn damping(inspected: &Value) -> &Value {
    inspected["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|property| property["field"]["key"] == compiler::WATER_DAMPING_KEY)
        .unwrap()
}

#[test]
fn water_damping_mcp_reports_intrinsic_absence_and_preserves_atomic_raw_scalar_drafts() {
    let key = compiler::WATER_DAMPING_KEY;
    let raw = " 5e-3 ";
    let shared = Arc::new(Mutex::new(Session::new(Err(
        "authoring needs no assets".into()
    ))));
    let server = EditorServer::new(shared.clone(), None);
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let inspected = report(server.get_property_inspector(Parameters(
        serde_json::from_value(json!({"world":true})).unwrap(),
    )));
    let field = damping(&inspected);
    assert_eq!(field["field"]["label"], "Liquid pressure damping");
    assert_eq!(field["field"]["kind"], "float");
    assert_eq!(field["field"]["control"], "number");
    assert_eq!(field["field"]["range"], json!([0.0, 0.01]));
    assert_eq!(field["field"]["value"], "0.01");
    assert_eq!(field["field"]["components"], json!([]));
    assert_eq!(field["stored_count"], 0);
    assert_eq!(field["can_reset"], false);
    assert!(field["values"][0]["stored"].is_null());
    assert!(field["values"][0]["default"].is_null());
    assert_eq!(field["values"][0]["effective"], "0.01");
    let opened = report(server.begin_property_edit(Parameters(
        serde_json::from_value(json!({"world":true})).unwrap(),
    )));
    let token = opened["draft"]["token"].as_u64().unwrap();
    let staged = report(
        server.update_property_edit(Parameters(
            serde_json::from_value(json!({"token":token,"changes":[
                {"action":"set","key":key,"value":raw}
            ]}))
            .unwrap(),
        )),
    );
    assert_eq!(staged, report(server.get_property_edit()));
    assert_eq!(damping(&staged["preview"])["field"]["control"], "number");
    assert_eq!(damping(&staged["preview"])["field"]["value"], raw);
    assert_eq!(
        staged["preview"],
        serde_json::to_value(lock(&shared).preview_property_edit().unwrap()).unwrap()
    );
    for invalid in ["1e-9999", "1e-46", "0.010000000000000002"] {
        let failed = server
            .update_property_edit(Parameters(
                serde_json::from_value(json!({"token":token,"changes":[
                    {"action":"set","key":"skyname","value":"must_not_apply"},
                    {"action":"set","key":key,"value":invalid}
                ]}))
                .unwrap(),
            ))
            .unwrap();
        assert!(failed.is_error.unwrap_or(false));
        assert_eq!(report(server.get_property_edit()), staged);
        assert_eq!(lock(&shared).document().vmf(), &source);
        assert_eq!(lock(&shared).history_snapshot(), history);
    }
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
                {"action":"set","key":key,"value":raw}
            ]}))
            .unwrap(),
        )),
    );
    report(server.commit_property_edit(Parameters(PropertyTokenArgs { token })));
    assert_eq!(
        lock(&shared).document().vmf().world.properties.get(key),
        Some(raw)
    );
    assert_eq!(lock(&shared).document().undo_labels().count(), 1);
    let authored = lock(&shared).document().vmf().clone();
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    lock(&shared).redo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &authored);
    let opened = report(server.begin_property_edit(Parameters(
        serde_json::from_value(json!({"world":true})).unwrap(),
    )));
    let token = opened["draft"]["token"].as_u64().unwrap();
    let removed = report(
        server.update_property_edit(Parameters(
            serde_json::from_value(json!({"token":token,"changes":[
                {"action":"remove","key":key}
            ]}))
            .unwrap(),
        )),
    );
    assert_eq!(damping(&removed["preview"])["stored_count"], 0);
    assert_eq!(
        damping(&removed["preview"])["values"][0]["effective"],
        "0.01"
    );
    report(server.commit_property_edit(Parameters(PropertyTokenArgs { token })));
    assert_eq!(
        lock(&shared).document().vmf().world.properties.get(key),
        None
    );
    assert_eq!(server.rendering_status()["initialized"], false);
}
