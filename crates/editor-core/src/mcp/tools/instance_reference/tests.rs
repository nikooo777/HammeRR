use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn instance_reference_mcp_actions_share_owned_session_and_keep_graphics_lazy() {
    let (_directory, game, parent, _child) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.open(Actor::Agent, &parent, true).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let call = |arguments| {
        server.set_instance_reference(Parameters(serde_json::from_value(arguments).unwrap()))
    };
    let source = lock(&shared).document().vmf().clone();
    let token = value(call(json!({"action":"begin"})))["token"]
        .as_u64()
        .unwrap();
    value(call(
        json!({"action":"update","token":token,"file":"child.vmf","origin":"64 16 8","angles":"0 90 0"}),
    ));
    let report = value(call(json!({"action":"check","token":token})));
    assert_eq!(report["presence_checked"], true);
    assert_eq!(report["contents_validation"], "not_checked");
    assert_eq!(lock(&shared).document().vmf(), &source);
    let id = value(call(json!({"action":"apply","token":token})))["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(lock(&shared).document().undo_labels().count(), 1);
    assert!(value(server.get_instance_reference())["token"].is_null());
    let token = value(call(json!({"action":"begin","id":id})))["token"]
        .as_u64()
        .unwrap();
    for arguments in [
        json!({"action":"apply","token":token,"file":"other.vmf"}),
        json!({"action":"file","token":token}),
        json!({"action":"update","token":token}),
        json!({"action":"cancel","token":token,"id":id}),
    ] {
        assert!(call(arguments).unwrap().is_error.unwrap_or(false));
    }
    for arguments in [
        json!({"action":"unknown"}),
        json!({"action":"begin","typo":true}),
    ] {
        assert!(serde_json::from_value::<InstanceReferenceArgs>(arguments).is_err());
    }
    value(call(json!({"action":"cancel","token":token})));
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
