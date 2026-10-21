use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn static_prop_packing_mcp_schema_forwarding_and_atomic_settings_need_no_asset_or_renderer_access()
{
    let session = Session::new(Err("settings need no mounted assets".into()));
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    assert_eq!(
        value(server.get_compile_settings())["pack_static_prop_assets"],
        false
    );
    let schema = serde_json::to_value(schemars::schema_for!(CompileSettingsArgs)).unwrap();
    assert!(schema["properties"]["pack_static_prop_assets"].is_object());
    assert_eq!(schema["additionalProperties"], false);
    for invalid in [
        json!({"pack_static_prop_assets":"true"}),
        json!({"pack_static_prop_assets":1}),
        json!({"pack_static_prop_assets":true,"pack_static_prop_assets_fallback":true}),
    ] {
        assert!(serde_json::from_value::<CompileSettingsArgs>(invalid).is_err());
    }
    for lighting in ["fullbright", "direct_ldr", "direct_ldr_hdr"] {
        let enabled = value(
            server.set_compile_settings(Parameters(
                serde_json::from_value(json!({"pack_static_prop_assets":true,"lighting":lighting}))
                    .unwrap(),
            )),
        );
        assert_eq!(enabled["pack_static_prop_assets"], true);
        assert_eq!(enabled["pack_sky_assets"], false);
        assert_eq!(enabled["bake_static_prop_vertices"], false);
        assert_eq!(enabled["generate_reflections"], false);
        assert_eq!(enabled["lighting"], lighting);
        assert_eq!(enabled, lock(&shared).compile_settings());
        assert!(lock(&shared).compile_options().pack_static_prop_assets);
        let omitted = value(server.set_compile_settings(Parameters(
            serde_json::from_value(json!({"pack_static_prop_assets":null})).unwrap(),
        )));
        assert_eq!(omitted, enabled);
        let failed = server
            .set_compile_settings(Parameters(
                serde_json::from_value(
                    json!({"pack_static_prop_assets":false,"smoothing_angle":91}),
                )
                .unwrap(),
            ))
            .unwrap();
        assert!(failed.is_error.unwrap_or(false));
        assert_eq!(value(server.get_compile_settings()), enabled);
    }
    let baked = value(server.set_compile_settings(Parameters(
        serde_json::from_value(json!({"bake_static_prop_vertices":true})).unwrap(),
    )));
    assert_eq!(baked["pack_static_prop_assets"], true);
    assert_eq!(baked["bake_static_prop_vertices"], true);
    let updated = value(server.set_compile_settings(Parameters(
        serde_json::from_value(json!({"visibility":"detailed"})).unwrap(),
    )));
    assert_eq!(updated["pack_static_prop_assets"], true);
    assert_eq!(value(server.get_compile_status())["settings"], updated);
    let disabled = value(server.set_compile_settings(Parameters(
        serde_json::from_value(json!({"pack_static_prop_assets":false})).unwrap(),
    )));
    assert_eq!(disabled["pack_static_prop_assets"], false);
    assert_eq!(disabled["visibility"], "detailed");
    assert_eq!(disabled["bake_static_prop_vertices"], true);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
    assert!(
        crate::session::compilation::stats_json(&compiler::Stats::default())
            .get("static_prop_assets")
            .is_none()
    );
}
