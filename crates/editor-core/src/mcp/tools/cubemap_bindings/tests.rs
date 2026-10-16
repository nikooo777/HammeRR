use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn mcp_and_human_actions_share_the_same_owned_binding_draft_and_undo() {
    let (_assets, session, _, far, face) =
        crate::session::cubemap_bindings::test_support::fixture();
    let before = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let report = value(server.get_cubemap_bindings(Parameters(CubemapBindingsArgs {
        helper: Some(far),
        query: None,
        offset: None,
        limit: Some(1),
    })));
    let draft=value(server.set_cubemap_bindings(Parameters(serde_json::from_value(json!({"action":"preview","helper":far,"faces":[{"solid":face.solid,"side":face.side}],"operation":"bind","snapshot":report["state"]["snapshot"]})).unwrap())));
    let token = draft["draft"]["token"].as_u64().unwrap();
    assert_eq!(lock(&shared).document().vmf(), &before);
    let reviewed = lock(&shared)
        .review_cubemap_bindings(Actor::User, token)
        .unwrap();
    assert!(
        server
            .set_cubemap_bindings(Parameters(SetCubemapBindingsArgs::Apply { token }))
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    let token = reviewed["draft"]["token"].as_u64().unwrap();
    let reviewed =
        value(server.set_cubemap_bindings(Parameters(SetCubemapBindingsArgs::Review { token })));
    let token = reviewed["draft"]["token"].as_u64().unwrap();
    value(server.set_cubemap_bindings(Parameters(SetCubemapBindingsArgs::Apply { token })));
    assert_eq!(
        lock(&shared)
            .document()
            .entity(far)
            .unwrap()
            .properties
            .get("sides"),
        Some(face.side.to_string().as_str())
    );
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
