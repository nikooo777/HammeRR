use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn lighting_work_mcp_exposes_exact_units_and_uses_atomic_shared_settings() {
    let shared = Arc::new(Mutex::new(Session::new(Err("no assets needed".into()))));
    let server = EditorServer::new(shared.clone(), None);
    let schema = serde_json::to_value(schemars::schema_for!(CompileSettingsArgs)).unwrap();
    assert_eq!(schema["additionalProperties"], false);
    let property = &schema["properties"]["lighting_work_limit"];
    assert_eq!(property["minimum"], compiler::MIN_LIGHTING_WORK_LIMIT);
    assert_eq!(property["maximum"], compiler::MAX_LIGHTING_WORK_LIMIT);
    for invalid in [
        json!({"lighting_work_limit":-1}),
        json!({"lighting_work_limit":1.5}),
        json!({"lighting_work_limit":"1000000000"}),
        json!({"lighting_work_limit":true}),
        json!({"lighting_work_limit":1,"lighting_work_fallback":true}),
    ] {
        assert!(serde_json::from_value::<CompileSettingsArgs>(invalid).is_err());
    }
    let initial = value(server.get_compile_settings());
    assert_eq!(
        initial["lighting_work_limit"],
        compiler::DEFAULT_LIGHTING_WORK_LIMIT
    );
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    for limit in [0, compiler::MAX_LIGHTING_WORK_LIMIT + 1, u64::MAX] {
        let failed = server
            .set_compile_settings(Parameters(
                serde_json::from_value(
                    json!({"lighting_work_limit":limit,"lighting":"direct_ldr_hdr"}),
                )
                .unwrap(),
            ))
            .unwrap();
        assert!(failed.is_error.unwrap_or(false));
        assert_eq!(value(server.get_compile_settings()), initial);
    }
    let mut user = Session::new(Err("no assets needed".into()));
    for limit in [
        compiler::MIN_LIGHTING_WORK_LIMIT,
        1_234_567_890,
        compiler::MAX_LIGHTING_WORK_LIMIT,
    ] {
        let actual = value(server.set_compile_settings(Parameters(
            serde_json::from_value(json!({"lighting_work_limit":limit})).unwrap(),
        )));
        let expected = user
            .update_build_settings(
                Actor::User,
                crate::session::compilation::BuildSettingsUpdate {
                    lighting_work_limit: Some(limit),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual, lock(&shared).compile_settings());
        assert_eq!(lock(&shared).compile_options(), user.compile_options());
    }
    let retained = value(server.get_compile_settings());
    assert_eq!(
        value(server.set_compile_settings(Parameters(
            serde_json::from_value(json!({"lighting_work_limit":null})).unwrap(),
        ))),
        retained
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
}
