use super::*;

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
                bounces: Some(compiler::MAX_LIGHT_BOUNCES),
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
    assert_eq!(captured["bounces"], compiler::MAX_LIGHT_BOUNCES);
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
