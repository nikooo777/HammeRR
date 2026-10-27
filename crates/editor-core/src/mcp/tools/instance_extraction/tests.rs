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
    assert_eq!(report["pivot_mode"], "world_origin");
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

#[test]
fn instance_extraction_mcp_local_pivot_is_frozen_at_prepare_and_schema_checked() {
    let (directory, game, parent, ids) = crate::test_assets::instance_extraction_map();
    let mut session = Session::new(Ok(game));
    session.open(Actor::Agent, &parent, true).unwrap();
    session.select(&ids, SelectMode::Replace).unwrap();
    let original = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let call =
        |args| server.set_instance_extraction(Parameters(serde_json::from_value(args).unwrap()));
    for pivot in [
        json!({"kind":"custom"}),
        json!({"kind":"custom","position":[1,2]}),
        json!({"kind":"custom","position":[1,2,3],"typo":true}),
        json!({"kind":"world_origin","position":[1,2,3]}),
        json!({"kind":"unknown"}),
    ] {
        assert!(
            serde_json::from_value::<InstanceExtractionArgs>(
                json!({"action":"prepare","file":"invalid.vmf","pivot":pivot})
            )
            .is_err()
        );
    }
    assert!(call(json!({"action":"prepare","file":"invalid.vmf","pivot":{"kind":"custom","position":[1e300,0,0]}})).unwrap().is_error.unwrap_or(false));
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert!(!directory.path().join("invalid.vmf").exists());
    let report = value(call(
        json!({"action":"prepare","file":"local.vmf","pivot":{"kind":"custom","position":[12.375,-6.625,2.125]}}),
    ));
    assert_eq!(report["ready"], true, "{report}");
    assert_eq!(report["pivot_mode"], "custom");
    assert_eq!(report["pivot"], json!([12.375, -6.625, 2.125]));
    let token = report["token"].as_u64().unwrap();
    for action in ["extract", "review", "cancel"] {
        assert!(
            call(
                json!({"action":action,"token":token,"pivot":{"kind":"custom","position":[0,0,0]}})
            )
            .unwrap()
            .is_error
            .unwrap_or(false)
        );
    }
    assert_eq!(
        value(server.get_instance_extraction())["pivot"],
        report["pivot"]
    );
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert!(!directory.path().join("local.vmf").exists());
    value(call(json!({"action":"extract","token":token})));
    let helper = lock(&shared)
        .document()
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("func_instance"))
        .cloned()
        .unwrap();
    assert_eq!(helper.origin(), Some(DVec3::new(12.375, -6.625, 2.125)));
    lock(&shared).undo(Actor::Agent).unwrap();
    lock(&shared).select(&ids, SelectMode::Replace).unwrap();
    let centre = value(call(
        json!({"action":"prepare","file":"centre.vmf","pivot":{"kind":"selection_centre"}}),
    ));
    assert_eq!(centre["ready"], true, "{centre}");
    assert_eq!(centre["pivot_mode"], "selection_centre");
    assert_eq!(
        serde_json::from_value::<[f64; 3]>(centre["pivot"].clone()).unwrap(),
        [60.0, 48.0, 16.0]
    );
    value(call(json!({"action":"cancel","token":centre["token"]})));
    assert!(!directory.path().join("centre.vmf").exists());
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
