use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn prop_vertex_lighting_mcp_schema_forwarding_and_atomic_validation_use_shared_settings() {
    let shared = Arc::new(Mutex::new(Session::new(Err(
        "no assets or capture runtime".into(),
    ))));
    let server = EditorServer::new(shared.clone(), None);
    let source = lock(&shared).document().vmf().clone();
    let schema = serde_json::to_value(schemars::schema_for!(CompileSettingsArgs)).unwrap();
    assert!(schema["properties"]["bake_static_prop_vertices"].is_object());
    assert_eq!(schema["additionalProperties"], false);
    for invalid in [
        json!({"bake_static_prop_vertices":"true"}),
        json!({"bake_static_prop_vertices":true,"prop_fallback":true}),
    ] {
        assert!(serde_json::from_value::<CompileSettingsArgs>(invalid).is_err());
    }
    let initial = value(server.get_compile_settings());
    assert_eq!(initial["bake_static_prop_vertices"], false);
    for update in [
        json!({"bake_static_prop_vertices":true}),
        json!({"bake_static_prop_vertices":true,"lighting":"direct_ldr","bounces":2}),
    ] {
        let failed = server
            .set_compile_settings(Parameters(serde_json::from_value(update).unwrap()))
            .unwrap();
        assert!(failed.is_error.unwrap_or(false));
        assert_eq!(value(server.get_compile_settings()), initial);
    }
    let enabled = value(
        server.set_compile_settings(Parameters(
            serde_json::from_value(json!({
                "bake_static_prop_vertices":true,"lighting":"direct_ldr_hdr","bounces":0
            }))
            .unwrap(),
        )),
    );
    assert_eq!(enabled["bake_static_prop_vertices"], true);
    assert_eq!(enabled["prop_vertex_lighting"]["available"], true);
    assert_eq!(
        enabled["prop_vertex_lighting"]["limits"]["bounces"],
        json!({"min":0,"max":1})
    );
    assert_eq!(
        enabled["prop_vertex_lighting"]["limits"]["receiver"]["root_lod"],
        0
    );
    assert_eq!(
        enabled["prop_vertex_lighting"]["limits"]["receiver"]["hardware_meshes_max"],
        256
    );
    assert_eq!(
        enabled["prop_vertex_lighting"]["limits"]["receiver"]["lods"],
        json!({"min":1,"max":8,"all_required":true})
    );
    assert_eq!(enabled, lock(&shared).compile_settings());
    let bounced = value(server.set_compile_settings(Parameters(
        serde_json::from_value(json!({"bounces":1})).unwrap(),
    )));
    assert_eq!(bounced["bake_static_prop_vertices"], true);
    assert_eq!(bounced["bounces"], 1);
    let failed = server
        .set_compile_settings(Parameters(
            serde_json::from_value(json!({"bounces":2,"physics":"engine"})).unwrap(),
        ))
        .unwrap();
    assert!(failed.is_error.unwrap_or(false));
    assert_eq!(value(server.get_compile_settings()), bounced);
    let retained = value(
        server.set_compile_settings(Parameters(
            serde_json::from_value(json!({
                "bake_static_prop_vertices":null,"visibility":"detailed"
            }))
            .unwrap(),
        )),
    );
    assert_eq!(retained["bake_static_prop_vertices"], true);
    assert_eq!(retained["bounces"], 1);
    let failed = server
        .set_compile_settings(Parameters(
            serde_json::from_value(json!({"lighting":"fullbright"})).unwrap(),
        ))
        .unwrap();
    assert!(failed.is_error.unwrap_or(false));
    assert_eq!(value(server.get_compile_status())["settings"], retained);
    let disabled = value(
        server.set_compile_settings(Parameters(
            serde_json::from_value(json!({
                "bake_static_prop_vertices":false,"lighting":"fullbright","bounces":2
            }))
            .unwrap(),
        )),
    );
    assert_eq!(disabled["bake_static_prop_vertices"], false);
    assert_eq!(disabled["bounces"], 2);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), 0);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
