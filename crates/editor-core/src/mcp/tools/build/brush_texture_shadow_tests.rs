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
    assert!(
        serde_json::from_value::<CompileSettingsArgs>(json!({"brush_texture_shadows":1})).is_err()
    );
    let initial = lock(&shared).compile_settings();
    assert_eq!(initial["brush_texture_shadows"], false);
    assert!(schema["properties"].get("brush_model_shadowing").is_none());
    assert_eq!(initial["brush_model_shadowing"]["available"], false);
    assert_eq!(
        initial["brush_model_shadowing"]["property"],
        "vrad_brush_cast_shadows"
    );
    assert_eq!(
        initial["brush_model_shadowing"]["fullbright"],
        "admit_and_preserve_without_baking"
    );
    assert_eq!(
        initial["brush_model_shadowing"]["baked_count"],
        "stats.brush_shadow_models"
    );
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
    assert_eq!(
        initial["brush_texture_shadowing"]["limits"]["global_root_model_scope"],
        json!({
            "property":"vrad_brush_cast_shadows","enabled_value":"1",
            "requires_brush_texture_shadows":true,"requires_lit_mode":true,
            "classname":"func_brush","pose":"existing_admitted_stationary_initial_pose",
            "requires_lit_receiver_scope":false,"child_models":false,
            "casts_into_world":true,"casts_into_other_models":true,
            "radiance_source":false,"world_occupancy":false
        })
    );
    assert_eq!(
        initial["brush_model_shadowing"]["limits"]["child_binary_cutout"],
        true
    );
    assert_eq!(
        initial["brush_texture_shadowing"]["limits"]["global_child_model_scope"],
        json!({
            "property":"vrad_brush_cast_shadows","enabled_value":"1",
            "requires_brush_texture_shadows":true,"requires_lit_mode":true,
            "classname":"func_brush","pose":"lowered_before_containing_transforms",
            "origin":"explicit_or_one_consumed_volume","requires_lit_receiver_scope":false,
            "reference_preparation":"deferred","optionless_compile_inspection":"ordinary_solid_only",
            "casts_into_world":true,"casts_into_other_models":true,
            "radiance_source":false,"world_occupancy":false
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
