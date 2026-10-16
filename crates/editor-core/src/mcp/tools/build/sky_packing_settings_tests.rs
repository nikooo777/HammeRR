use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn sky_packing_mcp_schema_and_tools_share_atomic_settings_without_asset_or_renderer_access() {
    let mut session = Session::new(Err("settings need no mounted assets".into()));
    let brush = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "test/unchanged",
            false,
        )
        .unwrap();
    session
        .preview_transform(
            Actor::User,
            &[brush],
            map_core::edit::Transform {
                translation: DVec3::X * 8.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let token = session.property_draft().unwrap().token;
    session
        .append_property_change(
            token,
            PropertyChange::Set {
                key: "skyname".into(),
                value: "pending_family".into(),
            },
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let rendered = session.render_document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let draft = json!(session.property_draft());
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    assert_eq!(
        value(server.get_compile_settings())["pack_sky_assets"],
        false
    );
    let schema = serde_json::to_value(schemars::schema_for!(CompileSettingsArgs)).unwrap();
    assert!(schema["properties"]["pack_sky_assets"].is_object());
    assert_eq!(schema["additionalProperties"], false);
    for invalid in [
        json!({"pack_sky_assets":"true"}),
        json!({"pack_sky_assets":1}),
        json!({"pack_sky_assets":true,"skyname":"substitute"}),
        json!({"pack_sky_assets":true,"pack_sky_assets_fallback":true}),
    ] {
        assert!(serde_json::from_value::<CompileSettingsArgs>(invalid).is_err());
    }
    let enabled = value(server.set_compile_settings(Parameters(
        serde_json::from_value(json!({"pack_sky_assets":true})).unwrap(),
    )));
    assert_eq!(enabled["pack_sky_assets"], true);
    assert_eq!(enabled["lighting"], "fullbright");
    assert_eq!(enabled["generate_reflections"], false);
    assert_eq!(enabled, lock(&shared).compile_settings());
    assert!(lock(&shared).compile_options().pack_sky_assets);
    for invalid in [
        json!({"pack_sky_assets":false,"smoothing_angle":91}),
        json!({"pack_sky_assets":false,"generate_reflections":true}),
    ] {
        let failed = server
            .set_compile_settings(Parameters(serde_json::from_value(invalid).unwrap()))
            .unwrap();
        assert!(failed.is_error.unwrap_or(false));
        assert_eq!(value(server.get_compile_settings()), enabled);
    }
    let omitted = value(server.set_compile_settings(Parameters(
        serde_json::from_value(json!({"pack_sky_assets":null})).unwrap(),
    )));
    assert_eq!(omitted, enabled);
    let updated = value(server.set_compile_settings(Parameters(
        serde_json::from_value(json!({"visibility":"detailed"})).unwrap(),
    )));
    assert_eq!(updated["pack_sky_assets"], true);
    assert_eq!(updated["visibility"], "detailed");
    assert_eq!(value(server.get_compile_status())["settings"], updated);
    let disabled = value(server.set_compile_settings(Parameters(
        serde_json::from_value(json!({"pack_sky_assets":false})).unwrap(),
    )));
    assert_eq!(disabled["pack_sky_assets"], false);
    assert_eq!(disabled["visibility"], "detailed");
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.render_document().vmf(), &rendered);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(json!(session.property_draft()), draft);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
    assert!(
        crate::session::compilation::stats_json(&compiler::Stats::default())
            .get("sky_assets")
            .is_none()
    );
}
