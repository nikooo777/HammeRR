use super::*;

#[test]
fn prop_vertex_lighting_settings_validate_atomically_without_assets_or_document_changes() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("settings need no asset or native runtime".into()));
        session.begin_property_edit(PropertyTarget::World).unwrap();
        let token = session.property_draft().unwrap().token;
        session
            .append_property_change(
                token,
                PropertyChange::Set {
                    key: "skyname".into(),
                    value: "pending_sky".into(),
                },
            )
            .unwrap();
        let source = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let draft = json!(session.property_draft());
        let initial = session.compile_settings();
        assert_eq!(initial["bake_static_prop_vertices"], false);
        assert_eq!(initial["prop_vertex_lighting"]["available"], false);
        assert_eq!(
            initial["prop_vertex_lighting"]["limits"]["bounces"],
            json!({"min":0,"max":1})
        );
        assert_eq!(
            initial["prop_vertex_lighting"]["limits"]["receiver"]["self_shadowing"],
            json!({"default":true,"disable_property":"disableselfshadowing","disable_value":1,"disabled_scope":"receiver_owned_triangles","retained_blockers":["other_props","world","terrain"]})
        );
        assert!(
            !initial["prop_vertex_lighting"]["limits"]["unsupported"]
                .as_array()
                .unwrap()
                .contains(&json!("disabled self-shadowing"))
        );
        assert!(
            session
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        bake_static_prop_vertices: Some(true),
                        physics: Some("engine"),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert_eq!(session.compile_settings(), initial);
        session
            .update_build_settings(
                actor,
                BuildSettingsUpdate {
                    lighting: Some("direct_ldr"),
                    bounces: Some(2),
                    ..Default::default()
                },
            )
            .unwrap();
        let bouncing = session.compile_settings();
        assert_eq!(bouncing["prop_vertex_lighting"]["available"], false);
        assert!(
            session
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        bake_static_prop_vertices: Some(true),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert_eq!(session.compile_settings(), bouncing);
        session
            .update_build_settings(
                actor,
                BuildSettingsUpdate {
                    bounces: Some(0),
                    bake_static_prop_vertices: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        for lighting in ["direct_ldr", "direct_ldr_hdr"] {
            session.set_compile_settings(actor, lighting).unwrap();
            for bounces in [0, 1] {
                session
                    .update_build_settings(
                        actor,
                        BuildSettingsUpdate {
                            bounces: Some(bounces),
                            ..Default::default()
                        },
                    )
                    .unwrap();
                let enabled = session.compile_settings();
                assert_eq!(enabled["bake_static_prop_vertices"], true);
                assert_eq!(enabled["prop_vertex_lighting"]["available"], true);
                for update in [
                    BuildSettingsUpdate {
                        lighting: Some("fullbright"),
                        ..Default::default()
                    },
                    BuildSettingsUpdate {
                        bounces: Some(2),
                        physics: Some("engine"),
                        ..Default::default()
                    },
                ] {
                    assert!(session.update_build_settings(actor, update).is_err());
                    assert_eq!(session.compile_settings(), enabled);
                }
                session
                    .update_build_settings(actor, BuildSettingsUpdate::default())
                    .unwrap();
                assert_eq!(session.compile_settings(), enabled);
            }
        }
        session.closing = true;
        assert!(
            session
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        bake_static_prop_vertices: Some(false),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        session.closing = false;
        assert!(session.compile_options().bake_static_prop_vertices);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(json!(session.property_draft()), draft);
    }
}

#[test]
fn prop_vertex_lighting_jobs_freeze_cpu_settings_and_setting_changes_stale_published_output() {
    let (directory, game, _) = crate::test_assets::game_process(true);
    let path = directory.path().join("room.vmf");
    let mut session = Session::new(Ok(game));
    session.replace_document(
        Document::new(crate::test_assets::room()),
        Some(path.clone()),
    );
    std::fs::write(&path, session.document().vmf().write(&Default::default())).unwrap();
    session
        .set_compile_settings(Actor::User, "direct_ldr")
        .unwrap();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    session.finish_compile(Actor::User, &result);
    assert!(session.launch_readiness().is_ok());
    assert!(
        session.compile_status()["job"]["result"]["stats"]
            .get("prop_vertex_lighting")
            .is_none()
    );
    session
        .update_build_settings(
            Actor::Agent,
            BuildSettingsUpdate {
                bake_static_prop_vertices: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(session.launch_readiness().is_err());
    assert_eq!(session.compile_status()["job"]["stale"], true);
    session
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                bake_static_prop_vertices: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(session.launch_readiness().is_ok());
    session
        .update_build_settings(
            Actor::Agent,
            BuildSettingsUpdate {
                bake_static_prop_vertices: Some(true),
                lighting: Some("direct_ldr_hdr"),
                bounces: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    let request = session.begin_compile(Actor::Agent, None).unwrap();
    assert!(request.options.bake_static_prop_vertices);
    assert!(request.environment.is_none());
    assert!(request.native_plan.is_none());
    let captured = session.compile_status()["job"]["settings"].clone();
    assert_eq!(captured["bake_static_prop_vertices"], true);
    assert_eq!(captured["bounces"], 1);
    assert_eq!(
        session.compile_status()["job"]["native_runtime"],
        Value::Null
    );
    assert!(
        session
            .update_build_settings(
                Actor::User,
                BuildSettingsUpdate {
                    bake_static_prop_vertices: Some(false),
                    ..Default::default()
                }
            )
            .is_err()
    );
    assert_eq!(session.compile_status()["job"]["settings"], captured);
    session
        .cancel_compile(Actor::User, session.compile_token().unwrap())
        .unwrap();
    let result = request.run();
    assert!(result.is_err());
    session.finish_compile(Actor::Agent, &result);
    assert_eq!(session.compile_settings()["editable"], true);
    assert!(session.compile_options().bake_static_prop_vertices);
}
