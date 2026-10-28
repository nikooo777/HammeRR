use super::*;

#[test]
fn brush_texture_shadows_share_atomic_settings_without_document_or_draft_changes() {
    let shared = Arc::new(Mutex::new(Session::new(Err("no assets needed".into()))));
    let server = EditorServer::new(shared.clone(), None);
    lock(&shared)
        .begin_property_edit(crate::session::PropertyTarget::World)
        .unwrap();
    let token = lock(&shared).property_draft().unwrap().token;
    lock(&shared)
        .append_property_change(
            token,
            crate::session::PropertyChange::Set {
                key: "message".into(),
                value: "Retained brush-shadow review".into(),
            },
        )
        .unwrap();
    let draft = json!(lock(&shared).property_draft());
    let camera = lock(&shared).camera();
    let schema = serde_json::to_value(schemars::schema_for!(CompileSettingsArgs)).unwrap();
    assert!(schema["properties"]["brush_texture_shadows"].is_object());
    assert!(schema["properties"]["refine_ambient_probes"].is_object());
    let initial = lock(&shared).compile_settings();
    assert_eq!(initial["brush_texture_shadows"], false);
    assert!(schema["properties"].get("brush_model_shadowing").is_none());
    assert_eq!(initial["brush_model_shadowing"]["available"], false);
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let failed = server
        .set_compile_settings(Parameters(
            serde_json::from_value(json!({"brush_texture_shadows":true,"physics":"engine"}))
                .unwrap(),
        ))
        .unwrap();
    assert!(failed.is_error.unwrap_or(false));
    assert_eq!(lock(&shared).compile_settings(), initial);
    let mut user = Session::new(Err("no assets needed".into()));
    for lighting in ["direct_ldr", "direct_ldr_hdr"] {
        let response = server
            .set_compile_settings(Parameters(
                serde_json::from_value(json!({"brush_texture_shadows":true,"lighting":lighting}))
                    .unwrap(),
            ))
            .unwrap();
        assert!(!response.is_error.unwrap_or(false));
        user.update_build_settings(
            Actor::User,
            crate::session::compilation::BuildSettingsUpdate {
                brush_texture_shadows: Some(true),
                lighting: Some(lighting),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(lock(&shared).compile_options(), user.compile_options());
        let retained = lock(&shared).compile_settings();
        assert_eq!(retained["brush_model_shadowing"]["available"], true);
        let failed = server
            .set_compile_settings(Parameters(
                serde_json::from_value(json!({"lighting":"fullbright"})).unwrap(),
            ))
            .unwrap();
        assert!(failed.is_error.unwrap_or(false));
        assert_eq!(lock(&shared).compile_settings(), retained);
    }
    let disabled = server
        .set_compile_settings(Parameters(
            serde_json::from_value(json!({"lighting":"fullbright","brush_texture_shadows":false}))
                .unwrap(),
        ))
        .unwrap();
    assert!(!disabled.is_error.unwrap_or(false));
    assert_eq!(
        lock(&shared).compile_options(),
        compiler::Options::default()
    );
    let refinement = server
        .set_compile_settings(Parameters(
            serde_json::from_value(json!({
                "lighting":"direct_ldr_hdr","sky_quality":"balanced","refine_ambient_probes":true
            }))
            .unwrap(),
        ))
        .unwrap();
    assert!(!refinement.is_error.unwrap_or(false));
    assert!(lock(&shared).compile_options().refine_ambient_probes);
    let retained = lock(&shared).compile_settings();
    let failed = server
        .set_compile_settings(Parameters(
            serde_json::from_value(json!({"sky_quality":"fast"})).unwrap(),
        ))
        .unwrap();
    assert!(failed.is_error.unwrap_or(false));
    assert_eq!(lock(&shared).compile_settings(), retained);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(json!(lock(&shared).property_draft()), draft);
    assert_eq!(lock(&shared).camera(), camera);
}
