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
    assert!(
        serde_json::from_value::<CompileSettingsArgs>(json!({"brush_texture_shadows":1})).is_err()
    );
    let initial = lock(&shared).compile_settings();
    assert_eq!(initial["brush_texture_shadows"], false);
    assert_eq!(initial["brush_texture_shadowing"]["available"], false);
    assert_eq!(
        initial["brush_texture_shadowing"]["limits"]["model_scope"],
        json!({
            "receivers":"same_model_only","pose":"existing_admitted_initial_pose",
            "nonzero_rotation":{"classname":"func_brush","parented":false},
            "requires_lit_receiver_scope":true,
            "casts_into_world":false,"casts_into_other_models":false,"radiance_source":false
        })
    );
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
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(json!(lock(&shared).property_draft()), draft);
    assert_eq!(lock(&shared).camera(), camera);
}
