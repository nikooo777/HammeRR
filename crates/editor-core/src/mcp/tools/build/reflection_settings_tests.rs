use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn reflection_mcp_schema_and_tools_use_atomic_shared_settings() {
    let shared = Arc::new(Mutex::new(Session::new(Err("no assets needed".into()))));
    let server = EditorServer::new(shared.clone(), None);
    let original = lock(&shared).document().vmf().clone();
    let settings = value(server.get_compile_settings());
    assert_eq!(settings["generate_reflections"], false);
    let schema = serde_json::to_value(schemars::schema_for!(CompileSettingsArgs)).unwrap();
    assert!(schema["properties"]["generate_reflections"].is_object());
    let failed = server
        .set_compile_settings(Parameters(
            serde_json::from_value(json!({"generate_reflections":true,"physics":"engine"}))
                .unwrap(),
        ))
        .unwrap();
    assert!(failed.is_error.unwrap_or(false));
    assert_eq!(lock(&shared).compile_settings(), settings);
    let enabled = value(
        server.set_compile_settings(Parameters(
            serde_json::from_value(json!({"lighting":"direct_ldr","generate_reflections":true}))
                .unwrap(),
        )),
    );
    assert_eq!(enabled["generate_reflections"], true);
    assert_eq!(
        lock(&shared).compile_options().lighting,
        compiler::LightingMode::DirectLdr
    );
    let failed = server
        .set_compile_settings(Parameters(
            serde_json::from_value(json!({"lighting":"fullbright"})).unwrap(),
        ))
        .unwrap();
    assert!(failed.is_error.unwrap_or(false));
    assert_eq!(value(server.get_compile_settings()), enabled);
    let hdr = value(server.set_compile_settings(Parameters(
        serde_json::from_value(json!({"lighting":"direct_ldr_hdr"})).unwrap(),
    )));
    assert_eq!(hdr["generate_reflections"], true);
    assert_eq!(hdr["lighting"], "direct_ldr_hdr");
    let runtime = value(server.get_native_capture_runtime());
    let failed = server
        .set_native_capture_runtime(Parameters(
            serde_json::from_value(
                json!({"engine_root":"/missing/hammerr-capture-install","steam_root":null}),
            )
            .unwrap(),
        ))
        .unwrap();
    assert!(failed.is_error.unwrap_or(false));
    assert_eq!(value(server.get_native_capture_runtime()), runtime);
    assert!(
        serde_json::from_value::<NativeCaptureRuntimeArgs>(
            json!({"engine_root":"/tmp","command":"anything"})
        )
        .is_err()
    );
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert_eq!(lock(&shared).document().undo_labels().count(), 0);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
