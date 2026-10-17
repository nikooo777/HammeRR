use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn model_scale_mcp_properties_inspection_and_reload_share_session_without_graphics() {
    let Ok(game) = crate::startup::mount_game(None) else {
        return;
    };
    let shared = Arc::new(Mutex::new(Session::new(Ok(game))));
    let server = EditorServer::new(shared.clone(), None);
    let created = value(
        server.create_entity(Parameters(
            serde_json::from_value(json!({
                "classname":"prop_dynamic", "origin":[0,0,32],
                "properties":{"model":"models/props_junk/wood_crate001a.mdl"}
            }))
            .unwrap(),
        )),
    );
    let id = created["id"].as_str().unwrap();
    let query = || Parameters(serde_json::from_value(json!({"id":id})).unwrap());
    let before = lock(&shared).document().vmf().clone();
    let history = lock(&shared).document().undo_labels().count();
    let camera = lock(&shared).camera();
    let selection = lock(&shared).selected_ids();
    let unit = value(server.inspect_entity_model(query()));
    assert_eq!(unit["model_scale"]["effective"], 1.0);
    let inspector = value(server.get_entity_inspector(Parameters(
        serde_json::from_value(json!({"id":id})).unwrap(),
    )));
    let scale = inspector["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["key"] == "modelscale")
        .unwrap();
    assert_eq!(scale["control"], "number");
    assert_eq!(scale["value"], "1");
    assert_eq!(scale["range"], json!([0.015625, 64.0]));
    value(server.set_entity_properties(Parameters(
        serde_json::from_value(json!({"id":id,"set":{"modelscale":"0.5000000001"}})).unwrap(),
    )));
    let scaled = value(server.inspect_entity_model(query()));
    assert_eq!(scaled["model_scale"]["authored"], "0.5000000001");
    assert_eq!(scaled["model_scale"]["effective"], 0.5);
    assert_eq!(scaled["model_scale"]["applied"], true);
    assert_ne!(scaled["world_bounds"], unit["world_bounds"]);
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    let authored = lock(&shared).document().vmf().clone();
    let stamp = lock(&shared).history_snapshot();
    value(server.reload_model_assets());
    assert_eq!(value(server.inspect_entity_model(query())), scaled);
    assert_eq!(lock(&shared).document().vmf(), &authored);
    assert_eq!(lock(&shared).history_snapshot(), stamp);
    value(server.undo(Parameters(StepsArgs { steps: Some(1) })));
    assert_eq!(lock(&shared).document().vmf(), &before);
    value(server.set_entity_properties(Parameters(
        serde_json::from_value(json!({"id":id,"set":{"modelscale":"0"}})).unwrap(),
    )));
    let unsupported = value(server.inspect_entity_model(query()));
    assert_eq!(unsupported["model_scale"]["supported"], false);
    assert_eq!(unsupported["model_scale"]["applied"], false);
    assert_eq!(unsupported["helper"], true);
    assert!(
        unsupported["error"]
            .as_str()
            .unwrap()
            .contains("Model scale")
    );
    value(server.undo(Parameters(StepsArgs { steps: Some(1) })));
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(lock(&shared).selected_ids(), selection);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
