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

#[test]
fn mcp_root_bindings_use_child_sample_provenance_and_reject_refreshed_transitive_drift() {
    let fixture = crate::session::cubemap_bindings::test_support::instance_fixture();
    let before = fixture.session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(fixture.session));
    let server = EditorServer::new(shared.clone(), None);
    let report = value(server.get_cubemap_bindings(Parameters(CubemapBindingsArgs {
        helper: None,
        query: Some(fixture.face.side.to_string()),
        offset: None,
        limit: None,
    })));
    assert_eq!(report["available"], true, "{report}");
    assert_eq!(report["face_scope"], "root_authored_faces");
    let row = report["assignments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["side"] == fixture.face.side)
        .unwrap();
    assert_eq!(row["helper"], Value::Null);
    assert_eq!(row["helper_source"]["helper"], fixture.child_helper);
    assert_eq!(
        row["helper_source"]["occurrence"].as_array().unwrap().len(),
        2
    );
    let draft = value(server.set_cubemap_bindings(Parameters(serde_json::from_value(json!({
        "action":"preview","helper":fixture.far,"faces":[{"solid":fixture.face.solid,"side":fixture.face.side}],
        "operation":"bind","snapshot":report["state"]["snapshot"],
    })).unwrap())));
    let token = draft["draft"]["token"].as_u64().unwrap();
    assert_eq!(
        draft["draft"]["report"]["assignments"][0]["helper_source"],
        json!({"scope":"root","helper":fixture.far})
    );
    value(server.set_cubemap_bindings(Parameters(SetCubemapBindingsArgs::Apply { token })));
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &before);
    let stamp = lock(&shared).cubemap_bindings_stamp();
    let draft = value(server.set_cubemap_bindings(Parameters(serde_json::from_value(json!({
        "action":"preview","helper":fixture.far,"faces":[{"solid":fixture.face.solid,"side":fixture.face.side}],
        "operation":"bind","snapshot":stamp,
    })).unwrap())));
    let token = draft["draft"]["token"].as_u64().unwrap();
    let path = fixture.directory.path().join("leaf.vmf");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.push(b'\n');
    std::fs::write(path, bytes).unwrap();
    let refreshed = value(server.get_cubemap_bindings(Parameters(CubemapBindingsArgs {
        helper: None,
        query: None,
        offset: None,
        limit: None,
    })));
    assert_eq!(refreshed["available"], true);
    assert_eq!(refreshed["state"]["draft"]["valid"], false);
    for args in [
        SetCubemapBindingsArgs::Review { token },
        SetCubemapBindingsArgs::Apply { token },
    ] {
        assert!(
            server
                .set_cubemap_bindings(Parameters(args))
                .unwrap()
                .is_error
                .unwrap_or(false)
        );
    }
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(
        lock(&shared).cubemap_binding_state()["draft"]["token"],
        token
    );
    value(server.set_cubemap_bindings(Parameters(SetCubemapBindingsArgs::Cancel { token })));
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}

#[test]
fn mcp_cubemap_inspection_uses_shared_texture_shadow_settings_without_reviving_drafts() {
    let fixture = crate::session::cubemap_bindings::test_support::cutout_instance_fixture();
    let before = fixture.session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(fixture.session));
    let server = EditorServer::new(shared.clone(), None);
    let args = || {
        Parameters(CubemapBindingsArgs {
            helper: None,
            query: None,
            offset: None,
            limit: None,
        })
    };
    assert_eq!(
        value(server.get_cubemap_bindings(args()))["available"],
        false
    );
    value(
        server.set_compile_settings(Parameters(
            serde_json::from_value(json!({
                "lighting":"direct_ldr","brush_texture_shadows":true,
            }))
            .unwrap(),
        )),
    );
    let report = value(server.get_cubemap_bindings(args()));
    assert_eq!(report["available"], true, "{report}");
    assert_eq!(report["state"]["brush_texture_shadows"], true);
    let draft = value(server.set_cubemap_bindings(Parameters(serde_json::from_value(json!({
        "action":"preview","helper":fixture.far,"faces":[{"solid":fixture.face.solid,"side":fixture.face.side}],
        "operation":"bind","snapshot":report["state"]["snapshot"],
    })).unwrap())));
    let token = draft["draft"]["token"].as_u64().unwrap();
    for enabled in [false, true] {
        value(
            server.set_compile_settings(Parameters(
                serde_json::from_value(json!({
                    "brush_texture_shadows":enabled,
                }))
                .unwrap(),
            )),
        );
        let report = value(server.get_cubemap_bindings(args()));
        assert_eq!(report["available"], enabled, "{report}");
        assert_eq!(report["state"]["draft"]["valid"], false);
        assert!(
            server
                .set_cubemap_bindings(Parameters(SetCubemapBindingsArgs::Apply { token }))
                .unwrap()
                .is_error
                .unwrap_or(false)
        );
    }
    assert_eq!(lock(&shared).document().vmf(), &before);
    assert_eq!(
        lock(&shared).cubemap_binding_state()["draft"]["token"],
        token
    );
    value(server.set_cubemap_bindings(Parameters(SetCubemapBindingsArgs::Cancel { token })));
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
