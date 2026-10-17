use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn instance_extraction_mcp_owns_atomic_review_and_remains_graphics_lazy() {
    let (directory, game, parent, ids) = crate::test_assets::instance_extraction_map();
    let mut session = Session::new(Ok(game));
    session.open(Actor::Agent, &parent, true).unwrap();
    session.select(&ids, SelectMode::Replace).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let original = lock(&shared).document().vmf().clone();
    let call =
        |args| server.set_instance_extraction(Parameters(serde_json::from_value(args).unwrap()));
    let report = value(call(json!({"action":"prepare","file":"new.vmf"})));
    let token = report["token"].as_u64().unwrap();
    assert_eq!(report["ready"], true);
    assert_eq!(report["selected"].as_array().unwrap().len(), 2);
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert!(!directory.path().join("new.vmf").exists());
    assert!(
        lock(&shared)
            .publish_instance_extraction(Actor::User, token)
            .is_err()
    );
    for args in [
        json!({"action":"extract","token":token,"file":"other.vmf"}),
        json!({"action":"cancel","token":token,"background":false}),
        json!({"action":"prepare","file":"other.vmf","token":token}),
    ] {
        assert!(call(args).unwrap().is_error.unwrap_or(false));
    }
    for args in [
        json!({"action":"unknown"}),
        json!({"action":"prepare","file":"other.vmf","typo":true}),
    ] {
        assert!(serde_json::from_value::<InstanceExtractionArgs>(args).is_err());
    }
    let result = value(call(json!({"action":"extract","token":token})));
    assert_eq!(result["last"]["published"], true);
    assert_eq!(lock(&shared).document().undo_labels().count(), 1);
    assert_eq!(
        value(call(json!({"action":"cancel","token":token})))["status"],
        "completed"
    );
    assert!(directory.path().join("new.vmf").exists());
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
